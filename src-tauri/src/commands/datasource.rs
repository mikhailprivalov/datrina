//! W30: Workbench commands for saved datasource definitions.
//!
//! Every command here is a thin wrapper around the existing primitives:
//! [`crate::commands::dashboard::datasource_plan_workflow`] for workflow
//! shape, [`crate::modules::workflow_engine::WorkflowEngine`] for
//! execution, and `Storage` for persistence. The Workbench surface
//! never bypasses the runtime path that real dashboard widgets use.

use anyhow::{anyhow, Result as AnyResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::commands::dashboard::{
    active_provider, datasource_plan_workflow, reconnect_enabled_mcp_servers,
    schedule_workflow_if_cron,
};
use crate::models::dashboard::{
    BuildDatasourcePlan, BuildDatasourcePlanKind, BuildWidgetProposal, BuildWidgetType, Dashboard,
};
use crate::models::datasource::{
    CreateDatasourceRequest, DatasourceBindingChange, DatasourceConsumer, DatasourceDefinition,
    DatasourceExportBundle, DatasourceHealth, DatasourceHealthStatus, DatasourceImpactPreview,
    DatasourceRunResult, UpdateDatasourceRequest,
};
use crate::models::external_source::{ExternalSource, ExternalSourceCredentialPolicy};
use crate::models::widget::{DatasourceBindingSource, DatasourceConfig, Widget};
use crate::models::workflow::{RunStatus, Workflow};
use crate::models::ApiResult;
use crate::modules::workflow_engine::WorkflowEngine;
use crate::AppState;

const EXPORT_BUNDLE_VERSION: u32 = 1;
const SAMPLE_PREVIEW_BYTES: usize = 4_096;

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Build a fresh workflow from a [`DatasourceDefinition`]. The shape is
/// intentionally the same as `datasource_plan_workflow` for a non-shared
/// plan: source → optional pipeline → `output.data`. Consumer widgets bind
/// to it through the standard `DatasourceConfig.workflow_id` / `output_key`
/// pair, so per-widget tail pipelines remain a future extension without
/// requiring a parallel engine today.
fn build_workflow_for_definition(def: &DatasourceDefinition, now: i64) -> AnyResult<Workflow> {
    if matches!(def.kind, BuildDatasourcePlanKind::Shared) {
        return Err(anyhow!(
            "DatasourceDefinition cannot have kind='shared'; shared keys live on a build proposal only"
        ));
    }
    if matches!(def.kind, BuildDatasourcePlanKind::Compose) {
        return Err(anyhow!(
            "DatasourceDefinition cannot have kind='compose'; compose plans are widget-scoped, not saved definitions"
        ));
    }
    let plan = BuildDatasourcePlan {
        kind: def.kind.clone(),
        tool_name: def.tool_name.clone(),
        server_id: def.server_id.clone(),
        arguments: def.arguments.clone(),
        prompt: def.prompt.clone(),
        output_path: None,
        refresh_cron: def.refresh_cron.clone(),
        pipeline: def.pipeline.clone(),
        source_key: None,
        inputs: None,
    };
    // The proposal is only used by datasource_plan_workflow for the
    // human-readable description; we forge a stand-in carrying the
    // definition name.
    let synthetic_proposal = BuildWidgetProposal {
        widget_type: BuildWidgetType::Text,
        title: def.name.clone(),
        data: Value::Null,
        datasource_plan: Some(plan.clone()),
        config: None,
        x: None,
        y: None,
        w: None,
        h: None,
        replace_widget_id: None,
        size_preset: None,
        layout_pattern: None,
    };
    datasource_plan_workflow(
        def.workflow_id.clone(),
        format!("Datasource: {}", def.name),
        &synthetic_proposal,
        &plan,
        now,
    )
}

fn truncate_preview(value: &Value) -> Option<Value> {
    let serialized = serde_json::to_string(value).ok()?;
    if serialized.len() <= SAMPLE_PREVIEW_BYTES {
        return Some(value.clone());
    }
    // For arrays / objects, hint at the truncation instead of returning a
    // potentially mis-parsed substring.
    Some(json!({
        "_truncated": true,
        "bytes": serialized.len(),
        "head": serialized.chars().take(SAMPLE_PREVIEW_BYTES).collect::<String>(),
    }))
}

/// W31: lift the optional datasource binding off any widget variant.
pub(crate) fn widget_datasource(widget: &Widget) -> Option<&DatasourceConfig> {
    match widget {
        Widget::Chart { datasource, .. } => datasource.as_ref(),
        Widget::Text { datasource, .. } => datasource.as_ref(),
        Widget::Table { datasource, .. } => datasource.as_ref(),
        Widget::Image { datasource, .. } => datasource.as_ref(),
        Widget::Gauge { datasource, .. } => datasource.as_ref(),
        Widget::Stat { datasource, .. } => datasource.as_ref(),
        Widget::Logs { datasource, .. } => datasource.as_ref(),
        Widget::BarGauge { datasource, .. } => datasource.as_ref(),
        Widget::StatusGrid { datasource, .. } => datasource.as_ref(),
        Widget::Heatmap { datasource, .. } => datasource.as_ref(),
        Widget::Gallery { datasource, .. } => datasource.as_ref(),
    }
}

pub(crate) fn widget_datasource_mut(widget: &mut Widget) -> Option<&mut Option<DatasourceConfig>> {
    Some(match widget {
        Widget::Chart { datasource, .. } => datasource,
        Widget::Text { datasource, .. } => datasource,
        Widget::Table { datasource, .. } => datasource,
        Widget::Image { datasource, .. } => datasource,
        Widget::Gauge { datasource, .. } => datasource,
        Widget::Stat { datasource, .. } => datasource,
        Widget::Logs { datasource, .. } => datasource,
        Widget::BarGauge { datasource, .. } => datasource,
        Widget::StatusGrid { datasource, .. } => datasource,
        Widget::Heatmap { datasource, .. } => datasource,
        Widget::Gallery { datasource, .. } => datasource,
    })
}

pub(crate) fn widget_kind(widget: &Widget) -> &'static str {
    match widget {
        Widget::Chart { .. } => "chart",
        Widget::Text { .. } => "text",
        Widget::Table { .. } => "table",
        Widget::Image { .. } => "image",
        Widget::Gauge { .. } => "gauge",
        Widget::Stat { .. } => "stat",
        Widget::Logs { .. } => "logs",
        Widget::BarGauge { .. } => "bar_gauge",
        Widget::StatusGrid { .. } => "status_grid",
        Widget::Heatmap { .. } => "heatmap",
        Widget::Gallery { .. } => "gallery",
    }
}

