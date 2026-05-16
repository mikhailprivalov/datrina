use anyhow::{anyhow, Result as AnyResult};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::models::dashboard::{
    AddWidgetRequest, ApplyBuildChangeRequest, ApplyBuildProposalRequest, BuildChangeAction,
    BuildDatasourcePlan, BuildDatasourcePlanKind, BuildWidgetProposal, BuildWidgetType,
    CreateDashboardRequest, CreateDashboardTemplate, Dashboard, DashboardWidgetType,
    UpdateDashboardRequest,
};
use crate::models::widget::{
    ChartConfig, ChartKind, ColumnFormat, DatasourceConfig, GaugeConfig, GaugeThreshold,
    ImageConfig, ImageFit, TableColumn, TableConfig, TextAlign, TextConfig, TextFormat, Widget,
};
use crate::models::workflow::{
    NodeKind, RunStatus, TriggerKind, Workflow, WorkflowEdge, WorkflowNode, WorkflowTrigger,
    WORKFLOW_EVENT_CHANNEL,
};
use crate::models::ApiResult;
use crate::modules::scheduler::ScheduledRuntime;
use crate::modules::workflow_engine::WorkflowEngine;
use crate::AppState;

#[tauri::command]
pub async fn list_dashboards(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<Dashboard>>, String> {
    Ok(match state.storage.list_dashboards().await {
        Ok(dashboards) => ApiResult::ok(dashboards),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_dashboard(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(match state.storage.get_dashboard(&id).await {
        Ok(Some(dashboard)) => ApiResult::ok(dashboard),
        Ok(None) => ApiResult::err("Dashboard not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn create_dashboard(
    state: State<'_, AppState>,
    req: CreateDashboardRequest,
) -> Result<ApiResult<Dashboard>, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let template = req.template.unwrap_or(CreateDashboardTemplate::Blank);
    let (layout, workflows) = match template {
        CreateDashboardTemplate::Blank => (vec![], vec![]),
        CreateDashboardTemplate::LocalMvp => local_mvp_slice(now),
    };

    let dashboard = Dashboard {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        description: req.description,
        layout,
        workflows,
        is_default: false,
        created_at: now,
        updated_at: now,
    };

    Ok(
        match persist_dashboard_with_workflows(&state, &dashboard).await {
            Ok(()) => {
                info!("📊 Created dashboard: {}", dashboard.name);
                ApiResult::ok(dashboard)
            }
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn update_dashboard(
    state: State<'_, AppState>,
    id: String,
    req: UpdateDashboardRequest,
) -> Result<ApiResult<Dashboard>, String> {
    let mut dashboard = match state.storage.get_dashboard(&id).await {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(ApiResult::err("Dashboard not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    if let Some(name) = req.name {
        dashboard.name = name;
    }
    if let Some(description) = req.description {
        dashboard.description = Some(description);
    }
    if let Some(layout) = req.layout {
        dashboard.layout = layout;
    }
    if let Some(workflows) = req.workflows {
        dashboard.workflows = workflows;
    }
    dashboard.updated_at = chrono::Utc::now().timestamp_millis();

    Ok(match state.storage.update_dashboard(&dashboard).await {
        Ok(()) => ApiResult::ok(dashboard),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn add_dashboard_widget(
    state: State<'_, AppState>,
    dashboard_id: String,
    req: AddWidgetRequest,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(
        match add_widget_to_dashboard(&state, &dashboard_id, req).await {
            Ok(dashboard) => ApiResult::ok(dashboard),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn apply_build_change(
    state: State<'_, AppState>,
    req: ApplyBuildChangeRequest,
) -> Result<ApiResult<Dashboard>, String> {
    let result = match req.action {
        BuildChangeAction::CreateLocalDashboard => {
            let name = req
                .title
                .unwrap_or_else(|| "AI Build Dashboard".to_string());
            create_dashboard(
                state,
                CreateDashboardRequest {
                    name,
                    description: Some(
                        "Created through an explicit build-chat apply command.".to_string(),
                    ),
                    template: Some(CreateDashboardTemplate::LocalMvp),
                },
            )
            .await
        }
        BuildChangeAction::AddTextWidget => {
            let dashboard_id = match req.dashboard_id {
                Some(id) => id,
                None => return Ok(ApiResult::err("dashboard_id is required".to_string())),
            };
            add_dashboard_widget(
                state,
                dashboard_id,
                AddWidgetRequest {
                    widget_type: DashboardWidgetType::Text,
                    title: req.title.unwrap_or_else(|| "Build note".to_string()),
                    content: req.content,
                    value: None,
                },
            )
            .await
        }
        BuildChangeAction::AddGaugeWidget => {
            let dashboard_id = match req.dashboard_id {
                Some(id) => id,
                None => return Ok(ApiResult::err("dashboard_id is required".to_string())),
            };
            add_dashboard_widget(
                state,
                dashboard_id,
                AddWidgetRequest {
                    widget_type: DashboardWidgetType::Gauge,
                    title: req.title.unwrap_or_else(|| "Build metric".to_string()),
                    content: None,
                    value: req.value,
                },
            )
            .await
        }
    };

    result
}

#[tauri::command]
pub async fn apply_build_proposal(
    app: AppHandle,
    state: State<'_, AppState>,
    req: ApplyBuildProposalRequest,
) -> Result<ApiResult<Dashboard>, String> {
    if !req.confirmed {
        return Ok(ApiResult::err(
            "Build proposal apply requires explicit confirmation".to_string(),
        ));
    }

    Ok(match apply_build_proposal_inner(&app, &state, req).await {
        Ok(dashboard) => ApiResult::ok(dashboard),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn delete_dashboard(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_dashboard(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn refresh_widget(
    app: AppHandle,
    state: State<'_, AppState>,
    dashboard_id: String,
    widget_id: String,
) -> Result<ApiResult<serde_json::Value>, String> {
    Ok(
        match refresh_widget_inner(app, &state, &dashboard_id, &widget_id).await {
            Ok(value) => ApiResult::ok(value),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WidgetDryRunResult {
    pub status: String,
    pub widget_runtime: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub pipeline_steps: u32,
    pub has_llm_step: bool,
    pub workflow_node_ids: Vec<String>,
}

#[tauri::command]
pub async fn dry_run_widget(
    state: State<'_, AppState>,
    proposal: crate::models::dashboard::BuildWidgetProposal,
    shared_datasources: Option<Vec<crate::models::dashboard::SharedDatasource>>,
) -> Result<ApiResult<WidgetDryRunResult>, String> {
    let resolved = match inline_shared_into_widget(proposal, shared_datasources.unwrap_or_default())
    {
        Ok(p) => p,
        Err(error) => {
            return Ok(ApiResult::ok(WidgetDryRunResult {
                status: "error".to_string(),
                widget_runtime: None,
                raw_output: None,
                error: Some(error.to_string()),
                duration_ms: 0,
                pipeline_steps: 0,
                has_llm_step: false,
                workflow_node_ids: Vec::new(),
            }));
        }
    };
    Ok(match dry_run_widget_inner(&state, &resolved).await {
        Ok(result) => ApiResult::ok(result),
        Err(error) => ApiResult::ok(WidgetDryRunResult {
            status: "error".to_string(),
            widget_runtime: None,
            raw_output: None,
            error: Some(error.to_string()),
            duration_ms: 0,
            pipeline_steps: 0,
            has_llm_step: false,
            workflow_node_ids: Vec::new(),
        }),
    })
}

/// If the widget's datasource_plan is kind='shared', resolve it against the
/// matching shared_datasource entry and inline the source + base pipeline
/// in front of the consumer pipeline. Returns a standalone equivalent that
/// dry_run can build a per-widget workflow from.
pub(crate) fn inline_shared_into_widget(
    mut proposal: crate::models::dashboard::BuildWidgetProposal,
    shared_datasources: Vec<crate::models::dashboard::SharedDatasource>,
) -> AnyResult<crate::models::dashboard::BuildWidgetProposal> {
    let needs_inline = proposal
        .datasource_plan
        .as_ref()
        .map(|p| matches!(p.kind, BuildDatasourcePlanKind::Shared))
        .unwrap_or(false);
    if !needs_inline {
        return Ok(proposal);
    }
    let plan = proposal.datasource_plan.as_ref().unwrap();
    let key = plan
        .source_key
        .as_deref()
        .ok_or_else(|| anyhow!("Shared datasource_plan requires source_key"))?;
    let shared = shared_datasources
        .into_iter()
        .find(|s| s.key == key)
        .ok_or_else(|| {
            anyhow!(
                "Shared source_key '{}' not provided to dry_run; pass shared_datasources alongside the widget proposal",
                key
            )
        })?;
    let mut combined_pipeline = shared.pipeline.clone();
    combined_pipeline.extend(plan.pipeline.clone());
    let inlined = BuildDatasourcePlan {
        kind: shared.kind.clone(),
        tool_name: shared.tool_name.clone(),
        server_id: shared.server_id.clone(),
        arguments: shared.arguments.clone(),
        prompt: shared.prompt.clone(),
        output_path: plan.output_path.clone(),
        refresh_cron: None,
        pipeline: combined_pipeline,
        source_key: None,
    };
    proposal.datasource_plan = Some(inlined);
    Ok(proposal)
}

async fn dry_run_widget_inner(
    state: &State<'_, AppState>,
    proposal: &crate::models::dashboard::BuildWidgetProposal,
) -> AnyResult<WidgetDryRunResult> {
    let now = chrono::Utc::now().timestamp_millis();
    let pipeline_steps = proposal
        .datasource_plan
        .as_ref()
        .map(|plan| plan.pipeline.len() as u32)
        .unwrap_or(0);
    let has_llm_step = proposal
        .datasource_plan
        .as_ref()
        .map(|plan| {
            plan.pipeline.iter().any(|step| {
                matches!(
                    step,
                    crate::models::pipeline::PipelineStep::LlmPostprocess { .. }
                )
            })
        })
        .unwrap_or(false);

    let (widget, workflow) = proposal_widget(proposal, 0, now)?;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow!("Widget has no datasource workflow"))?;
    let workflow_node_ids: Vec<String> = workflow.nodes.iter().map(|n| n.id.clone()).collect();

    reconnect_enabled_mcp_servers(state).await?;
    let started = std::time::Instant::now();
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        active_provider(state).await?,
    );
    let execution = engine.execute(&workflow, None).await?;
    let duration_ms = started.elapsed().as_millis() as u64;
    let run = execution.run;
    let node_results = run.node_results.clone();

    if !matches!(run.status, RunStatus::Success) {
        return Ok(WidgetDryRunResult {
            status: "error".to_string(),
            widget_runtime: None,
            raw_output: node_results,
            error: run.error,
            duration_ms,
            pipeline_steps,
            has_llm_step,
            workflow_node_ids,
        });
    }
    let node_results =
        node_results.ok_or_else(|| anyhow!("Datasource workflow returned no node results"))?;
    let output = extract_output(&node_results, &datasource.output_key)
        .ok_or_else(|| anyhow!("Workflow output '{}' not found", datasource.output_key))?
        .clone();
    let widget_runtime = widget_runtime_data(&widget, &output)?;
    Ok(WidgetDryRunResult {
        status: "ok".to_string(),
        widget_runtime: Some(widget_runtime),
        raw_output: Some(output),
        error: None,
        duration_ms,
        pipeline_steps,
        has_llm_step,
        workflow_node_ids,
    })
}

async fn persist_dashboard_with_workflows(
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
) -> AnyResult<()> {
    for workflow in &dashboard.workflows {
        state.storage.create_workflow(workflow).await?;
    }
    state.storage.create_dashboard(dashboard).await?;
    Ok(())
}

async fn add_widget_to_dashboard(
    state: &State<'_, AppState>,
    dashboard_id: &str,
    req: AddWidgetRequest,
) -> AnyResult<Dashboard> {
    let mut dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;

    let now = chrono::Utc::now().timestamp_millis();
    let next_y = dashboard
        .layout
        .iter()
        .map(|widget| widget_position_bottom(widget))
        .max()
        .unwrap_or(0);
    let (widget, workflow) = match req.widget_type {
        DashboardWidgetType::Text => local_text_widget(
            req.title,
            req.content
                .unwrap_or_else(|| "Build note saved locally.".to_string()),
            next_y,
            now,
        ),
        DashboardWidgetType::Gauge => {
            local_gauge_widget(req.title, req.value.unwrap_or(64.0), next_y, now)
        }
    };

    state.storage.create_workflow(&workflow).await?;
    dashboard.workflows.push(workflow);
    dashboard.layout.push(widget);
    dashboard.updated_at = now;
    state.storage.update_dashboard(&dashboard).await?;
    Ok(dashboard)
}

async fn apply_build_proposal_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: ApplyBuildProposalRequest,
) -> AnyResult<Dashboard> {
    if req.proposal.widgets.is_empty() && req.proposal.remove_widget_ids.is_empty() {
        return Err(anyhow!(
            "Build proposal contains no widget changes to apply"
        ));
    }

    let now = chrono::Utc::now().timestamp_millis();

    // Pre-process shared datasources: pre-assign workflow_ids and consumer
    // widget_ids so the fan-out workflow nodes can reference them.
    let shared_by_key: std::collections::HashMap<
        String,
        &crate::models::dashboard::SharedDatasource,
    > = req
        .proposal
        .shared_datasources
        .iter()
        .map(|s| (s.key.clone(), s))
        .collect();
    let mut consumers_by_key: std::collections::HashMap<String, Vec<usize>> = Default::default();
    for (idx, widget) in req.proposal.widgets.iter().enumerate() {
        if let Some(plan) = &widget.datasource_plan {
            if matches!(plan.kind, BuildDatasourcePlanKind::Shared) {
                let key = plan.source_key.as_deref().ok_or_else(|| {
                    anyhow!(
                        "Widget '{}' has datasource_plan.kind='shared' but no source_key",
                        widget.title
                    )
                })?;
                if !shared_by_key.contains_key(key) {
                    return Err(anyhow!(
                        "Widget '{}' references unknown shared source_key '{}' (declare it in proposal.shared_datasources)",
                        widget.title,
                        key
                    ));
                }
                consumers_by_key
                    .entry(key.to_string())
                    .or_default()
                    .push(idx);
            }
        }
    }
    let mut shared_workflow_ids: std::collections::HashMap<String, String> = Default::default();
    let mut prebuilt_widget_ids: std::collections::HashMap<usize, String> = Default::default();
    for key in consumers_by_key.keys() {
        shared_workflow_ids.insert(key.clone(), uuid::Uuid::new_v4().to_string());
    }
    for indices in consumers_by_key.values() {
        for &idx in indices {
            prebuilt_widget_ids.insert(idx, uuid::Uuid::new_v4().to_string());
        }
    }

    // Build the shared fan-out workflows up front. Each one combines the
    // shared source, optional shared pipeline, and a per-consumer tail
    // ending at `output_<widget_id>`. Cron is attached to the shared
    // workflow so a single tick refreshes every consumer.
    let mut shared_workflows: Vec<Workflow> = Vec::new();
    for (key, consumer_indices) in &consumers_by_key {
        let shared = shared_by_key.get(key.as_str()).copied().unwrap();
        let workflow_id = shared_workflow_ids.get(key).cloned().unwrap();
        let consumers: Vec<(String, &BuildWidgetProposal)> = consumer_indices
            .iter()
            .map(|&idx| {
                (
                    prebuilt_widget_ids.get(&idx).cloned().unwrap(),
                    &req.proposal.widgets[idx],
                )
            })
            .collect();
        let workflow = build_shared_fanout_workflow(&workflow_id, shared, &consumers, now)?;
        shared_workflows.push(workflow);
    }
    let mut dashboard = match req.dashboard_id.as_deref() {
        Some(id) => state
            .storage
            .get_dashboard(id)
            .await?
            .ok_or_else(|| anyhow!("Dashboard not found"))?,
        None => Dashboard {
            id: uuid::Uuid::new_v4().to_string(),
            name: req
                .proposal
                .dashboard_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| req.proposal.title.clone()),
            description: req
                .proposal
                .dashboard_description
                .clone()
                .or(req.proposal.summary.clone()),
            layout: vec![],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
        },
    };

    // Step 1: removals - drop widgets the agent explicitly asked to remove and
    // unschedule/delete their workflows.
    for remove_id in &req.proposal.remove_widget_ids {
        if let Some(index) = dashboard
            .layout
            .iter()
            .position(|widget| widget.id() == remove_id)
        {
            let removed = dashboard.layout.remove(index);
            if let Some(workflow_id) = removed_workflow_id(&removed) {
                drop_workflow(app, state, &mut dashboard, &workflow_id).await?;
            }
        }
    }

    // Step 2: replacements + appends. A proposal widget with replace_widget_id
    // overwrites the existing widget at the same slot (preserving x/y/w/h
    // unless the proposal supplies its own); without it, the widget is
    // appended. Widgets without explicit x/y get auto-packed row-first into
    // a 24-col grid starting below the current bottom row.
    let existing_bottom = dashboard
        .layout
        .iter()
        .map(widget_position_bottom)
        .max()
        .unwrap_or(0);

    let mut auto_cursor_x = 0_i32;
    let mut auto_cursor_y = existing_bottom;
    let mut auto_row_height = 0_i32;

    // Persist the shared fan-out workflows once so consumer widgets can
    // reference them. Cron triggers attached to these shared workflows
    // refresh every consumer with a single tick.
    for workflow in &shared_workflows {
        state.storage.create_workflow(workflow).await?;
        schedule_workflow_if_cron(app, state, workflow.clone()).await?;
        dashboard.workflows.push(workflow.clone());
    }

    for (widget_index, widget_proposal) in req.proposal.widgets.iter().enumerate() {
        let shared_consumer = widget_proposal
            .datasource_plan
            .as_ref()
            .filter(|p| matches!(p.kind, BuildDatasourcePlanKind::Shared))
            .and_then(|p| p.source_key.clone());
        if let Some(replace_id) = widget_proposal.replace_widget_id.as_deref() {
            if let Some(index) = dashboard.layout.iter().position(|w| w.id() == replace_id) {
                let existing = &dashboard.layout[index];
                let preserved = existing_position(existing);
                if let Some(workflow_id) = removed_workflow_id(existing) {
                    // Don't drop shared workflows when replacing a consumer
                    // widget - the same workflow still feeds other consumers.
                    if !shared_workflow_ids.values().any(|id| id == &workflow_id) {
                        drop_workflow(app, state, &mut dashboard, &workflow_id).await?;
                    }
                }
                let (mut widget, workflow_opt) = if let Some(key) = shared_consumer.as_ref() {
                    let shared_workflow_id = shared_workflow_ids.get(key).cloned().unwrap();
                    let widget_id = prebuilt_widget_ids
                        .get(&widget_index)
                        .cloned()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let datasource = DatasourceConfig {
                        workflow_id: shared_workflow_id,
                        output_key: format!("output_{}.data", widget_id),
                        post_process: None,
                    };
                    (
                        build_widget_shell(
                            widget_proposal,
                            preserved.y,
                            widget_id,
                            Some(datasource),
                        )?,
                        None,
                    )
                } else {
                    let (w, wf) = proposal_widget(widget_proposal, preserved.y, now)?;
                    (w, Some(wf))
                };
                if widget_proposal.x.is_none()
                    && widget_proposal.y.is_none()
                    && widget_proposal.w.is_none()
                    && widget_proposal.h.is_none()
                {
                    overwrite_widget_position(&mut widget, &preserved);
                }
                if let Some(workflow) = workflow_opt {
                    state.storage.create_workflow(&workflow).await?;
                    schedule_workflow_if_cron(app, state, workflow.clone()).await?;
                    dashboard.workflows.push(workflow);
                }
                dashboard.layout[index] = widget;
                continue;
            }
            // replace_widget_id pointed at a widget that no longer exists -
            // fall through to append it instead of failing.
        }
        let (mut widget, workflow_opt) = if let Some(key) = shared_consumer.as_ref() {
            let shared_workflow_id = shared_workflow_ids.get(key).cloned().unwrap();
            let widget_id = prebuilt_widget_ids
                .get(&widget_index)
                .cloned()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let datasource = DatasourceConfig {
                workflow_id: shared_workflow_id,
                output_key: format!("output_{}.data", widget_id),
                post_process: None,
            };
            (
                build_widget_shell(widget_proposal, auto_cursor_y, widget_id, Some(datasource))?,
                None,
            )
        } else {
            let (w, wf) = proposal_widget(widget_proposal, auto_cursor_y, now)?;
            (w, Some(wf))
        };
        // Always auto-pack newly added widgets row-first on the 12-col grid.
        // We ignore any explicit `x`/`y` the agent supplied because models
        // are unreliable at placement and consistently leave gaps. `w` and
        // `h` from the proposal ARE respected.
        {
            let pos = existing_position(&widget);
            let w = pos.w.clamp(1, GRID_COLS);
            if auto_cursor_x + w > GRID_COLS {
                auto_cursor_x = 0;
                auto_cursor_y += auto_row_height;
                auto_row_height = 0;
            }
            overwrite_widget_position(
                &mut widget,
                &WidgetPosition {
                    x: auto_cursor_x,
                    y: auto_cursor_y,
                    w,
                    h: pos.h.max(1),
                },
            );
            auto_cursor_x += w;
            auto_row_height = auto_row_height.max(pos.h.max(1));
        }
        if let Some(workflow) = workflow_opt {
            state.storage.create_workflow(&workflow).await?;
            schedule_workflow_if_cron(app, state, workflow.clone()).await?;
            dashboard.workflows.push(workflow);
        }
        dashboard.layout.push(widget);
    }
    dashboard.updated_at = now;

    if req.dashboard_id.is_some() {
        state.storage.update_dashboard(&dashboard).await?;
    } else {
        state.storage.create_dashboard(&dashboard).await?;
    }

    Ok(dashboard)
}

pub(crate) fn proposal_widget_public(
    proposal: &BuildWidgetProposal,
    default_y: i32,
    now: i64,
) -> AnyResult<(Widget, Workflow)> {
    proposal_widget(proposal, default_y, now)
}

pub(crate) fn extract_output_public<'a>(
    node_results: &'a serde_json::Value,
    output_key: &str,
) -> Option<&'a serde_json::Value> {
    extract_output(node_results, output_key)
}

pub(crate) fn widget_runtime_data_public(
    widget: &Widget,
    output: &serde_json::Value,
) -> AnyResult<serde_json::Value> {
    widget_runtime_data(widget, output)
}

fn proposal_widget(
    proposal: &BuildWidgetProposal,
    default_y: i32,
    now: i64,
) -> AnyResult<(Widget, Workflow)> {
    if proposal.title.trim().is_empty() {
        return Err(anyhow!("Build proposal widget title is required"));
    }
    let plan = proposal.datasource_plan.as_ref().ok_or_else(|| {
        anyhow!(
            "Build proposal widget '{}' must include an executable datasource_plan",
            proposal.title
        )
    })?;
    if matches!(plan.kind, BuildDatasourcePlanKind::Shared) {
        return Err(anyhow!(
            "Shared widget '{}' must be built via build_widget_shell with its shared datasource, not proposal_widget",
            proposal.title
        ));
    }
    let workflow_id = uuid::Uuid::new_v4().to_string();
    let widget_id = uuid::Uuid::new_v4().to_string();
    let workflow = datasource_plan_workflow(
        workflow_id.clone(),
        format!("Generated live datasource: {}", proposal.title),
        proposal,
        plan,
        now,
    )?;
    let datasource = Some(DatasourceConfig {
        workflow_id,
        output_key: "output.data".to_string(),
        post_process: None,
    });
    let widget = build_widget_shell(proposal, default_y, widget_id, datasource)?;
    Ok((widget, workflow))
}

fn build_widget_shell(
    proposal: &BuildWidgetProposal,
    default_y: i32,
    widget_id: String,
    datasource: Option<DatasourceConfig>,
) -> AnyResult<Widget> {
    if proposal.title.trim().is_empty() {
        return Err(anyhow!("Build proposal widget title is required"));
    }
    let x = proposal.x.unwrap_or(0);
    let y = proposal.y.unwrap_or(default_y);

    let widget = match proposal.widget_type {
        BuildWidgetType::Text => Widget::Text {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(6),
            h: proposal.h.unwrap_or(3),
            config: proposal_config(proposal).unwrap_or(TextConfig {
                format: TextFormat::Markdown,
                font_size: 14,
                color: None,
                align: TextAlign::Left,
            }),
            datasource,
        },
        BuildWidgetType::Gauge => Widget::Gauge {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(4),
            h: proposal.h.unwrap_or(4),
            config: proposal_config(proposal).unwrap_or(GaugeConfig {
                min: 0.0,
                max: 100.0,
                unit: None,
                thresholds: None,
                show_value: true,
            }),
            datasource,
        },
        BuildWidgetType::Table => Widget::Table {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(8),
            h: proposal.h.unwrap_or(5),
            config: proposal_config(proposal)
                .unwrap_or_else(|| table_config_from_data(&proposal.data)),
            datasource,
        },
        BuildWidgetType::Chart => Widget::Chart {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(8),
            h: proposal.h.unwrap_or(5),
            config: proposal_config(proposal).unwrap_or(ChartConfig {
                kind: ChartKind::Bar,
                x_axis: first_object_key(&proposal.data),
                y_axis: numeric_object_keys(&proposal.data),
                colors: None,
                stacked: false,
                show_legend: true,
            }),
            datasource,
            refresh_interval: None,
        },
        BuildWidgetType::Image => Widget::Image {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(6),
            h: proposal.h.unwrap_or(4),
            config: proposal_config(proposal).unwrap_or(ImageConfig {
                fit: ImageFit::Contain,
                border_radius: 4,
            }),
            datasource,
        },
        BuildWidgetType::Stat => Widget::Stat {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(3),
            h: proposal.h.unwrap_or(2),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::StatConfig {
                unit: None,
                prefix: None,
                suffix: None,
                decimals: None,
                color_mode: crate::models::widget::StatColorMode::Value,
                thresholds: None,
                show_sparkline: false,
                graph_mode: crate::models::widget::StatGraphMode::None,
                align: crate::models::widget::TextAlign::Center,
            }),
            datasource,
        },
        BuildWidgetType::Logs => Widget::Logs {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(12),
            h: proposal.h.unwrap_or(6),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::LogsConfig {
                max_entries: 200,
                show_timestamp: true,
                show_level: true,
                wrap: false,
                reverse: false,
            }),
            datasource,
        },
        BuildWidgetType::BarGauge => Widget::BarGauge {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(8),
            h: proposal.h.unwrap_or(5),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::BarGaugeConfig {
                orientation: crate::models::widget::BarGaugeOrientation::Horizontal,
                display_mode: crate::models::widget::BarGaugeDisplayMode::Gradient,
                show_value: true,
                min: Some(0.0),
                max: None,
                unit: None,
                thresholds: None,
            }),
            datasource,
        },
        BuildWidgetType::StatusGrid => Widget::StatusGrid {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(8),
            h: proposal.h.unwrap_or(4),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::StatusGridConfig {
                columns: 4,
                layout: crate::models::widget::StatusGridLayout::Grid,
                show_label: true,
                status_colors: None,
            }),
            datasource,
        },
        BuildWidgetType::Heatmap => Widget::Heatmap {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: proposal.w.unwrap_or(12),
            h: proposal.h.unwrap_or(6),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::HeatmapConfig {
                color_scheme: crate::models::widget::HeatmapColorScheme::Viridis,
                x_label: None,
                y_label: None,
                unit: None,
                show_legend: true,
                log_scale: false,
            }),
            datasource,
        },
    };

    Ok(widget)
}

fn proposal_config<T: serde::de::DeserializeOwned>(proposal: &BuildWidgetProposal) -> Option<T> {
    proposal
        .config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

/// Build a single fan-out workflow that runs the shared source + base
/// pipeline once, then branches into a per-consumer tail (`pipeline_<wid>`
/// optional + `output_<wid>`). Each consumer widget points at
/// `output_<wid>.data` via its DatasourceConfig.
fn build_shared_fanout_workflow(
    workflow_id: &str,
    shared: &crate::models::dashboard::SharedDatasource,
    consumers: &[(String, &BuildWidgetProposal)],
    now: i64,
) -> AnyResult<Workflow> {
    let virt_plan = BuildDatasourcePlan {
        kind: shared.kind.clone(),
        tool_name: shared.tool_name.clone(),
        server_id: shared.server_id.clone(),
        arguments: shared.arguments.clone(),
        prompt: shared.prompt.clone(),
        output_path: None,
        refresh_cron: None,
        pipeline: Vec::new(),
        source_key: None,
    };
    let (source_node, source_kind_label) = datasource_source_node(&virt_plan)?;
    let mut nodes = vec![source_node];
    let mut edges = Vec::new();
    let mut tail_node = "source".to_string();

    if !shared.pipeline.is_empty() {
        let id = "shared_pipeline".to_string();
        nodes.push(WorkflowNode {
            id: id.clone(),
            kind: NodeKind::Transform,
            label: format!("Shared pipeline ({} step(s))", shared.pipeline.len()),
            position: None,
            config: Some(json!({
                "input_key": tail_node,
                "transform": "pipeline",
                "steps": shared.pipeline,
            })),
        });
        edges.push(WorkflowEdge {
            id: format!("{}-to-{}", tail_node, id),
            source: tail_node.clone(),
            target: id.clone(),
            condition: None,
        });
        tail_node = id;
    }

    for (widget_id, proposal) in consumers {
        let consumer_plan = proposal.datasource_plan.as_ref();
        let mut consumer_tail = tail_node.clone();

        // If the consumer wants an output_path on top of the shared result,
        // apply it as a pick_path BEFORE its pipeline tail.
        let consumer_output_path = consumer_plan
            .and_then(|p| p.output_path.as_deref())
            .filter(|p| !p.trim().is_empty());
        if let Some(path) = consumer_output_path {
            let id = format!("pick_{}", widget_id);
            nodes.push(WorkflowNode {
                id: id.clone(),
                kind: NodeKind::Transform,
                label: format!("Pick path '{}' for {}", path, proposal.title),
                position: None,
                config: Some(json!({
                    "input_key": consumer_tail,
                    "transform": "pick_path",
                    "path": path
                })),
            });
            edges.push(WorkflowEdge {
                id: format!("{}-to-{}", consumer_tail, id),
                source: consumer_tail.clone(),
                target: id.clone(),
                condition: None,
            });
            consumer_tail = id;
        }

        let consumer_pipeline = consumer_plan
            .map(|p| p.pipeline.clone())
            .unwrap_or_default();
        if !consumer_pipeline.is_empty() {
            let id = format!("pipeline_{}", widget_id);
            nodes.push(WorkflowNode {
                id: id.clone(),
                kind: NodeKind::Transform,
                label: format!(
                    "Tail pipeline for {} ({} step(s))",
                    proposal.title,
                    consumer_pipeline.len()
                ),
                position: None,
                config: Some(json!({
                    "input_key": consumer_tail,
                    "transform": "pipeline",
                    "steps": consumer_pipeline,
                })),
            });
            edges.push(WorkflowEdge {
                id: format!("{}-to-{}", consumer_tail, id),
                source: consumer_tail.clone(),
                target: id.clone(),
                condition: None,
            });
            consumer_tail = id;
        }

        let output_id = format!("output_{}", widget_id);
        nodes.push(WorkflowNode {
            id: output_id.clone(),
            kind: NodeKind::Output,
            label: format!("Widget output: {}", proposal.title),
            position: None,
            config: Some(json!({
                "input_node": consumer_tail,
                "output_key": "data"
            })),
        });
        edges.push(WorkflowEdge {
            id: format!("{}-to-{}", consumer_tail, output_id),
            source: consumer_tail,
            target: output_id,
            condition: None,
        });
    }

    let trigger = shared
        .refresh_cron
        .as_deref()
        .filter(|cron| !cron.trim().is_empty())
        .and_then(|cron| match normalize_cron_expression(cron) {
            Some(normalized) => Some(WorkflowTrigger {
                kind: TriggerKind::Cron,
                config: Some(crate::models::workflow::TriggerConfig {
                    cron: Some(normalized),
                    event: None,
                }),
            }),
            None => {
                tracing::warn!(
                    "ignoring unparseable cron for shared datasource '{}': {}",
                    shared.key,
                    cron
                );
                None
            }
        })
        .unwrap_or(WorkflowTrigger {
            kind: TriggerKind::Manual,
            config: None,
        });

    let label = shared
        .label
        .clone()
        .unwrap_or_else(|| format!("shared:{}", shared.key));
    let description = format!(
        "Shared datasource '{}' fanned out to {} widget(s) via {}.",
        shared.key,
        consumers.len(),
        source_kind_label
    );
    Ok(Workflow {
        id: workflow_id.to_string(),
        name: format!("Shared datasource: {}", label),
        description: Some(description),
        nodes,
        edges,
        trigger,
        is_enabled: true,
        last_run: None,
        created_at: now,
        updated_at: now,
    })
}

fn datasource_plan_workflow(
    workflow_id: String,
    name: String,
    proposal: &BuildWidgetProposal,
    plan: &BuildDatasourcePlan,
    now: i64,
) -> AnyResult<Workflow> {
    let (source_node, source_kind_label) = datasource_source_node(plan)?;
    let output_path = plan
        .output_path
        .as_deref()
        .filter(|path| !path.trim().is_empty());

    let mut nodes = vec![source_node];
    let mut edges = Vec::new();
    let mut tail_node = "source".to_string();

    if let Some(path) = output_path {
        let id = "shape".to_string();
        nodes.push(WorkflowNode {
            id: id.clone(),
            kind: NodeKind::Transform,
            label: "Pick widget data from datasource result".to_string(),
            position: None,
            config: Some(json!({
                "input_key": tail_node,
                "transform": "pick_path",
                "path": path
            })),
        });
        edges.push(WorkflowEdge {
            id: format!("{}-to-{}", tail_node, id),
            source: tail_node.clone(),
            target: id.clone(),
            condition: None,
        });
        tail_node = id;
    }

    if !plan.pipeline.is_empty() {
        let id = "pipeline".to_string();
        nodes.push(WorkflowNode {
            id: id.clone(),
            kind: NodeKind::Transform,
            label: format!("Deterministic pipeline ({} step(s))", plan.pipeline.len()),
            position: None,
            config: Some(json!({
                "input_key": tail_node,
                "transform": "pipeline",
                "steps": plan.pipeline,
            })),
        });
        edges.push(WorkflowEdge {
            id: format!("{}-to-{}", tail_node, id),
            source: tail_node.clone(),
            target: id.clone(),
            condition: None,
        });
        tail_node = id;
    }

    nodes.push(WorkflowNode {
        id: "output".to_string(),
        kind: NodeKind::Output,
        label: "Widget output".to_string(),
        position: None,
        config: Some(json!({
            "input_node": tail_node,
            "output_key": "data"
        })),
    });
    edges.push(WorkflowEdge {
        id: format!("{}-to-output", tail_node),
        source: tail_node,
        target: "output".to_string(),
        condition: None,
    });

    let trigger = plan
        .refresh_cron
        .as_deref()
        .filter(|cron| !cron.trim().is_empty())
        .and_then(|cron| match normalize_cron_expression(cron) {
            Some(normalized) => Some(WorkflowTrigger {
                kind: TriggerKind::Cron,
                config: Some(crate::models::workflow::TriggerConfig {
                    cron: Some(normalized),
                    event: None,
                }),
            }),
            None => {
                tracing::warn!(
                    "ignoring unparseable cron expression '{}' for proposal widget, falling back to manual trigger",
                    cron
                );
                None
            }
        })
        .unwrap_or(WorkflowTrigger {
            kind: TriggerKind::Manual,
            config: None,
        });

    Ok(Workflow {
        id: workflow_id,
        name,
        description: Some(format!(
            "Live datasource workflow generated for '{}' through {}.",
            proposal.title, source_kind_label
        )),
        nodes,
        edges,
        trigger,
        is_enabled: true,
        last_run: None,
        created_at: now,
        updated_at: now,
    })
}

/// Normalize a cron expression into the 6-field form expected by
/// `tokio_cron_scheduler` (`<sec> <min> <hour> <day_of_month> <month>
/// <day_of_week>`). Accepts the 5-field POSIX form by prepending `0` for
/// seconds, and validates the result with the `cron` crate so unparseable
/// expressions surface as `None` instead of crashing the scheduler.
pub(crate) fn normalize_cron_expression(cron: &str) -> Option<String> {
    let trimmed = cron.trim();
    if trimmed.is_empty() {
        return None;
    }
    let field_count = trimmed.split_whitespace().count();
    let candidate = match field_count {
        5 => format!("0 {}", trimmed),
        6 | 7 => trimmed.to_string(),
        _ => return None,
    };
    use std::str::FromStr;
    cron::Schedule::from_str(&candidate).ok().map(|_| candidate)
}

fn datasource_source_node(plan: &BuildDatasourcePlan) -> AnyResult<(WorkflowNode, &'static str)> {
    match plan.kind {
        BuildDatasourcePlanKind::BuiltinTool => {
            let tool_name = plan
                .tool_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| anyhow!("builtin_tool datasource_plan requires tool_name"))?;
            Ok((
                WorkflowNode {
                    id: "source".to_string(),
                    kind: NodeKind::McpTool,
                    label: format!("Built-in tool: {}", tool_name),
                    position: None,
                    config: Some(json!({
                        "server_id": "builtin",
                        "tool_name": tool_name,
                        "arguments": plan.arguments.clone().unwrap_or_else(|| json!({}))
                    })),
                },
                "built-in ToolEngine execution",
            ))
        }
        BuildDatasourcePlanKind::McpTool => {
            let server_id = plan
                .server_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| anyhow!("mcp_tool datasource_plan requires server_id"))?;
            let tool_name = plan
                .tool_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| anyhow!("mcp_tool datasource_plan requires tool_name"))?;
            Ok((
                WorkflowNode {
                    id: "source".to_string(),
                    kind: NodeKind::McpTool,
                    label: format!("MCP tool: {}", tool_name),
                    position: None,
                    config: Some(json!({
                        "server_id": server_id,
                        "tool_name": tool_name,
                        "arguments": plan.arguments.clone().unwrap_or_else(|| json!({}))
                    })),
                },
                "stdio MCP tool execution",
            ))
        }
        BuildDatasourcePlanKind::ProviderPrompt => {
            let prompt = plan
                .prompt
                .as_deref()
                .filter(|prompt| !prompt.trim().is_empty())
                .ok_or_else(|| anyhow!("provider_prompt datasource_plan requires prompt"))?;
            Ok((
                WorkflowNode {
                    id: "source".to_string(),
                    kind: NodeKind::Llm,
                    label: "Provider datasource prompt".to_string(),
                    position: None,
                    config: Some(json!({ "prompt": prompt })),
                },
                "Rust-mediated provider execution",
            ))
        }
        BuildDatasourcePlanKind::Shared => Err(anyhow!(
            "Shared datasource_plan must be resolved at apply time, not handled as a workflow source node"
        )),
    }
}

fn table_config_from_data(data: &Value) -> TableConfig {
    let columns = data
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .map(|row| {
            row.keys()
                .map(|key| TableColumn {
                    key: key.clone(),
                    header: title_case(key),
                    width: None,
                    format: ColumnFormat::Text,
                    thresholds: None,
                    status_colors: None,
                    link_template: None,
                })
                .collect::<Vec<_>>()
        })
        .filter(|columns| !columns.is_empty())
        .unwrap_or_else(|| {
            vec![TableColumn {
                key: "value".to_string(),
                header: "Value".to_string(),
                width: None,
                format: ColumnFormat::Text,
                thresholds: None,
                status_colors: None,
                link_template: None,
            }]
        });

    TableConfig {
        columns,
        page_size: 10,
        sortable: true,
        filterable: false,
    }
}

fn first_object_key(data: &Value) -> Option<String> {
    data.as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .and_then(|row| row.keys().next().cloned())
}

fn numeric_object_keys(data: &Value) -> Option<Vec<String>> {
    let keys = data
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .map(|row| {
            row.iter()
                .filter_map(|(key, value)| value.as_f64().map(|_| key.clone()))
                .collect::<Vec<_>>()
        })
        .filter(|keys| !keys.is_empty());
    keys
}

fn title_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn refresh_widget_inner(
    app: AppHandle,
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_id: &str,
) -> AnyResult<Value> {
    let dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let widget = dashboard
        .layout
        .iter()
        .find(|widget| widget.id() == widget_id)
        .ok_or_else(|| anyhow!("Widget not found"))?;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow!("Widget has no datasource workflow"))?;

    if datasource
        .post_process
        .as_ref()
        .is_some_and(|steps| !steps.is_empty())
    {
        return Err(anyhow!(
            "Widget post_process steps are unavailable in the MVP vertical slice"
        ));
    }

    let workflow = match state.storage.get_workflow(&datasource.workflow_id).await? {
        Some(workflow) => workflow,
        None => dashboard
            .workflows
            .iter()
            .find(|workflow| workflow.id == datasource.workflow_id)
            .cloned()
            .ok_or_else(|| anyhow!("Datasource workflow not found"))?,
    };

    reconnect_enabled_mcp_servers(state).await?;
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        active_provider(state).await?,
    );
    let execution = engine.execute(&workflow, None).await?;
    let run = execution.run;

    state.storage.save_workflow_run(&workflow.id, &run).await?;
    state
        .storage
        .update_workflow_last_run(&workflow.id, &run)
        .await?;
    for event in execution.events {
        app.emit(WORKFLOW_EVENT_CHANNEL, event)?;
    }

    if !matches!(run.status, RunStatus::Success) {
        return Err(anyhow!(
            "Datasource workflow failed: {}",
            run.error
                .unwrap_or_else(|| "unknown workflow error".to_string())
        ));
    }

    let node_results = run
        .node_results
        .as_ref()
        .ok_or_else(|| anyhow!("Datasource workflow returned no node results"))?;
    let output = extract_output(node_results, &datasource.output_key)
        .ok_or_else(|| anyhow!("Workflow output '{}' not found", datasource.output_key))?;
    let data = widget_runtime_data(widget, output)?;

    Ok(json!({
        "status": "ok",
        "workflow_run_id": run.id,
        "data": data,
    }))
}

async fn active_provider(
    state: &State<'_, AppState>,
) -> AnyResult<Option<crate::models::provider::LLMProvider>> {
    let providers = state.storage.list_providers().await?;
    let active_provider_id = state
        .storage
        .get_config("active_provider_id")
        .await?
        .filter(|id| !id.trim().is_empty());
    Ok(active_provider_id
        .as_deref()
        .and_then(|id| {
            providers
                .iter()
                .find(|provider| provider.id == id && provider.is_enabled)
        })
        .or_else(|| providers.iter().find(|provider| provider.is_enabled))
        .cloned())
}

async fn reconnect_enabled_mcp_servers(state: &State<'_, AppState>) -> AnyResult<()> {
    let servers = state.storage.list_mcp_servers().await?;
    for server in servers.into_iter().filter(|server| server.is_enabled) {
        if state.mcp_manager.is_connected(&server.id).await {
            continue;
        }
        state.tool_engine.validate_mcp_server(&server)?;
        state.mcp_manager.connect(server).await?;
    }
    Ok(())
}

async fn schedule_workflow_if_cron(
    app: &AppHandle,
    state: &State<'_, AppState>,
    workflow: Workflow,
) -> AnyResult<()> {
    let raw_cron = match workflow
        .trigger
        .config
        .as_ref()
        .and_then(|config| config.cron.as_deref())
        .filter(|cron| !cron.trim().is_empty())
    {
        Some(cron) => cron.to_string(),
        None => return Ok(()),
    };
    let cron = match normalize_cron_expression(&raw_cron) {
        Some(value) => value,
        None => {
            tracing::warn!(
                "skipping scheduling for workflow '{}': cron '{}' is not parseable",
                workflow.id,
                raw_cron
            );
            return Ok(());
        }
    };
    let runtime = ScheduledRuntime {
        app: app.clone(),
        storage: state.storage.clone(),
        tool_engine: state.tool_engine.clone(),
        mcp_manager: state.mcp_manager.clone(),
        ai_engine: state.ai_engine.clone(),
        provider: active_provider(state).await?,
    };
    state
        .scheduler
        .lock()
        .await
        .schedule_cron(workflow, &cron, runtime)
        .await
}

const GRID_COLS: i32 = 12;

#[derive(Clone, Copy)]
struct WidgetPosition {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

fn existing_position(widget: &Widget) -> WidgetPosition {
    match widget {
        Widget::Chart { x, y, w, h, .. }
        | Widget::Text { x, y, w, h, .. }
        | Widget::Table { x, y, w, h, .. }
        | Widget::Image { x, y, w, h, .. }
        | Widget::Gauge { x, y, w, h, .. }
        | Widget::Stat { x, y, w, h, .. }
        | Widget::Logs { x, y, w, h, .. }
        | Widget::BarGauge { x, y, w, h, .. }
        | Widget::StatusGrid { x, y, w, h, .. }
        | Widget::Heatmap { x, y, w, h, .. } => WidgetPosition {
            x: *x,
            y: *y,
            w: *w,
            h: *h,
        },
    }
}

fn overwrite_widget_position(widget: &mut Widget, pos: &WidgetPosition) {
    match widget {
        Widget::Chart { x, y, w, h, .. }
        | Widget::Text { x, y, w, h, .. }
        | Widget::Table { x, y, w, h, .. }
        | Widget::Image { x, y, w, h, .. }
        | Widget::Gauge { x, y, w, h, .. }
        | Widget::Stat { x, y, w, h, .. }
        | Widget::Logs { x, y, w, h, .. }
        | Widget::BarGauge { x, y, w, h, .. }
        | Widget::StatusGrid { x, y, w, h, .. }
        | Widget::Heatmap { x, y, w, h, .. } => {
            *x = pos.x;
            *y = pos.y;
            *w = pos.w;
            *h = pos.h;
        }
    }
}

fn removed_workflow_id(widget: &Widget) -> Option<String> {
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
        | Widget::Heatmap { datasource, .. } => datasource.as_ref().map(|d| d.workflow_id.clone()),
    }
}

async fn drop_workflow(
    app: &AppHandle,
    state: &State<'_, AppState>,
    dashboard: &mut Dashboard,
    workflow_id: &str,
) -> AnyResult<()> {
    dashboard.workflows.retain(|w| w.id != workflow_id);
    let _ = state.scheduler.lock().await.unschedule(workflow_id).await;
    if let Err(err) = state.storage.delete_workflow(workflow_id).await {
        tracing::warn!(
            "failed to delete workflow {} while applying proposal: {}",
            workflow_id,
            err
        );
        let _ = app;
    }
    Ok(())
}

fn widget_position_bottom(widget: &Widget) -> i32 {
    match widget {
        Widget::Chart { y, h, .. }
        | Widget::Text { y, h, .. }
        | Widget::Table { y, h, .. }
        | Widget::Image { y, h, .. }
        | Widget::Gauge { y, h, .. }
        | Widget::Stat { y, h, .. }
        | Widget::Logs { y, h, .. }
        | Widget::BarGauge { y, h, .. }
        | Widget::StatusGrid { y, h, .. }
        | Widget::Heatmap { y, h, .. } => y + h,
    }
}

fn extract_output<'a>(node_results: &'a Value, output_key: &str) -> Option<&'a Value> {
    if let Some(value) = node_results.get(output_key) {
        return Some(value);
    }

    let mut current = node_results;
    let mut found_path = true;
    for segment in output_key.split('.') {
        match current.get(segment) {
            Some(next) => current = next,
            None => {
                found_path = false;
                break;
            }
        }
    }
    if found_path {
        return Some(current);
    }

    node_results
        .get("output")
        .and_then(|output| output.get(output_key))
}

fn widget_runtime_data(widget: &Widget, output: &Value) -> AnyResult<Value> {
    let normalized = normalize_datasource_output(output);
    match widget_runtime_data_strict(widget, &normalized) {
        Ok(value) => Ok(value),
        Err(error) => {
            let reason = error.to_string();
            tracing::warn!(
                "widget runtime parsing fallback for {} ({}): {}",
                widget.id(),
                widget_kind_for_log(widget),
                reason
            );
            Ok(serde_json::json!({
                "kind": "text",
                "content": format!(
                    "Widget runtime data did not match the expected shape for this widget type. The raw datasource output is shown below.\n\n_Parser error:_ `{reason}`\n\n```json\n{}\n```",
                    pretty_json(&normalized)
                ),
                "fallback": true,
                "error": reason,
            }))
        }
    }
}

fn widget_kind_for_log(widget: &Widget) -> &'static str {
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
    }
}

fn widget_runtime_data_strict(widget: &Widget, normalized: &Value) -> AnyResult<Value> {
    match widget {
        Widget::Gauge { .. } => {
            let value = normalized
                .as_f64()
                .or_else(|| normalized.get("value").and_then(Value::as_f64))
                .or_else(|| find_number(&normalized))
                .ok_or_else(|| anyhow!("Gauge workflow output must be a number"))?;
            Ok(json!({ "kind": "gauge", "value": value }))
        }
        Widget::Text { .. } => {
            let content = normalized
                .as_str()
                .map(ToString::to_string)
                .unwrap_or_else(|| pretty_json(&normalized));
            Ok(json!({ "kind": "text", "content": content }))
        }
        Widget::Table { .. } => {
            let rows = coerce_rows(&normalized)
                .ok_or_else(|| anyhow!("Table workflow output must be an array or object"))?;
            Ok(json!({ "kind": "table", "rows": rows }))
        }
        Widget::Chart { .. } => {
            let rows = coerce_rows(&normalized)
                .ok_or_else(|| anyhow!("Chart workflow output must be an array or object"))?;
            Ok(json!({ "kind": "chart", "rows": rows }))
        }
        Widget::Image { .. } => {
            let src = normalized
                .as_str()
                .or_else(|| normalized.get("src").and_then(Value::as_str))
                .ok_or_else(|| anyhow!("Image workflow output must be a string or src object"))?;
            Ok(json!({
                "kind": "image",
                "src": src,
                "alt": normalized.get("alt").and_then(Value::as_str),
            }))
        }
        Widget::Stat { .. } => {
            let value = stat_value(&normalized).ok_or_else(|| {
                anyhow!("Stat workflow output must contain a numeric or string value")
            })?;
            let delta = normalized.get("delta").cloned();
            let label = normalized
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_string);
            let sparkline = normalized.get("sparkline").cloned();
            Ok(json!({
                "kind": "stat",
                "value": value,
                "delta": delta,
                "label": label,
                "sparkline": sparkline,
            }))
        }
        Widget::Logs { .. } => {
            let entries = logs_entries(&normalized).ok_or_else(|| {
                anyhow!(
                    "Logs workflow output must be an array of entries or an object with 'entries'"
                )
            })?;
            Ok(json!({ "kind": "logs", "entries": entries }))
        }
        Widget::BarGauge { .. } => {
            let rows = bar_gauge_rows(&normalized).ok_or_else(|| {
                anyhow!("BarGauge workflow output must be an array of {{name, value}} rows")
            })?;
            Ok(json!({ "kind": "bar_gauge", "rows": rows }))
        }
        Widget::StatusGrid { .. } => {
            let items = status_grid_items(&normalized).ok_or_else(|| {
                anyhow!("StatusGrid workflow output must be an array of {{name, status}} items")
            })?;
            Ok(json!({ "kind": "status_grid", "items": items }))
        }
        Widget::Heatmap { .. } => {
            let cells = heatmap_cells(&normalized).ok_or_else(|| {
                anyhow!(
                    "Heatmap workflow output must be a matrix or an array of {{x, y, value}} cells"
                )
            })?;
            Ok(json!({ "kind": "heatmap", "cells": cells }))
        }
    }
}

fn logs_entries(value: &Value) -> Option<Vec<Value>> {
    let candidate = value
        .as_array()
        .cloned()
        .or_else(|| value.get("entries").and_then(Value::as_array).cloned())?;
    Some(
        candidate
            .into_iter()
            .map(|item| {
                if item.is_object() {
                    item
                } else if let Some(text) = item.as_str() {
                    json!({ "message": text })
                } else {
                    json!({ "message": item.to_string() })
                }
            })
            .collect(),
    )
}

fn bar_gauge_rows(value: &Value) -> Option<Vec<Value>> {
    let array = value
        .as_array()
        .cloned()
        .or_else(|| value.get("rows").and_then(Value::as_array).cloned())?;
    let mut rows = Vec::new();
    for item in array {
        let obj = item.as_object()?;
        let name = obj
            .get("name")
            .or_else(|| obj.get("label"))
            .or_else(|| obj.get("key"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let val = obj
            .get("value")
            .or_else(|| obj.get("v"))
            .or_else(|| obj.get("count"))
            .and_then(Value::as_f64);
        if let Some(v) = val {
            let max = obj.get("max").and_then(Value::as_f64);
            let mut row = json!({ "name": name, "value": v });
            if let Some(m) = max {
                row["max"] = json!(m);
            }
            rows.push(row);
        }
    }
    if rows.is_empty() {
        None
    } else {
        Some(rows)
    }
}

fn status_grid_items(value: &Value) -> Option<Vec<Value>> {
    let array = value
        .as_array()
        .cloned()
        .or_else(|| value.get("items").and_then(Value::as_array).cloned())?;
    let mut items = Vec::new();
    for item in array {
        let obj = item.as_object()?;
        let name = obj
            .get("name")
            .or_else(|| obj.get("label"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let status = obj
            .get("status")
            .or_else(|| obj.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let mut row = json!({ "name": name, "status": status });
        if let Some(detail) = obj.get("detail").or_else(|| obj.get("description")) {
            row["detail"] = detail.clone();
        }
        items.push(row);
    }
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn heatmap_cells(value: &Value) -> Option<Vec<Value>> {
    if let Some(array) = value.as_array() {
        let mut cells = Vec::new();
        for (yi, row) in array.iter().enumerate() {
            if let Some(row_arr) = row.as_array() {
                for (xi, v) in row_arr.iter().enumerate() {
                    if let Some(num) = v.as_f64() {
                        cells.push(json!({ "x": xi, "y": yi, "value": num }));
                    }
                }
            }
        }
        if !cells.is_empty() {
            return Some(cells);
        }
    }
    let array = value
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())?;
    let mut cells = Vec::new();
    for item in array {
        let obj = item.as_object()?;
        let x = obj.get("x").cloned().unwrap_or(json!(0));
        let y = obj.get("y").cloned().unwrap_or(json!(0));
        let v = obj
            .get("value")
            .or_else(|| obj.get("v"))
            .and_then(Value::as_f64)?;
        cells.push(json!({ "x": x, "y": y, "value": v }));
    }
    if cells.is_empty() {
        None
    } else {
        Some(cells)
    }
}

fn stat_value(value: &Value) -> Option<Value> {
    if value.is_number() || value.is_string() {
        return Some(value.clone());
    }
    if let Some(v) = value.get("value") {
        if v.is_number() || v.is_string() {
            return Some(v.clone());
        }
    }
    find_number(value).map(|n| Value::from(n))
}

fn normalize_datasource_output(output: &Value) -> Value {
    if let Some(unwrapped) = unwrap_mcp_content(output) {
        return unwrapped;
    }
    if let Some(text) = output.as_str() {
        return parse_json_or_string(text);
    }
    output.clone()
}

fn unwrap_mcp_content(value: &Value) -> Option<Value> {
    let content = value.get("content")?.as_array()?;
    let text_parts = content
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if text_parts.is_empty() {
        return None;
    }
    let text = text_parts.join("\n");
    Some(parse_json_or_string(&text))
}

fn parse_json_or_string(text: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

fn coerce_rows(value: &Value) -> Option<Vec<Value>> {
    if let Some(array) = value.as_array() {
        return Some(array.iter().map(row_value).collect());
    }

    if let Some(object) = value.as_object() {
        if let Some(array) = object.values().find_map(Value::as_array) {
            return Some(array.iter().map(row_value).collect());
        }
        return Some(vec![row_value(value)]);
    }

    Some(vec![json!({ "value": value.clone() })])
}

fn row_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let row = object
                .iter()
                .map(|(key, value)| (key.clone(), cell_value(value)))
                .collect::<serde_json::Map<_, _>>();
            Value::Object(row)
        }
        _ => json!({ "value": cell_value(value) }),
    }
}

fn cell_value(value: &Value) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
        _ => Value::String(pretty_json(value)),
    }
}

fn find_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Array(items) => items.iter().find_map(find_number),
        Value::Object(object) => object.values().find_map(find_number),
        _ => None,
    }
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub(crate) fn local_mvp_slice(now: i64) -> (Vec<Widget>, Vec<Workflow>) {
    let workflow_id = uuid::Uuid::new_v4().to_string();
    let widget_id = uuid::Uuid::new_v4().to_string();

    let workflow = Workflow {
        id: workflow_id.clone(),
        name: "Local MVP metric refresh".to_string(),
        description: Some(
            "Deterministic local datasource used by the MVP vertical slice.".to_string(),
        ),
        nodes: vec![
            WorkflowNode {
                id: "source".to_string(),
                kind: NodeKind::Datasource,
                label: "Local sample metric".to_string(),
                position: None,
                config: Some(json!({ "data": { "value": 72, "label": "Local health" } })),
            },
            WorkflowNode {
                id: "pick_value".to_string(),
                kind: NodeKind::Transform,
                label: "Pick metric value".to_string(),
                position: None,
                config: Some(json!({
                    "input_key": "source",
                    "transform": "pick",
                    "key": "value"
                })),
            },
            WorkflowNode {
                id: "output".to_string(),
                kind: NodeKind::Output,
                label: "Widget output".to_string(),
                position: None,
                config: Some(json!({
                    "input_node": "pick_value",
                    "output_key": "value"
                })),
            },
        ],
        edges: vec![
            WorkflowEdge {
                id: "source-to-pick".to_string(),
                source: "source".to_string(),
                target: "pick_value".to_string(),
                condition: None,
            },
            WorkflowEdge {
                id: "pick-to-output".to_string(),
                source: "pick_value".to_string(),
                target: "output".to_string(),
                condition: None,
            },
        ],
        trigger: WorkflowTrigger {
            kind: TriggerKind::Manual,
            config: None,
        },
        is_enabled: true,
        last_run: None,
        created_at: now,
        updated_at: now,
    };

    let widget = Widget::Gauge {
        id: widget_id,
        title: "Local MVP Metric".to_string(),
        x: 0,
        y: 0,
        w: 4,
        h: 4,
        config: GaugeConfig {
            min: 0.0,
            max: 100.0,
            unit: Some("%".to_string()),
            thresholds: Some(vec![
                GaugeThreshold {
                    value: 50.0,
                    color: "hsl(0 72% 50%)".to_string(),
                    label: Some("Low".to_string()),
                },
                GaugeThreshold {
                    value: 80.0,
                    color: "hsl(38 92% 50%)".to_string(),
                    label: Some("Good".to_string()),
                },
                GaugeThreshold {
                    value: 100.0,
                    color: "hsl(142 76% 36%)".to_string(),
                    label: Some("High".to_string()),
                },
            ]),
            show_value: true,
        },
        datasource: Some(DatasourceConfig {
            workflow_id,
            output_key: "output.value".to_string(),
            post_process: None,
        }),
    };

    (vec![widget], vec![workflow])
}

fn local_text_widget(title: String, content: String, y: i32, now: i64) -> (Widget, Workflow) {
    let workflow_id = uuid::Uuid::new_v4().to_string();
    let widget_id = uuid::Uuid::new_v4().to_string();
    let workflow = single_output_workflow(
        workflow_id.clone(),
        "Local text widget refresh".to_string(),
        json!(content),
        "content".to_string(),
        now,
    );

    let widget = Widget::Text {
        id: widget_id,
        title,
        x: 0,
        y,
        w: 6,
        h: 3,
        config: TextConfig {
            format: TextFormat::Markdown,
            font_size: 14,
            color: None,
            align: TextAlign::Left,
        },
        datasource: Some(DatasourceConfig {
            workflow_id,
            output_key: "output.content".to_string(),
            post_process: None,
        }),
    };

    (widget, workflow)
}

fn local_gauge_widget(title: String, value: f64, y: i32, now: i64) -> (Widget, Workflow) {
    let workflow_id = uuid::Uuid::new_v4().to_string();
    let widget_id = uuid::Uuid::new_v4().to_string();
    let workflow = single_output_workflow(
        workflow_id.clone(),
        "Local gauge widget refresh".to_string(),
        json!(value),
        "value".to_string(),
        now,
    );

    let widget = Widget::Gauge {
        id: widget_id,
        title,
        x: 0,
        y,
        w: 4,
        h: 4,
        config: GaugeConfig {
            min: 0.0,
            max: 100.0,
            unit: Some("%".to_string()),
            thresholds: Some(vec![
                GaugeThreshold {
                    value: 50.0,
                    color: "hsl(0 72% 50%)".to_string(),
                    label: Some("Low".to_string()),
                },
                GaugeThreshold {
                    value: 80.0,
                    color: "hsl(38 92% 50%)".to_string(),
                    label: Some("Good".to_string()),
                },
                GaugeThreshold {
                    value: 100.0,
                    color: "hsl(142 76% 36%)".to_string(),
                    label: Some("High".to_string()),
                },
            ]),
            show_value: true,
        },
        datasource: Some(DatasourceConfig {
            workflow_id,
            output_key: "output.value".to_string(),
            post_process: None,
        }),
    };

    (widget, workflow)
}

fn single_output_workflow(
    workflow_id: String,
    name: String,
    value: Value,
    output_key: String,
    now: i64,
) -> Workflow {
    Workflow {
        id: workflow_id,
        name,
        description: Some(
            "Deterministic local workflow created by an explicit apply command.".to_string(),
        ),
        nodes: vec![
            WorkflowNode {
                id: "source".to_string(),
                kind: NodeKind::Datasource,
                label: "Local applied value".to_string(),
                position: None,
                config: Some(json!({ "data": value })),
            },
            WorkflowNode {
                id: "output".to_string(),
                kind: NodeKind::Output,
                label: "Widget output".to_string(),
                position: None,
                config: Some(json!({
                    "input_node": "source",
                    "output_key": output_key
                })),
            },
        ],
        edges: vec![WorkflowEdge {
            id: "source-to-output".to_string(),
            source: "source".to_string(),
            target: "output".to_string(),
            condition: None,
        }],
        trigger: WorkflowTrigger {
            kind: TriggerKind::Manual,
            config: None,
        },
        is_enabled: true,
        last_run: None,
        created_at: now,
        updated_at: now,
    }
}

pub(crate) trait WidgetDatasource {
    fn datasource(&self) -> Option<&DatasourceConfig>;
}

impl WidgetDatasource for Widget {
    fn datasource(&self) -> Option<&DatasourceConfig> {
        match self {
            Widget::Chart { datasource, .. }
            | Widget::Text { datasource, .. }
            | Widget::Table { datasource, .. }
            | Widget::Image { datasource, .. }
            | Widget::Gauge { datasource, .. }
            | Widget::Stat { datasource, .. }
            | Widget::Logs { datasource, .. }
            | Widget::BarGauge { datasource, .. }
            | Widget::StatusGrid { datasource, .. }
            | Widget::Heatmap { datasource, .. } => datasource.as_ref(),
        }
    }
}
