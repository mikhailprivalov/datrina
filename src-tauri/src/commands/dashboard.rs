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
    if req.proposal.widgets.is_empty() {
        return Err(anyhow!("Build proposal contains no widgets to apply"));
    }

    let now = chrono::Utc::now().timestamp_millis();
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

    let mut next_y = dashboard
        .layout
        .iter()
        .map(widget_position_bottom)
        .max()
        .unwrap_or(0);

    for widget_proposal in &req.proposal.widgets {
        let (widget, workflow) = proposal_widget(widget_proposal, next_y, now)?;
        next_y = widget_position_bottom(&widget);
        state.storage.create_workflow(&workflow).await?;
        schedule_workflow_if_cron(app, state, workflow.clone()).await?;
        dashboard.workflows.push(workflow);
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

fn proposal_widget(
    proposal: &BuildWidgetProposal,
    default_y: i32,
    now: i64,
) -> AnyResult<(Widget, Workflow)> {
    if proposal.title.trim().is_empty() {
        return Err(anyhow!("Build proposal widget title is required"));
    }
    let workflow_id = uuid::Uuid::new_v4().to_string();
    let widget_id = uuid::Uuid::new_v4().to_string();
    let plan = proposal.datasource_plan.as_ref().ok_or_else(|| {
        anyhow!(
            "Build proposal widget '{}' must include an executable datasource_plan",
            proposal.title
        )
    })?;
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
    };

    Ok((widget, workflow))
}

fn proposal_config<T: serde::de::DeserializeOwned>(proposal: &BuildWidgetProposal) -> Option<T> {
    proposal
        .config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
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
    let output_input_node = if let Some(path) = output_path {
        nodes.push(WorkflowNode {
            id: "shape".to_string(),
            kind: NodeKind::Transform,
            label: "Pick widget data from datasource result".to_string(),
            position: None,
            config: Some(json!({
                "input_key": "source",
                "transform": "pick_path",
                "path": path
            })),
        });
        edges.push(WorkflowEdge {
            id: "source-to-shape".to_string(),
            source: "source".to_string(),
            target: "shape".to_string(),
            condition: None,
        });
        "shape"
    } else {
        "source"
    };

    nodes.push(WorkflowNode {
        id: "output".to_string(),
        kind: NodeKind::Output,
        label: "Widget output".to_string(),
        position: None,
        config: Some(json!({
            "input_node": output_input_node,
            "output_key": "data"
        })),
    });
    edges.push(WorkflowEdge {
        id: format!("{}-to-output", output_input_node),
        source: output_input_node.to_string(),
        target: "output".to_string(),
        condition: None,
    });

    let trigger = plan
        .refresh_cron
        .as_deref()
        .filter(|cron| !cron.trim().is_empty())
        .map(|cron| WorkflowTrigger {
            kind: TriggerKind::Cron,
            config: Some(crate::models::workflow::TriggerConfig {
                cron: Some(cron.to_string()),
                event: None,
            }),
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
    let cron = match workflow
        .trigger
        .config
        .as_ref()
        .and_then(|config| config.cron.as_deref())
        .filter(|cron| !cron.trim().is_empty())
    {
        Some(cron) => cron.to_string(),
        None => return Ok(()),
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

fn widget_position_bottom(widget: &Widget) -> i32 {
    match widget {
        Widget::Chart { y, h, .. }
        | Widget::Text { y, h, .. }
        | Widget::Table { y, h, .. }
        | Widget::Image { y, h, .. }
        | Widget::Gauge { y, h, .. } => y + h,
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
    }
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

fn local_mvp_slice(now: i64) -> (Vec<Widget>, Vec<Workflow>) {
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

trait WidgetDatasource {
    fn datasource(&self) -> Option<&DatasourceConfig>;
}

impl WidgetDatasource for Widget {
    fn datasource(&self) -> Option<&DatasourceConfig> {
        match self {
            Widget::Chart { datasource, .. }
            | Widget::Text { datasource, .. }
            | Widget::Table { datasource, .. }
            | Widget::Image { datasource, .. }
            | Widget::Gauge { datasource, .. } => datasource.as_ref(),
        }
    }
}