/// W31: which datasource definition a widget consumes, if any. Returns
/// the explicit `datasource_definition_id` first; falls back to a
/// `workflow_id` lookup so legacy widgets still resolve.
async fn matches_definition(
    state: &State<'_, AppState>,
    config: &DatasourceConfig,
    def: &DatasourceDefinition,
) -> AnyResult<MatchKind> {
    if let Some(bound_id) = config.datasource_definition_id.as_deref() {
        if bound_id == def.id {
            return Ok(MatchKind::Explicit);
        }
        // Explicit binding to a different definition — never match by
        // workflow id; the caller has chosen ownership.
        return Ok(MatchKind::None);
    }
    if config.workflow_id == def.workflow_id {
        return Ok(MatchKind::Legacy);
    }
    // Stretch: a widget might point at a workflow that belongs to a
    // different definition with the same backing workflow id. The
    // index on `workflow_id` is unique per definition in practice, but
    // we still verify by storage so manual edits cannot create
    // ambiguous bindings.
    if let Some(other) = state
        .storage
        .get_datasource_by_workflow_id(&config.workflow_id)
        .await?
    {
        if other.id == def.id {
            return Ok(MatchKind::Legacy);
        }
    }
    Ok(MatchKind::None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Explicit,
    Legacy,
    None,
}

fn build_consumer(
    dashboard: &Dashboard,
    widget: &Widget,
    config: &DatasourceConfig,
    explicit: bool,
) -> DatasourceConsumer {
    DatasourceConsumer {
        dashboard_id: dashboard.id.clone(),
        dashboard_name: dashboard.name.clone(),
        widget_id: widget.id().to_string(),
        widget_title: widget.title().to_string(),
        widget_kind: widget_kind(widget).to_string(),
        output_key: config.output_key.clone(),
        explicit_binding: explicit,
        binding_source: config.binding_source,
        bound_at: config.bound_at,
        tail_step_count: config.tail_pipeline.len() as u32,
    }
}

async fn run_definition_once(
    state: &State<'_, AppState>,
    def: &DatasourceDefinition,
) -> AnyResult<DatasourceRunResult> {
    reconnect_enabled_mcp_servers(state).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let workflow = build_workflow_for_definition(def, now)?;
    let workflow_node_ids: Vec<String> = workflow.nodes.iter().map(|n| n.id.clone()).collect();

    let started = std::time::Instant::now();
    // W47: standalone datasource runs (workbench / studio) inherit the
    // app default language; no dashboard scope is available here.
    let language_directive =
        crate::commands::language::resolve_effective_language(state.storage.as_ref(), None, None)
            .await
            .ok()
            .and_then(|resolved| resolved.system_directive());
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        active_provider(state).await?,
    )
    .with_storage(state.storage.as_ref())
    .with_language(language_directive);
    let execution = engine.execute(&workflow, None).await?;
    let duration_ms = started.elapsed().as_millis() as u32;
    let run = execution.run;
    let node_results = run.node_results.clone();

    let pipeline_steps = def.pipeline.len() as u32;
    let raw_source = node_results
        .as_ref()
        .and_then(|results| results.get("source"))
        .cloned();

    if !matches!(run.status, RunStatus::Success) {
        return Ok(DatasourceRunResult {
            status: DatasourceHealthStatus::Error,
            duration_ms,
            error: run.error,
            raw_source,
            final_value: None,
            pipeline_steps,
            workflow_node_ids,
        });
    }
    let final_value = node_results
        .as_ref()
        .and_then(|results| results.get("output"))
        .and_then(|out| out.get("data"))
        .cloned();
    Ok(DatasourceRunResult {
        status: DatasourceHealthStatus::Ok,
        duration_ms,
        error: None,
        raw_source,
        final_value,
        pipeline_steps,
        workflow_node_ids,
    })
}

