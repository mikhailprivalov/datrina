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

/// W51: hand the chat trace / Pipeline Debug UI the redacted raw
/// payload behind a `raw_artifact_id` so the user can see the full
/// local artifact that backed a compressed tool result. Returns the
/// post-redaction `payload_json` plus retention metadata; never echoes
/// secrets.
#[tauri::command]
pub async fn get_raw_artifact(
    state: State<'_, AppState>,
    artifact_id: String,
) -> Result<ApiResult<Option<RawArtifactPayload>>, String> {
    match state.storage.get_raw_artifact(&artifact_id).await {
        Ok(None) => Ok(ApiResult::ok(None)),
        Ok(Some(record)) => Ok(ApiResult::ok(Some(RawArtifactPayload {
            id: record.id,
            owner_kind: record.owner_kind,
            owner_id: record.owner_id,
            profile: record.profile,
            raw_size: record.raw_size,
            compact_size: record.compact_size,
            checksum: record.checksum,
            redaction_version: record.redaction_version,
            retention_class: record.retention_class,
            payload_json: record.payload_json,
            created_at: record.created_at,
        }))),
        Err(e) => Ok(ApiResult::err(e.to_string())),
    }
}

/// W51: typed wire shape for `get_raw_artifact`. The TS side renders
/// this in the "raw local artifact" panel beside the compact view.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawArtifactPayload {
    pub id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub profile: String,
    pub raw_size: usize,
    pub compact_size: usize,
    pub checksum: String,
    pub redaction_version: u32,
    pub retention_class: String,
    pub payload_json: String,
    pub created_at: i64,
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

    // W47: live trace run inherits the dashboard's language policy so
    // the inspector shows the same prose language the user gets at
    // refresh time.
    let trace_language_directive = crate::commands::language::resolve_effective_language(
        state.storage.as_ref(),
        Some(dashboard_id),
        None,
    )
    .await
    .ok()
    .and_then(|resolved| resolved.system_directive());
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        crate::commands::dashboard::active_provider_public(state).await?,
    )
    .with_language(trace_language_directive);
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
    // W47: trace-time pipeline runs against the dashboard's resolved
    // assistant language, same as the live refresh path.
    let language_directive = crate::commands::language::resolve_effective_language(
        state.storage.as_ref(),
        Some(dashboard_id),
        None,
    )
    .await
    .ok()
    .and_then(|resolved| resolved.system_directive());
    let (final_value, step_traces) = run_pipeline_with_trace(
        initial,
        &steps,
        Some(state.ai_engine.as_ref()),
        provider.as_ref(),
        Some(state.mcp_manager.as_ref()),
        language_directive.as_deref(),
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
        | Widget::Heatmap { datasource, .. }
        | Widget::Gallery { datasource, .. } => Some(datasource),
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

// ─── W32: Typed pipeline replay ─────────────────────────────────────────────

/// Replay a candidate pipeline against an explicit sample value. Reuses
/// the production `run_pipeline_with_trace` runner so the result matches
/// what a live workflow would produce on the same input. Provider /
/// MCP-aware steps (`llm_postprocess`, `mcp_call`) are intentionally not
/// connected here: the Studio is a deterministic preview surface, so
/// those steps return a typed "skipped" trace entry instead of hitting
/// the network. Use the W23 Debug view for full-runtime traces.
#[tauri::command]
pub async fn replay_pipeline(
    state: State<'_, AppState>,
    req: ReplayPipelineRequest,
) -> Result<ApiResult<PipelineReplayResult>, String> {
    Ok(match replay_pipeline_inner(&state, req).await {
        Ok(value) => ApiResult::ok(value),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplayPipelineRequest {
    pub steps: Vec<PipelineStep>,
    /// Inline sample value the pipeline starts from. Required when
    /// `from_widget_trace` is `None`.
    #[serde(default)]
    pub sample: Option<Value>,
    /// Replay against a stored W23 trace: we pull the trace's first
    /// recorded input sample as the initial value. The Studio surfaces
    /// this when seeded from the Debug modal.
    #[serde(default)]
    pub from_widget_trace: Option<WidgetTraceRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WidgetTraceRef {
    pub widget_id: String,
    pub captured_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineReplayResult {
    pub started_at: i64,
    pub finished_at: i64,
    pub initial_value: Option<Value>,
    pub steps: Vec<crate::models::pipeline::PipelineStepTrace>,
    pub final_value: Option<Value>,
    pub error: Option<String>,
    /// 0-based index of the first step whose output became empty
    /// (null / empty array) or that errored. `None` when every step
    /// produced data. Helps the UI point at the broken step.
    pub first_empty_step_index: Option<u32>,
}

async fn replay_pipeline_inner(
    state: &State<'_, AppState>,
    req: ReplayPipelineRequest,
) -> AnyResult<PipelineReplayResult> {
    let started_at = chrono::Utc::now().timestamp_millis();
    let initial = match (&req.sample, &req.from_widget_trace) {
        (Some(v), _) => v.clone(),
        (None, Some(reference)) => {
            let stored = state
                .storage
                .get_widget_trace(&reference.widget_id, reference.captured_at)
                .await?
                .ok_or_else(|| anyhow!("stored trace not found"))?;
            let parsed: PipelineTrace = serde_json::from_str(&stored)?;
            // The pipeline node's input equals the first step's input
            // sample. For an empty pipeline, fall back to the trace's
            // final value (still useful for ad-hoc replays).
            if let Some(first) = parsed.steps.first() {
                first.input_sample.preview.clone()
            } else {
                parsed.final_value.unwrap_or(Value::Null)
            }
        }
        (None, None) => {
            return Err(anyhow!(
                "replay_pipeline requires either `sample` or `from_widget_trace`"
            ));
        }
    };
    // Deterministic-only: deny provider/MCP steps so the Studio never
    // surprises the user with a network call or a cost line item.
    if let Some(bad) = req.steps.iter().find(|s| {
        matches!(
            s,
            PipelineStep::LlmPostprocess { .. } | PipelineStep::McpCall { .. }
        )
    }) {
        return Err(anyhow!(
            "Studio replay cannot execute {} steps — use the Debug view's full traced run.",
            crate::modules::workflow_engine::pipeline_step_kind_public(bad)
        ));
    }
    let (final_value, step_traces) =
        run_pipeline_with_trace(initial.clone(), &req.steps, None, None, None, None).await;
    let finished_at = chrono::Utc::now().timestamp_millis();
    let error = step_traces.iter().find_map(|s| s.error.clone());
    let first_empty_step_index = first_empty_step(&step_traces);
    Ok(PipelineReplayResult {
        started_at,
        finished_at,
        initial_value: Some(initial),
        steps: step_traces,
        final_value: Some(final_value),
        error,
        first_empty_step_index,
    })
}

fn first_empty_step(steps: &[crate::models::pipeline::PipelineStepTrace]) -> Option<u32> {
    use crate::models::pipeline::SampleKind;
    for step in steps {
        if step.error.is_some() {
            return Some(step.index);
        }
        match step.output_sample.kind {
            SampleKind::Null => return Some(step.index),
            SampleKind::ArrayHead => {
                if step.output_sample.size_hint.items.unwrap_or(0) == 0 {
                    return Some(step.index);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pipeline::{
        PipelineStep, PipelineStepTrace, SampleKind, SampleValue, SizeHint,
    };

    fn step_trace(
        index: u32,
        kind: &str,
        output: SampleValue,
        error: Option<&str>,
    ) -> PipelineStepTrace {
        PipelineStepTrace {
            index,
            kind: kind.to_string(),
            config_json: Value::Null,
            input_sample: SampleValue {
                kind: SampleKind::Value,
                size_hint: SizeHint::default(),
                preview: Value::Null,
            },
            output_sample: output,
            duration_ms: 0,
            error: error.map(|s| s.to_string()),
        }
    }

    #[test]
    fn first_empty_step_returns_none_when_every_step_has_data() {
        let traces = vec![step_trace(
            0,
            "pick",
            SampleValue {
                kind: SampleKind::ArrayHead,
                size_hint: SizeHint {
                    items: Some(3),
                    bytes: None,
                },
                preview: json!([1, 2, 3]),
            },
            None,
        )];
        assert_eq!(first_empty_step(&traces), None);
    }

    #[test]
    fn first_empty_step_flags_empty_array() {
        let traces = vec![
            step_trace(
                0,
                "pick",
                SampleValue {
                    kind: SampleKind::ArrayHead,
                    size_hint: SizeHint {
                        items: Some(2),
                        bytes: None,
                    },
                    preview: json!([1, 2]),
                },
                None,
            ),
            step_trace(
                1,
                "filter",
                SampleValue {
                    kind: SampleKind::ArrayHead,
                    size_hint: SizeHint {
                        items: Some(0),
                        bytes: None,
                    },
                    preview: json!([]),
                },
                None,
            ),
        ];
        assert_eq!(first_empty_step(&traces), Some(1));
    }

    #[test]
    fn first_empty_step_flags_step_error_before_empty() {
        let traces = vec![step_trace(
            0,
            "pick",
            SampleValue {
                kind: SampleKind::Null,
                size_hint: SizeHint::default(),
                preview: Value::Null,
            },
            Some("path not found"),
        )];
        assert_eq!(first_empty_step(&traces), Some(0));
    }

    #[tokio::test]
    async fn replay_matches_live_pipeline_for_deterministic_steps() {
        let steps = vec![
            PipelineStep::Pick {
                path: "items".into(),
            },
            PipelineStep::Limit { count: 2 },
            PipelineStep::Length,
        ];
        let sample = json!({ "items": [{"id":1},{"id":2},{"id":3}] });
        let (live_final, _live_traces) = crate::modules::workflow_engine::run_pipeline_with_trace(
            sample.clone(),
            &steps,
            None,
            None,
            None,
            None,
        )
        .await;
        let (replay_final, _replay_traces) =
            crate::modules::workflow_engine::run_pipeline_with_trace(
                sample.clone(),
                &steps,
                None,
                None,
                None,
                None,
            )
            .await;
        assert_eq!(live_final, replay_final);
        assert_eq!(replay_final, json!(2));
    }
}
