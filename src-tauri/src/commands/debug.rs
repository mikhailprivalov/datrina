//! W23 — pipeline debug view.
//!
//! Provides per-widget pipeline trace capture for the Debug modal and the
//! W18 reflection turn. Trace execution piggybacks on the regular datasource
//! workflow: we run the workflow once, locate the pipeline transform node,
//! and re-run its steps through [`run_pipeline_with_trace`] so we can stream
//! per-step samples + timings without paying observability cost on every
//! production refresh.

use anyhow::{anyhow, Result as AnyResult};
use serde_json::{json, Value};
use tauri::State;
use tracing::info;

use crate::commands::dashboard::WidgetDatasource;
use crate::models::pipeline::{PipelineStep, PipelineTrace, SourceSummary};
use crate::models::widget::Widget;
use crate::models::workflow::{NodeKind, RunStatus, Workflow, WorkflowNode};
use crate::models::ApiResult;
use crate::modules::workflow_engine::{run_pipeline_with_trace, WorkflowEngine};
use crate::AppState;

#[tauri::command]
pub async fn trace_widget_pipeline(
    state: State<'_, AppState>,
    dashboard_id: String,
    widget_id: String,
) -> Result<ApiResult<PipelineTrace>, String> {
    Ok(
        match trace_widget_pipeline_inner(&state, &dashboard_id, &widget_id, false).await {
            Ok(trace) => ApiResult::ok(trace),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn list_widget_traces(
    state: State<'_, AppState>,
    widget_id: String,
) -> Result<ApiResult<Vec<TraceEntry>>, String> {
    Ok(match list_widget_traces_inner(&state, &widget_id).await {
        Ok(entries) => ApiResult::ok(entries),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_widget_trace(
    state: State<'_, AppState>,
    widget_id: String,
    captured_at: i64,
) -> Result<ApiResult<Option<PipelineTrace>>, String> {
    Ok(
        match get_widget_trace_inner(&state, &widget_id, captured_at).await {
            Ok(value) => ApiResult::ok(value),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn set_widget_capture_traces(
    state: State<'_, AppState>,
    dashboard_id: String,
    widget_id: String,
    capture: bool,
) -> Result<ApiResult<bool>, String> {
    Ok(
        match set_widget_capture_traces_inner(&state, &dashboard_id, &widget_id, capture).await {
            Ok(()) => ApiResult::ok(true),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceEntry {
    pub captured_at: i64,
    pub trace: PipelineTrace,
}

pub(crate) async fn trace_widget_pipeline_inner(
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_id: &str,
    persist: bool,
) -> AnyResult<PipelineTrace> {
    let started_at = chrono::Utc::now().timestamp_millis();
    let dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let widget = dashboard
        .layout
        .iter()
        .find(|w| w.id() == widget_id)
        .ok_or_else(|| anyhow!("Widget not found"))?;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow!("Widget has no datasource workflow"))?;
    let workflow = match state.storage.get_workflow(&datasource.workflow_id).await? {
        Some(w) => w,
        None => dashboard
            .workflows
            .iter()
            .find(|w| w.id == datasource.workflow_id)
            .cloned()
            .ok_or_else(|| anyhow!("Datasource workflow not found"))?,
    };

    let source_summary = derive_source_summary(&workflow);

    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        crate::commands::dashboard::active_provider_public(state).await?,
    );
    let execution = engine.execute(&workflow, None).await?;
    let run = execution.run;

    if !matches!(run.status, RunStatus::Success) {
        let trace = PipelineTrace {
            workflow_id: workflow.id.clone(),
            widget_id: widget_id.to_string(),
            started_at,
            finished_at: chrono::Utc::now().timestamp_millis(),
            source_summary,
            steps: Vec::new(),
            final_value: None,
            error: Some(run.error.unwrap_or_else(|| "workflow failed".to_string())),
        };
        if persist {
            persist_trace(state, widget_id, &trace).await?;
        }
        return Ok(trace);
    }
    let context = run
        .node_results
        .ok_or_else(|| anyhow!("Datasource workflow returned no node results"))?;

    let (initial, steps) = locate_pipeline_initial(&workflow, &context)?;

    let provider = crate::commands::dashboard::active_provider_public(state).await?;
    let (final_value, step_traces) = run_pipeline_with_trace(
        initial,
        &steps,
        Some(state.ai_engine.as_ref()),
        provider.as_ref(),
        Some(state.mcp_manager.as_ref()),
    )
    .await;

    let finished_at = chrono::Utc::now().timestamp_millis();
    let final_value = truncate_final_value(final_value);
    let error = step_traces.iter().find_map(|s| s.error.clone());
    let trace = PipelineTrace {
        workflow_id: workflow.id.clone(),
        widget_id: widget_id.to_string(),
        started_at,
        finished_at,
        source_summary,
        steps: step_traces,
        final_value,
        error,
    };
    if persist {
        persist_trace(state, widget_id, &trace).await?;
    }
    Ok(trace)
}

pub(crate) async fn capture_trace_after_refresh(
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_id: &str,
) {
    match trace_widget_pipeline_inner(state, dashboard_id, widget_id, true).await {
        Ok(_) => info!("📊 Captured trace for widget {}", widget_id),
        Err(error) => tracing::warn!("trace capture failed for widget {}: {}", widget_id, error),
    }
}

async fn list_widget_traces_inner(
    state: &State<'_, AppState>,
    widget_id: &str,
) -> AnyResult<Vec<TraceEntry>> {
    let rows = state.storage.list_widget_traces(widget_id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (captured_at, trace_json) in rows {
        let trace: PipelineTrace = serde_json::from_str(&trace_json)
            .map_err(|e| anyhow!("stored trace was unparseable: {}", e))?;
        out.push(TraceEntry { captured_at, trace });
    }
    Ok(out)
}

async fn get_widget_trace_inner(
    state: &State<'_, AppState>,
    widget_id: &str,
    captured_at: i64,
) -> AnyResult<Option<PipelineTrace>> {
    let row = state
        .storage
        .get_widget_trace(widget_id, captured_at)
        .await?;
    match row {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

async fn set_widget_capture_traces_inner(
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_id: &str,
    capture: bool,
) -> AnyResult<()> {
    let mut dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let mut changed = false;
    for widget in dashboard.layout.iter_mut() {
        if widget.id() != widget_id {
            continue;
        }
        if let Some(ds) = widget_datasource_mut(widget) {
            if let Some(cfg) = ds.as_mut() {
                if cfg.capture_traces != capture {
                    cfg.capture_traces = capture;
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(());
    }
    dashboard.updated_at = chrono::Utc::now().timestamp_millis();
    state.storage.update_dashboard(&dashboard).await?;
    Ok(())
}

fn widget_datasource_mut(
    widget: &mut Widget,
) -> Option<&mut Option<crate::models::widget::DatasourceConfig>> {
    match widget {
        Widget::Chart { datasource, .. }
        | Widget::Text { datasource, .. }
        | Widget::Table { datasource, .. }
        | Widget::Image { datasource, .. }
        | Widget::Gauge { datasource, .. }
        | Widget::Stat { datasource, .. }
        | Widget::Logs { datasource, .. }
        | Widget::BarGauge { datasource, .. }
        | Widget::StatusGrid { datasource, .. }
        | Widget::Heatmap { datasource, .. } => Some(datasource),
    }
}

async fn persist_trace(
    state: &State<'_, AppState>,
    widget_id: &str,
    trace: &PipelineTrace,
) -> AnyResult<()> {
    let trace_json = serde_json::to_string(trace)?;
    state
        .storage
        .insert_widget_trace(widget_id, trace.finished_at, &trace_json)
        .await
}

fn derive_source_summary(workflow: &Workflow) -> SourceSummary {
    let Some(source) = workflow.nodes.iter().find(|n| n.id == "source") else {
        return SourceSummary::Unknown;
    };
    let empty = json!({});
    let config = source.config.as_ref().unwrap_or(&empty);
    match source.kind {
        NodeKind::McpTool => {
            let server_id = config
                .get("server_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tool_name = config
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = config.get("arguments").cloned();
            if server_id == "builtin" {
                SourceSummary::BuiltinTool {
                    tool_name,
                    arguments,
                }
            } else {
                SourceSummary::McpTool {
                    server_id,
                    tool_name,
                    arguments,
                }
            }
        }
        NodeKind::Llm => SourceSummary::ProviderPrompt {
            prompt: config
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        _ => SourceSummary::Unknown,
    }
}

/// Find the `pipeline` Transform node, resolve its initial input from the
/// workflow context, and extract its pipeline steps. If the widget has no
/// pipeline transform (source-only or shape-only widget), returns an empty
/// step list with the tail node's value as the final.
fn locate_pipeline_initial(
    workflow: &Workflow,
    context: &Value,
) -> AnyResult<(Value, Vec<PipelineStep>)> {
    let pipeline_node = workflow
        .nodes
        .iter()
        .find(|n| n.id == "pipeline" && matches!(n.kind, NodeKind::Transform));
    let Some(pipeline_node) = pipeline_node else {
        return Ok((Value::Null, Vec::new()));
    };
    let (initial, steps) = pipeline_node_input(pipeline_node, context)?;
    Ok((initial, steps))
}

fn pipeline_node_input(
    pipeline_node: &WorkflowNode,
    context: &Value,
) -> AnyResult<(Value, Vec<PipelineStep>)> {
    let empty = json!({});
    let config = pipeline_node.config.as_ref().unwrap_or(&empty);
    let input_key = config
        .get("input_key")
        .and_then(Value::as_str)
        .unwrap_or("__input")
        .to_string();
    let steps_value = config
        .get("steps")
        .cloned()
        .ok_or_else(|| anyhow!("pipeline node missing steps"))?;
    let steps: Vec<PipelineStep> = serde_json::from_value(steps_value)
        .map_err(|e| anyhow!("pipeline steps malformed: {}", e))?;
    let initial = context.get(&input_key).cloned().unwrap_or(Value::Null);
    Ok((initial, steps))
}

/// Truncate the final value preview to ~8 KB so the persisted trace stays
/// bounded. Returns `None` if serialization fails.
fn truncate_final_value(value: Value) -> Option<Value> {
    const MAX_BYTES: usize = 8 * 1024;
    let serialized = serde_json::to_string(&value).ok()?;
    if serialized.len() <= MAX_BYTES {
        return Some(value);
    }
    let truncated: String = serialized.chars().take(MAX_BYTES).collect();
    Some(Value::String(format!(
        "{}… [{} bytes total, truncated]",
        truncated,
        serialized.len()
    )))
}

pub fn trace_summary_for_reflection(trace: &PipelineTrace) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Widget {} pipeline trace ({} step(s))\n",
        trace.widget_id,
        trace.steps.len()
    ));
    if let Some(error) = &trace.error {
        out.push_str(&format!("error: {}\n", error));
    }
    for step in &trace.steps {
        out.push_str(&format!(
            "  step {}: {} — items {:?} → {:?}{}\n",
            step.index,
            step.kind,
            step.input_sample.size_hint.items,
            step.output_sample.size_hint.items,
            step.error
                .as_deref()
                .map(|e| format!(" [error: {}]", e))
                .unwrap_or_default()
        ));
    }
    out
}