async fn upsert_health_from_run(
    state: &State<'_, AppState>,
    def_id: &str,
    consumer_count: u32,
    result: &DatasourceRunResult,
) -> AnyResult<()> {
    let sample_preview = result.final_value.as_ref().and_then(truncate_preview);
    let health = DatasourceHealth {
        last_run_at: chrono::Utc::now().timestamp_millis(),
        last_status: result.status,
        last_error: result.error.clone(),
        last_duration_ms: result.duration_ms,
        sample_preview,
        consumer_count,
    };
    state
        .storage
        .upsert_datasource_health(def_id, &health)
        .await?;
    Ok(())
}

async fn count_consumers_for(
    state: &State<'_, AppState>,
    def: &DatasourceDefinition,
) -> AnyResult<u32> {
    let dashboards = state.storage.list_dashboards().await?;
    let mut count = 0u32;
    for dashboard in &dashboards {
        for widget in &dashboard.layout {
            if let Some(config) = widget_datasource(widget) {
                if matches_definition(state, config, def).await? != MatchKind::None {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

// ─── Commands ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_datasource_definitions(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<DatasourceDefinition>>, String> {
    Ok(match state.storage.list_datasource_definitions().await {
        Ok(defs) => ApiResult::ok(defs),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_datasource_definition(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<DatasourceDefinition>, String> {
    Ok(match state.storage.get_datasource_definition(&id).await {
        Ok(Some(def)) => ApiResult::ok(def),
        Ok(None) => ApiResult::err("Datasource definition not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn create_datasource_definition(
    app: AppHandle,
    state: State<'_, AppState>,
    req: CreateDatasourceRequest,
) -> Result<ApiResult<DatasourceDefinition>, String> {
    Ok(match create_datasource_inner(&app, &state, req).await {
        Ok(def) => ApiResult::ok(def),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn create_datasource_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: CreateDatasourceRequest,
) -> AnyResult<DatasourceDefinition> {
    if req.name.trim().is_empty() {
        return Err(anyhow!("Datasource name is required"));
    }
    if matches!(req.kind, BuildDatasourcePlanKind::Shared) {
        return Err(anyhow!(
            "DatasourceDefinition cannot have kind='shared'; pick builtin_tool, mcp_tool, or provider_prompt"
        ));
    }
    if matches!(req.kind, BuildDatasourcePlanKind::Compose) {
        return Err(anyhow!(
            "DatasourceDefinition cannot have kind='compose'; compose plans live on widgets only"
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let def = DatasourceDefinition {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.trim().to_string(),
        description: req
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        kind: req.kind,
        tool_name: req.tool_name,
        server_id: req.server_id,
        arguments: req.arguments,
        prompt: req.prompt,
        pipeline: req.pipeline,
        refresh_cron: req.refresh_cron.filter(|s| !s.trim().is_empty()),
        workflow_id: uuid::Uuid::new_v4().to_string(),
        created_at: now,
        updated_at: now,
        health: None,
        originated_external_source_id: None,
    };
    let workflow = build_workflow_for_definition(&def, now)?;
    state.storage.create_workflow(&workflow).await?;
    schedule_workflow_if_cron(app, state, workflow).await?;
    state.storage.insert_datasource_definition(&def).await?;
    Ok(def)
}

#[tauri::command]
pub async fn update_datasource_definition(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    req: UpdateDatasourceRequest,
) -> Result<ApiResult<DatasourceDefinition>, String> {
    Ok(
        match update_datasource_inner(&app, &state, &id, req).await {
            Ok(def) => ApiResult::ok(def),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn update_datasource_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    req: UpdateDatasourceRequest,
) -> AnyResult<DatasourceDefinition> {
    let mut def = state
        .storage
        .get_datasource_definition(id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    if let Some(name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Datasource name cannot be empty"));
        }
        def.name = trimmed.to_string();
    }
    if let Some(description) = req.description {
        let trimmed = description.trim();
        def.description = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    if req.tool_name.is_some() {
        def.tool_name = req.tool_name;
    }
    if req.server_id.is_some() {
        def.server_id = req.server_id;
    }
    if req.arguments.is_some() {
        def.arguments = req.arguments;
    }
    if req.prompt.is_some() {
        def.prompt = req.prompt;
    }
    if let Some(pipeline) = req.pipeline {
        def.pipeline = pipeline;
    }
    if let Some(refresh_cron) = req.refresh_cron {
        def.refresh_cron = Some(refresh_cron).filter(|s| !s.trim().is_empty());
    }
    def.updated_at = chrono::Utc::now().timestamp_millis();

    // Rebuild backing workflow. We replace the existing workflow row in
    // place so consumer widgets keep their `workflow_id` bindings.
    let new_workflow = build_workflow_for_definition(&def, def.updated_at)?;
    let _ = state
        .scheduler
        .lock()
        .await
        .unschedule(&def.workflow_id)
        .await;
    let _ = state.storage.delete_workflow(&def.workflow_id).await;
    state.storage.create_workflow(&new_workflow).await?;
    schedule_workflow_if_cron(app, state, new_workflow).await?;

    state.storage.update_datasource_definition(&def).await?;
    Ok(def)
}

#[tauri::command]
pub async fn delete_datasource_definition(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match delete_datasource_inner(&app, &state, &id).await {
        Ok(removed) => ApiResult::ok(removed),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn delete_datasource_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
) -> AnyResult<bool> {
    let Some(def) = state.storage.get_datasource_definition(id).await? else {
        return Ok(false);
    };
    let count = count_consumers_for(state, &def).await?;
    if count > 0 {
        return Err(anyhow!(
            "Cannot delete datasource '{}': {} widget(s) still bound to it. Unbind or remove them first.",
            def.name,
            count
        ));
    }
    let _ = state
        .scheduler
        .lock()
        .await
        .unschedule(&def.workflow_id)
        .await;
    let _ = state.storage.delete_workflow(&def.workflow_id).await;
    let removed = state.storage.delete_datasource_definition(id).await?;
    let _ = app; // signature parity with other delete commands
    Ok(removed)
}

#[tauri::command]
pub async fn duplicate_datasource_definition(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<DatasourceDefinition>, String> {
    Ok(match duplicate_inner(&app, &state, &id).await {
        Ok(def) => ApiResult::ok(def),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn duplicate_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
) -> AnyResult<DatasourceDefinition> {
    let source = state
        .storage
        .get_datasource_definition(id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    let req = CreateDatasourceRequest {
        name: format!("{} (copy)", source.name),
        description: source.description,
        kind: source.kind,
        tool_name: source.tool_name,
        server_id: source.server_id,
        arguments: source.arguments,
        prompt: source.prompt,
        pipeline: source.pipeline,
        refresh_cron: source.refresh_cron,
    };
    create_datasource_inner(app, state, req).await
}

#[tauri::command]
pub async fn run_datasource_definition(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<DatasourceRunResult>, String> {
    Ok(match run_inner(&state, &id).await {
        Ok(result) => ApiResult::ok(result),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn run_inner(state: &State<'_, AppState>, id: &str) -> AnyResult<DatasourceRunResult> {
    let def = state
        .storage
        .get_datasource_definition(id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    let result = run_definition_once(state, &def).await?;
    let consumer_count = count_consumers_for(state, &def).await?;
    upsert_health_from_run(state, id, consumer_count, &result).await?;
    Ok(result)
}

#[tauri::command]
pub async fn list_datasource_consumers(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<Vec<DatasourceConsumer>>, String> {
    Ok(match list_consumers_inner(&state, &id).await {
        Ok(consumers) => ApiResult::ok(consumers),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn list_consumers_inner(
    state: &State<'_, AppState>,
    id: &str,
) -> AnyResult<Vec<DatasourceConsumer>> {
    let def = state
        .storage
        .get_datasource_definition(id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    Ok(scan_consumers(state, &def).await?)
}

async fn scan_consumers(
    state: &State<'_, AppState>,
    def: &DatasourceDefinition,
) -> AnyResult<Vec<DatasourceConsumer>> {
    let dashboards = state.storage.list_dashboards().await?;
    let mut consumers = Vec::new();
    for dashboard in dashboards {
        for widget in &dashboard.layout {
            if let Some(config) = widget_datasource(widget) {
                match matches_definition(state, config, def).await? {
                    MatchKind::Explicit => {
                        consumers.push(build_consumer(&dashboard, widget, config, true));
                    }
                    MatchKind::Legacy => {
                        consumers.push(build_consumer(&dashboard, widget, config, false));
                    }
                    MatchKind::None => {}
                }
            }
        }
    }
    Ok(consumers)
}

#[tauri::command]
pub async fn preview_datasource_impact(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<DatasourceImpactPreview>, String> {
    Ok(match preview_impact_inner(&state, &id).await {
        Ok(preview) => ApiResult::ok(preview),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn preview_impact_inner(
    state: &State<'_, AppState>,
    id: &str,
) -> AnyResult<DatasourceImpactPreview> {
    let def = state
        .storage
        .get_datasource_definition(id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    let consumers = scan_consumers(state, &def).await?;
    let legacy_consumer_count = consumers.iter().filter(|c| !c.explicit_binding).count() as u32;
    let has_explicit_consumers = consumers.iter().any(|c| c.explicit_binding);
    Ok(DatasourceImpactPreview {
        datasource_id: def.id.clone(),
        datasource_name: def.name.clone(),
        workflow_id: def.workflow_id.clone(),
        consumers,
        legacy_consumer_count,
        has_explicit_consumers,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindWidgetToDatasourceRequest {
    pub dashboard_id: String,
    pub widget_id: String,
    pub datasource_definition_id: String,
    /// Optional override for the per-widget output_key. Defaults to
    /// `output.data`, matching the single-consumer workflow shape we
    /// generate in `build_workflow_for_definition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_source: Option<DatasourceBindingSource>,
    /// W31.1: optional per-widget tail pipeline to apply after the
    /// saved datasource workflow output. When supplied, replaces the
    /// previous tail; pass an empty vector to clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_pipeline: Option<Vec<crate::models::pipeline::PipelineStep>>,
}

#[tauri::command]
pub async fn bind_widget_to_datasource(
    state: State<'_, AppState>,
    req: BindWidgetToDatasourceRequest,
) -> Result<ApiResult<DatasourceBindingChange>, String> {
    Ok(match bind_widget_inner(&state, req).await {
        Ok(change) => ApiResult::ok(change),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn bind_widget_inner(
    state: &State<'_, AppState>,
    req: BindWidgetToDatasourceRequest,
) -> AnyResult<DatasourceBindingChange> {
    let def = state
        .storage
        .get_datasource_definition(&req.datasource_definition_id)
        .await?
        .ok_or_else(|| anyhow!("Datasource definition not found"))?;
    let mut dashboard = state
        .storage
        .get_dashboard(&req.dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let now = chrono::Utc::now().timestamp_millis();
    let widget = dashboard
        .layout
        .iter_mut()
        .find(|w| w.id() == req.widget_id)
        .ok_or_else(|| anyhow!("Widget not found on dashboard"))?;

    let slot = widget_datasource_mut(widget)
        .ok_or_else(|| anyhow!("Widget kind does not support datasource bindings"))?;
    let previous_workflow_id = slot.as_ref().map(|c| c.workflow_id.clone());
    let previous_definition_id = slot
        .as_ref()
        .and_then(|c| c.datasource_definition_id.clone());
    let previous_post_process = slot.as_ref().and_then(|c| c.post_process.clone());
    let previous_capture = slot.as_ref().map(|c| c.capture_traces).unwrap_or(false);
    let previous_tail = slot
        .as_ref()
        .map(|c| c.tail_pipeline.clone())
        .unwrap_or_default();
    let output_key = req
        .output_key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "output.data".to_string());
    let binding_source = req
        .binding_source
        .unwrap_or(DatasourceBindingSource::Manual);

    *slot = Some(DatasourceConfig {
        workflow_id: def.workflow_id.clone(),
        output_key,
        post_process: previous_post_process,
        capture_traces: previous_capture,
        datasource_definition_id: Some(def.id.clone()),
        binding_source: Some(binding_source),
        bound_at: Some(now),
        tail_pipeline: req.tail_pipeline.unwrap_or(previous_tail),
        model_override: None,
    });
    dashboard.updated_at = now;
    state.storage.update_dashboard(&dashboard).await?;

    Ok(DatasourceBindingChange {
        dashboard_id: dashboard.id,
        widget_id: req.widget_id,
        datasource_definition_id: Some(def.id),
        workflow_id: Some(def.workflow_id),
        binding_source: Some(binding_source),
        previous_workflow_id,
        previous_datasource_definition_id: previous_definition_id,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbindWidgetFromDatasourceRequest {
    pub dashboard_id: String,
    pub widget_id: String,
    /// When `true`, drop the datasource binding entirely. When `false`
    /// (the default) keep the `workflow_id` so the widget still
    /// refreshes, but clear the explicit `datasource_definition_id` —
    /// the widget becomes a legacy consumer.
    #[serde(default)]
    pub drop_datasource: bool,
}

#[tauri::command]
pub async fn unbind_widget_from_datasource(
    state: State<'_, AppState>,
    req: UnbindWidgetFromDatasourceRequest,
) -> Result<ApiResult<DatasourceBindingChange>, String> {
    Ok(match unbind_widget_inner(&state, req).await {
        Ok(change) => ApiResult::ok(change),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn unbind_widget_inner(
    state: &State<'_, AppState>,
    req: UnbindWidgetFromDatasourceRequest,
) -> AnyResult<DatasourceBindingChange> {
    let mut dashboard = state
        .storage
        .get_dashboard(&req.dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let now = chrono::Utc::now().timestamp_millis();
    let widget = dashboard
        .layout
        .iter_mut()
        .find(|w| w.id() == req.widget_id)
        .ok_or_else(|| anyhow!("Widget not found on dashboard"))?;
    let slot = widget_datasource_mut(widget)
        .ok_or_else(|| anyhow!("Widget kind does not support datasource bindings"))?;
    let previous_workflow_id = slot.as_ref().map(|c| c.workflow_id.clone());
    let previous_definition_id = slot
        .as_ref()
        .and_then(|c| c.datasource_definition_id.clone());

    if req.drop_datasource {
        *slot = None;
    } else if let Some(config) = slot.as_mut() {
        config.datasource_definition_id = None;
        config.binding_source = Some(DatasourceBindingSource::Manual);
        config.bound_at = Some(now);
    }
    let (workflow_id, binding_source) = match slot.as_ref() {
        Some(c) => (Some(c.workflow_id.clone()), c.binding_source),
        None => (None, None),
    };
    dashboard.updated_at = now;
    state.storage.update_dashboard(&dashboard).await?;

    Ok(DatasourceBindingChange {
        dashboard_id: dashboard.id,
        widget_id: req.widget_id,
        datasource_definition_id: None,
        workflow_id,
        binding_source,
        previous_workflow_id,
        previous_datasource_definition_id: previous_definition_id,
    })
}

// ─── Local import / export ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportDatasourcesRequest {
    /// Optional id filter; empty / `None` exports the whole catalog.
    #[serde(default)]
    pub ids: Vec<String>,
}

#[tauri::command]
pub async fn export_datasource_definitions(
    state: State<'_, AppState>,
    req: ExportDatasourcesRequest,
) -> Result<ApiResult<DatasourceExportBundle>, String> {
    Ok(match export_inner(&state, &req).await {
        Ok(bundle) => ApiResult::ok(bundle),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn export_inner(
    state: &State<'_, AppState>,
    req: &ExportDatasourcesRequest,
) -> AnyResult<DatasourceExportBundle> {
    let mut defs = state.storage.list_datasource_definitions().await?;
    if !req.ids.is_empty() {
        defs.retain(|d| req.ids.iter().any(|wanted| wanted == &d.id));
    }
    // Drop health so the bundle is stable across exports — re-runs in
    // the destination profile will repopulate it.
    for def in defs.iter_mut() {
        def.health = None;
    }
    Ok(DatasourceExportBundle {
        version: EXPORT_BUNDLE_VERSION,
        exported_at: chrono::Utc::now().timestamp_millis(),
        definitions: defs,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDatasourcesRequest {
    pub bundle: DatasourceExportBundle,
    /// When true, an incoming definition with the same id replaces the
    /// existing row instead of being skipped. Off by default so import
    /// is non-destructive.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDatasourcesResult {
    pub imported: u32,
    pub skipped: u32,
    pub overwritten: u32,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn import_datasource_definitions(
    app: AppHandle,
    state: State<'_, AppState>,
    req: ImportDatasourcesRequest,
) -> Result<ApiResult<ImportDatasourcesResult>, String> {
    Ok(match import_inner(&app, &state, req).await {
        Ok(result) => ApiResult::ok(result),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn import_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: ImportDatasourcesRequest,
) -> AnyResult<ImportDatasourcesResult> {
    if req.bundle.version != EXPORT_BUNDLE_VERSION {
        return Err(anyhow!(
            "Unsupported datasource bundle version {} (this build understands v{})",
            req.bundle.version,
            EXPORT_BUNDLE_VERSION
        ));
    }
    let mut result = ImportDatasourcesResult {
        imported: 0,
        skipped: 0,
        overwritten: 0,
        errors: Vec::new(),
    };
    for mut def in req.bundle.definitions {
        match state.storage.get_datasource_definition(&def.id).await? {
            Some(_existing) if !req.overwrite => {
                result.skipped += 1;
                continue;
            }
            Some(_existing) => {
                def.updated_at = chrono::Utc::now().timestamp_millis();
                def.health = None;
                match build_workflow_for_definition(&def, def.updated_at) {
                    Ok(new_workflow) => {
                        let _ = state
                            .scheduler
                            .lock()
                            .await
                            .unschedule(&def.workflow_id)
                            .await;
                        let _ = state.storage.delete_workflow(&def.workflow_id).await;
                        if let Err(e) = state.storage.create_workflow(&new_workflow).await {
                            result
                                .errors
                                .push(format!("create_workflow failed for '{}': {}", def.name, e));
                            continue;
                        }
                        if let Err(e) = schedule_workflow_if_cron(app, state, new_workflow).await {
                            result
                                .errors
                                .push(format!("schedule failed for '{}': {}", def.name, e));
                        }
                        state.storage.update_datasource_definition(&def).await?;
                        result.overwritten += 1;
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("build_workflow failed for '{}': {}", def.name, e));
                    }
                }
            }
            None => {
                let now = chrono::Utc::now().timestamp_millis();
                def.created_at = now;
                def.updated_at = now;
                def.health = None;
                match build_workflow_for_definition(&def, now) {
                    Ok(workflow) => {
                        if let Err(e) = state.storage.create_workflow(&workflow).await {
                            result
                                .errors
                                .push(format!("create_workflow failed for '{}': {}", def.name, e));
                            continue;
                        }
                        if let Err(e) = schedule_workflow_if_cron(app, state, workflow).await {
                            result
                                .errors
                                .push(format!("schedule failed for '{}': {}", def.name, e));
                        }
                        state.storage.insert_datasource_definition(&def).await?;
                        result.imported += 1;
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("build_workflow failed for '{}': {}", def.name, e));
                    }
                }
            }
        }
    }
    Ok(result)
}

// ─── W37: external source → saved datasource bridge ────────────────────────

/// Persist a runnable external source as a `DatasourceDefinition` so the
/// existing workflow + binding machinery can consume it. The arguments
/// vector is captured verbatim into the `http_request` payload — the
/// LLM tool path stays parameter-aware, but a saved datasource is a
/// concrete request (no per-refresh substitution today). Sources whose
/// `credential_policy == Required` are rejected: storing a real API key
/// inside a workflow JSON row would leak it through normal export.
pub async fn create_datasource_definition_from_external_source(
    app: &AppHandle,
    state: &State<'_, AppState>,
    source: &ExternalSource,
    name: &str,
    arguments: &serde_json::Value,
    refresh_cron: Option<&str>,
) -> AnyResult<DatasourceDefinition> {
    if matches!(
        source.credential_policy,
        ExternalSourceCredentialPolicy::Required
    ) {
        return Err(anyhow!(
            "Source '{}' requires a credential — saving it as a datasource would leak the key into workflow JSON. Use it through chat instead.",
            source.id
        ));
    }
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(anyhow!("Datasource name is required"));
    }
    // Reuse the W37 substitution + headers builder so the saved request
    // shape exactly matches what the chat path would have produced.
    let (effective_url, headers, body) =
        crate::commands::external_source::build_http_call_for_save(source, arguments)?;
    // For sources with an optional credential, attach the catalog id so
    // the workflow engine can inject the header at run time from the
    // current `external_source_state` row — the credential itself never
    // touches the workflow JSON.
    let attach_credential_marker = source.http.credential_header.is_some()
        && !matches!(
            source.credential_policy,
            ExternalSourceCredentialPolicy::None
        );
    let mut http_args = serde_json::json!({
        "method": source.http.method,
        "url": effective_url,
        "headers": headers,
        "body": body,
    });
    if attach_credential_marker {
        if let Some(obj) = http_args.as_object_mut() {
            obj.insert(
                "_external_source_id".to_string(),
                serde_json::Value::String(source.id.clone()),
            );
        }
    }

    let create_req = CreateDatasourceRequest {
        name: trimmed_name.to_string(),
        description: Some(format!(
            "Saved from external source catalog: {} ({}).",
            source.display_name,
            source
                .attribution
                .as_deref()
                .unwrap_or("no attribution required")
        )),
        kind: BuildDatasourcePlanKind::BuiltinTool,
        tool_name: Some("http_request".to_string()),
        server_id: None,
        arguments: Some(http_args),
        prompt: None,
        pipeline: source.default_pipeline.clone(),
        refresh_cron: refresh_cron.map(|s| s.to_string()),
    };
    let mut def = create_datasource_inner(app, state, create_req).await?;
    def.originated_external_source_id = Some(source.id.clone());
    state.storage.update_datasource_definition(&def).await?;
    Ok(def)
}

/// W37: enumerate saved datasources that were originated from a given
/// external source. Used by the catalog UI to warn before disabling a
/// source that still has live consumers.
pub async fn list_datasources_originated_from(
    state: &State<'_, AppState>,
    source_id: &str,
) -> AnyResult<Vec<DatasourceDefinition>> {
    let all = state.storage.list_datasource_definitions().await?;
    Ok(all
        .into_iter()
        .filter(|def| {
            def.originated_external_source_id
                .as_deref()
                .map(|id| id == source_id)
                .unwrap_or(false)
        })
        .collect())
}
