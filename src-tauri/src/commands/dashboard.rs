use anyhow::{anyhow, Result as AnyResult};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_notification::NotificationExt;
use tracing::info;

use crate::models::alert::{AlertEvent, ALERT_EVENT_CHANNEL};
use crate::models::dashboard::{
    AddWidgetRequest, ApplyBuildChangeRequest, ApplyBuildProposalRequest, BuildChangeAction,
    BuildDatasourcePlan, BuildDatasourcePlanKind, BuildWidgetProposal, BuildWidgetType,
    CreateDashboardRequest, CreateDashboardTemplate, Dashboard, DashboardDiff, DashboardVersion,
    DashboardVersionSummary, DashboardWidgetType, JsonPathChange, UpdateDashboardRequest,
    VersionSource, WidgetDiff, WidgetSummary,
};
use crate::models::datasource::DatasourceDefinition;
use crate::models::snapshot::WidgetRuntimeSnapshot;
use crate::models::widget::{
    ChartConfig, ChartKind, ColumnFormat, DatasourceConfig, GaugeConfig, GaugeThreshold,
    ImageConfig, ImageFit, TableColumn, TableConfig, TextAlign, TextConfig, TextFormat, Widget,
};
use crate::models::widget_stream::{
    WidgetStreamEnvelope, WidgetStreamKind, WidgetStreamPayload, WIDGET_STREAM_EVENT_CHANNEL,
};
use crate::models::workflow::{
    NodeKind, RunStatus, TriggerKind, Workflow, WorkflowEdge, WorkflowNode, WorkflowTrigger,
    WORKFLOW_EVENT_CHANNEL,
};
use crate::models::ApiResult;
use crate::modules::alert_engine;
use crate::modules::parameter_engine::{self, ResolvedParameters, SubstituteOptions};
use crate::modules::scheduler::ScheduledRuntime;
use crate::modules::workflow_engine::WorkflowEngine;
use crate::{AppState, ReflectionPending};
use sha1::{Digest, Sha1};

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
        parameters: Vec::new(),
        model_policy: None,
        language_policy: None,
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
    let original = match state.storage.get_dashboard(&id).await {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(ApiResult::err("Dashboard not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let mut dashboard = original.clone();
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

    // W19: snapshot the pre-edit state before persisting. We only snapshot
    // when something actually changed (layout/name/description/workflows)
    // so non-mutating round-trips do not pollute history. updated_at is
    // ignored for that check.
    let summary = update_summary(&original, &dashboard);
    if let Some(summary) = summary {
        if let Err(error) = record_dashboard_version(
            &state,
            &original,
            VersionSource::ManualEdit,
            &summary,
            None,
            None,
        )
        .await
        {
            return Ok(ApiResult::err(error.to_string()));
        }
    }

    Ok(match state.storage.update_dashboard(&dashboard).await {
        Ok(()) => ApiResult::ok(dashboard),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

fn update_summary(before: &Dashboard, after: &Dashboard) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if before.name != after.name {
        parts.push("name".to_string());
    }
    if before.description != after.description {
        parts.push("description".to_string());
    }
    if before.layout.len() != after.layout.len() {
        parts.push(format!(
            "widgets ({} → {})",
            before.layout.len(),
            after.layout.len()
        ));
    } else if serde_json::to_value(&before.layout).ok() != serde_json::to_value(&after.layout).ok()
    {
        parts.push("layout".to_string());
    }
    if serde_json::to_value(&before.workflows).ok() != serde_json::to_value(&after.workflows).ok() {
        parts.push("workflows".to_string());
    }
    if before.model_policy != after.model_policy {
        parts.push("model_policy".to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("Manual edit: {}", parts.join(", ")))
    }
}

/// W43: write the dashboard-level default LLM policy. Snapshots the
/// pre-change dashboard so the change shows up in W19 version history,
/// then validates the new policy by running the resolution helper —
/// surface typed errors immediately so the user does not discover them
/// at refresh time.
#[tauri::command]
pub async fn set_dashboard_model_policy(
    state: State<'_, AppState>,
    req: crate::models::dashboard::SetDashboardModelPolicyRequest,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(match set_dashboard_model_policy_inner(&state, req).await {
        Ok(dashboard) => ApiResult::ok(dashboard),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn set_dashboard_model_policy_inner(
    state: &State<'_, AppState>,
    req: crate::models::dashboard::SetDashboardModelPolicyRequest,
) -> AnyResult<Dashboard> {
    let original = state
        .storage
        .get_dashboard(&req.dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;

    if let Some(policy) = req.policy.as_ref() {
        let providers = state.storage.list_providers().await?;
        crate::modules::ai::resolve_effective_widget_model(
            // Use any widget (or a synthetic) — capability check is
            // determined by the policy, not the widget shape. When the
            // dashboard has no widgets yet we still want to validate.
            original.layout.first().unwrap_or_else(|| {
                // Borrow a `&Widget` from a static placeholder by
                // pretending; the resolver only reads the override slot
                // which a fresh widget does not have. Construct a tiny
                // text widget on the fly via `serde_json::from_value`
                // to keep this synchronous and infallible.
                static EMPTY: once_cell::sync::Lazy<Widget> = once_cell::sync::Lazy::new(|| {
                    serde_json::from_value(serde_json::json!({
                        "type": "text",
                        "id": "policy-probe",
                        "title": "policy-probe",
                        "x": 0, "y": 0, "w": 1, "h": 1,
                        "config": {}
                    }))
                    .expect("static probe widget literal must parse")
                });
                &*EMPTY
            }),
            &Dashboard {
                model_policy: Some(policy.clone()),
                ..original.clone()
            },
            &providers,
            None,
        )
        .map_err(|error| anyhow!(error.to_string()))?;
    }

    let mut next = original.clone();
    next.model_policy = req.policy;
    next.updated_at = chrono::Utc::now().timestamp_millis();

    if let Some(summary) = update_summary(&original, &next) {
        record_dashboard_version(
            state,
            &original,
            VersionSource::ManualEdit,
            &summary,
            None,
            None,
        )
        .await?;
    }

    state.storage.update_dashboard(&next).await?;
    Ok(next)
}

/// W43: write a single widget's LLM override. Validates the policy
/// against the live provider list so a bad override surfaces immediately
/// (provider missing, disabled, mis-configured, or model lacks a
/// required cap). On success the dashboard is snapshotted via W19 so
/// the override shows up in version diffs alongside other widget edits.
#[tauri::command]
pub async fn set_widget_model_override(
    state: State<'_, AppState>,
    req: crate::models::dashboard::SetWidgetModelOverrideRequest,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(match set_widget_model_override_inner(&state, req).await {
        Ok(dashboard) => ApiResult::ok(dashboard),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn set_widget_model_override_inner(
    state: &State<'_, AppState>,
    req: crate::models::dashboard::SetWidgetModelOverrideRequest,
) -> AnyResult<Dashboard> {
    let original = state
        .storage
        .get_dashboard(&req.dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;

    let widget_index = original
        .layout
        .iter()
        .position(|w| w.id() == req.widget_id)
        .ok_or_else(|| anyhow!("Widget not found"))?;
    let widget = &original.layout[widget_index];
    let datasource = widget_datasource_ref(widget)
        .ok_or_else(|| {
            anyhow!("Widget has no datasource; model override is only valid for LLM-backed widgets")
        })?
        .clone();

    if let Some(policy) = req.policy.as_ref() {
        // Validate against the live provider list. The widget kind and
        // pipeline shape do not influence the resolver — only the
        // policy/provider/model triple does.
        let providers = state.storage.list_providers().await?;
        let probe_widget = clone_widget_with_override(widget, Some(policy.clone()));
        crate::modules::ai::resolve_effective_widget_model(
            &probe_widget,
            &original,
            &providers,
            None,
        )
        .map_err(|error| anyhow!(error.to_string()))?;
    }

    let mut next = original.clone();
    let new_datasource = DatasourceConfig {
        model_override: req.policy,
        ..datasource
    };
    let widget = &mut next.layout[widget_index];
    set_widget_datasource(widget, Some(new_datasource));
    next.updated_at = chrono::Utc::now().timestamp_millis();

    let summary = format!("Manual edit: widget {} model override", req.widget_id);
    record_dashboard_version(
        state,
        &original,
        VersionSource::ManualEdit,
        &summary,
        None,
        None,
    )
    .await?;
    state.storage.update_dashboard(&next).await?;
    Ok(next)
}

fn widget_datasource_ref(widget: &Widget) -> Option<&DatasourceConfig> {
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
        | Widget::Gallery { datasource, .. } => datasource.as_ref(),
    }
}

fn set_widget_datasource(widget: &mut Widget, value: Option<DatasourceConfig>) {
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
        | Widget::Gallery { datasource, .. } => *datasource = value,
    }
}

fn clone_widget_with_override(
    widget: &Widget,
    override_policy: Option<crate::models::widget::WidgetModelOverride>,
) -> Widget {
    let mut clone = widget.clone();
    if let Some(ds) = widget_datasource_ref(&clone).cloned() {
        set_widget_datasource(
            &mut clone,
            Some(DatasourceConfig {
                model_override: override_policy,
                ..ds
            }),
        );
    }
    clone
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

    // W29: server-side validation gate. Even if a stale UI manages to
    // surface an Apply button for a proposal whose validator already
    // failed, the apply command refuses the request rather than
    // silently mutating the dashboard. Frontend-side gating is the
    // primary UX, but the backend must fail closed too.
    let session_messages = match req.session_id.as_deref() {
        Some(session_id) => state
            .storage
            .get_chat_session(session_id)
            .await
            .ok()
            .flatten()
            .map(|session| session.messages)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let dashboard_for_validation = match req.dashboard_id.as_deref() {
        Some(dashboard_id) => state
            .storage
            .get_dashboard(dashboard_id)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    // W38: apply-time re-validation is intentionally unscoped (no target
    // widget ids). By the time the user clicks Apply, the chat-time
    // mention scope is already gone, and an explicit Apply means the
    // operator accepts whatever the proposal touches.
    let validation_issues = crate::commands::validation::validate_build_proposal(
        &req.proposal,
        dashboard_for_validation.as_ref(),
        &session_messages,
        None,
    );
    if !validation_issues.is_empty() {
        let summaries = validation_issues
            .iter()
            .map(crate::models::validation::ValidationIssue::summary)
            .collect::<Vec<_>>()
            .join("; ");
        return Ok(ApiResult::err(format!(
            "proposal_validation_failed: apply blocked by {} unresolved validation issue(s) — {}",
            validation_issues.len(),
            summaries
        )));
    }

    Ok(match apply_build_proposal_inner(&app, &state, req).await {
        Ok(dashboard) => ApiResult::ok(dashboard),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

/// W39: per-source resolution surfaced to the proposal preview before
/// the user clicks Apply. The shape mirrors the apply path's
/// materialization decisions exactly so the preview cannot drift away
/// from what apply actually does.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProposalMaterializationPreview {
    pub creates: Vec<MaterializationEntry>,
    pub reuses: Vec<MaterializationEntry>,
    pub rejects: Vec<MaterializationReject>,
    /// Inline plans we skipped (compose, output_path, inputs) — they
    /// still apply via the legacy per-widget workflow path, just not as
    /// saved catalog entries. Listed so the user sees them in the
    /// preview rather than wondering where they went.
    pub passthrough: Vec<MaterializationEntry>,
    pub total_widgets: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaterializationEntry {
    pub widget_title: String,
    pub source_kind: String,
    pub label: String,
    /// `widget` for inline plans, `shared` for `shared_datasources`
    /// entries, `passthrough` for inline plans we intentionally don't
    /// auto-materialize.
    pub origin: String,
    /// For reuses, the saved DatasourceDefinition id. For creates, the
    /// new id reserved for the upcoming apply (stable for the lifetime
    /// of the preview call).
    pub datasource_definition_id: Option<String>,
    /// For shared reuses, the existing workflow id; for new
    /// materialization, the workflow id reserved for the create.
    pub workflow_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MaterializationReject {
    pub widget_title: String,
    pub source_kind: String,
    pub origin: String,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreviewProposalMaterializationRequest {
    pub proposal: crate::models::dashboard::BuildProposal,
}

#[tauri::command]
pub async fn preview_proposal_materialization(
    state: State<'_, AppState>,
    req: PreviewProposalMaterializationRequest,
) -> Result<ApiResult<ProposalMaterializationPreview>, String> {
    Ok(
        match preview_proposal_materialization_inner(&state, req).await {
            Ok(preview) => ApiResult::ok(preview),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn preview_proposal_materialization_inner(
    state: &State<'_, AppState>,
    req: PreviewProposalMaterializationRequest,
) -> AnyResult<ProposalMaterializationPreview> {
    use crate::modules::datasource_signature::DatasourceSignature;

    let mut creates: Vec<MaterializationEntry> = Vec::new();
    let mut reuses: Vec<MaterializationEntry> = Vec::new();
    let mut rejects: Vec<MaterializationReject> = Vec::new();
    let mut passthrough: Vec<MaterializationEntry> = Vec::new();

    let saved_defs = state.storage.list_datasource_definitions().await?;
    let mut sig_to_def: std::collections::HashMap<DatasourceSignature, (String, String)> =
        Default::default();
    for def in &saved_defs {
        if let Some(sig) = DatasourceSignature::from_definition(def) {
            sig_to_def
                .entry(sig)
                .or_insert_with(|| (def.id.clone(), def.workflow_id.clone()));
        }
    }

    // Shared sources: walk shared_datasources first.
    for shared in &req.proposal.shared_datasources {
        let label = source_label_for_shared(shared);
        if matches!(shared.kind, BuildDatasourcePlanKind::BuiltinTool)
            && shared.tool_name.as_deref() == Some("http_request")
        {
            if let Some(args) = shared.arguments.as_ref() {
                if let Err(e) = crate::modules::tool_engine::validate_http_request_arguments(args) {
                    rejects.push(MaterializationReject {
                        widget_title: shared.key.clone(),
                        source_kind: "http_request".to_string(),
                        origin: "shared".to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            } else {
                rejects.push(MaterializationReject {
                    widget_title: shared.key.clone(),
                    source_kind: "http_request".to_string(),
                    origin: "shared".to_string(),
                    reason: "missing arguments object".to_string(),
                });
                continue;
            }
        }
        let entry = MaterializationEntry {
            widget_title: shared.key.clone(),
            source_kind: shared_kind_label(&shared.kind).to_string(),
            label,
            origin: "shared".to_string(),
            datasource_definition_id: None,
            workflow_id: None,
        };
        if let Some(sig) = DatasourceSignature::from_shared(shared) {
            if let Some((def_id, workflow_id)) = sig_to_def.get(&sig) {
                reuses.push(MaterializationEntry {
                    datasource_definition_id: Some(def_id.clone()),
                    workflow_id: Some(workflow_id.clone()),
                    ..entry
                });
                continue;
            }
            // Reserve a placeholder identity so preview entries can be
            // referenced by the same id across renders.
            let new_def_id = uuid::Uuid::new_v4().to_string();
            let new_workflow_id = uuid::Uuid::new_v4().to_string();
            sig_to_def.insert(sig, (new_def_id.clone(), new_workflow_id.clone()));
            creates.push(MaterializationEntry {
                datasource_definition_id: Some(new_def_id),
                workflow_id: Some(new_workflow_id),
                ..entry
            });
        } else {
            // Shared key with a non-materializable kind (shouldn't
            // happen, but surface it transparently).
            passthrough.push(entry);
        }
    }

    // Widget inline plans.
    for widget in &req.proposal.widgets {
        let Some(plan) = widget.datasource_plan.as_ref() else {
            continue;
        };
        let label = source_label_for_plan(plan);
        let kind_label = shared_kind_label(&plan.kind);
        match plan.kind {
            BuildDatasourcePlanKind::Shared => {
                // Already accounted for via shared_datasources above.
                continue;
            }
            BuildDatasourcePlanKind::Compose => {
                passthrough.push(MaterializationEntry {
                    widget_title: widget.title.clone(),
                    source_kind: "compose".to_string(),
                    label: label.clone(),
                    origin: "compose".to_string(),
                    datasource_definition_id: None,
                    workflow_id: None,
                });
                // W48: walk inner compose plans and surface a per-input
                // reuse entry whenever the inner plan's signature matches
                // a saved DatasourceDefinition. The compose workflow still
                // executes the inputs inline today, but operators get a
                // truthful "this input maps to the saved Forecast source"
                // link in the preview surface.
                if let Some(inputs) = plan.inputs.as_ref() {
                    for (alias, inner) in inputs.iter() {
                        let inner_label = source_label_for_plan(inner);
                        let inner_kind = shared_kind_label(&inner.kind).to_string();
                        if let Some(sig) = DatasourceSignature::from_inline_plan(inner) {
                            if let Some((def_id, workflow_id)) = sig_to_def.get(&sig) {
                                reuses.push(MaterializationEntry {
                                    widget_title: widget.title.clone(),
                                    source_kind: inner_kind,
                                    label: format!("compose[{}]: {}", alias, inner_label),
                                    origin: format!("compose:{}", alias),
                                    datasource_definition_id: Some(def_id.clone()),
                                    workflow_id: Some(workflow_id.clone()),
                                });
                            }
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        let has_output_path = plan
            .output_path
            .as_deref()
            .map(|p| !p.trim().is_empty())
            .unwrap_or(false);
        let has_inputs = plan.inputs.as_ref().is_some_and(|m| !m.is_empty());
        if has_output_path || has_inputs {
            passthrough.push(MaterializationEntry {
                widget_title: widget.title.clone(),
                source_kind: kind_label.to_string(),
                label,
                origin: "passthrough".to_string(),
                datasource_definition_id: None,
                workflow_id: None,
            });
            continue;
        }
        if matches!(plan.kind, BuildDatasourcePlanKind::BuiltinTool)
            && plan.tool_name.as_deref() == Some("http_request")
        {
            if let Some(args) = plan.arguments.as_ref() {
                if let Err(e) = crate::modules::tool_engine::validate_http_request_arguments(args) {
                    rejects.push(MaterializationReject {
                        widget_title: widget.title.clone(),
                        source_kind: "http_request".to_string(),
                        origin: "widget".to_string(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            } else {
                rejects.push(MaterializationReject {
                    widget_title: widget.title.clone(),
                    source_kind: "http_request".to_string(),
                    origin: "widget".to_string(),
                    reason: "missing arguments object".to_string(),
                });
                continue;
            }
        }
        let Some(sig) = DatasourceSignature::from_inline_plan(plan) else {
            passthrough.push(MaterializationEntry {
                widget_title: widget.title.clone(),
                source_kind: kind_label.to_string(),
                label,
                origin: "passthrough".to_string(),
                datasource_definition_id: None,
                workflow_id: None,
            });
            continue;
        };
        let entry = MaterializationEntry {
            widget_title: widget.title.clone(),
            source_kind: kind_label.to_string(),
            label,
            origin: "widget".to_string(),
            datasource_definition_id: None,
            workflow_id: None,
        };
        if let Some((def_id, workflow_id)) = sig_to_def.get(&sig) {
            reuses.push(MaterializationEntry {
                datasource_definition_id: Some(def_id.clone()),
                workflow_id: Some(workflow_id.clone()),
                ..entry
            });
            continue;
        }
        let new_def_id = uuid::Uuid::new_v4().to_string();
        let new_workflow_id = uuid::Uuid::new_v4().to_string();
        sig_to_def.insert(sig, (new_def_id.clone(), new_workflow_id.clone()));
        creates.push(MaterializationEntry {
            datasource_definition_id: Some(new_def_id),
            workflow_id: Some(new_workflow_id),
            ..entry
        });
    }
    Ok(ProposalMaterializationPreview {
        creates,
        reuses,
        rejects,
        passthrough,
        total_widgets: req.proposal.widgets.len() as u32,
    })
}

fn shared_kind_label(kind: &BuildDatasourcePlanKind) -> &'static str {
    match kind {
        BuildDatasourcePlanKind::BuiltinTool => "builtin_tool",
        BuildDatasourcePlanKind::McpTool => "mcp_tool",
        BuildDatasourcePlanKind::ProviderPrompt => "provider_prompt",
        BuildDatasourcePlanKind::Shared => "shared",
        BuildDatasourcePlanKind::Compose => "compose",
    }
}

fn source_label_for_shared(shared: &crate::models::dashboard::SharedDatasource) -> String {
    match shared.kind {
        BuildDatasourcePlanKind::BuiltinTool => shared
            .tool_name
            .clone()
            .map(|t| format!("builtin: {}", t))
            .unwrap_or_else(|| "builtin".to_string()),
        BuildDatasourcePlanKind::McpTool => format!(
            "mcp: {}/{}",
            shared.server_id.clone().unwrap_or_else(|| "?".to_string()),
            shared.tool_name.clone().unwrap_or_else(|| "?".to_string())
        ),
        BuildDatasourcePlanKind::ProviderPrompt => "provider prompt".to_string(),
        BuildDatasourcePlanKind::Shared => "shared".to_string(),
        BuildDatasourcePlanKind::Compose => "compose".to_string(),
    }
}

fn source_label_for_plan(plan: &BuildDatasourcePlan) -> String {
    match plan.kind {
        BuildDatasourcePlanKind::BuiltinTool => plan
            .tool_name
            .clone()
            .map(|t| format!("builtin: {}", t))
            .unwrap_or_else(|| "builtin".to_string()),
        BuildDatasourcePlanKind::McpTool => format!(
            "mcp: {}/{}",
            plan.server_id.clone().unwrap_or_else(|| "?".to_string()),
            plan.tool_name.clone().unwrap_or_else(|| "?".to_string())
        ),
        BuildDatasourcePlanKind::ProviderPrompt => "provider prompt".to_string(),
        BuildDatasourcePlanKind::Shared => plan
            .source_key
            .clone()
            .map(|k| format!("shared: {}", k))
            .unwrap_or_else(|| "shared".to_string()),
        BuildDatasourcePlanKind::Compose => "compose".to_string(),
    }
}

#[tauri::command]
pub async fn delete_dashboard(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    // W19: capture a final pre_delete snapshot so an accidental delete is
    // recoverable via the version list. SQLite FKs are not enforced in
    // this build, so the version row survives the cascade and can be
    // queried back. Failure to snapshot is logged but does not block the
    // delete since the user explicitly asked for it.
    if let Ok(Some(dashboard)) = state.storage.get_dashboard(&id).await {
        if let Err(error) = record_dashboard_version(
            &state,
            &dashboard,
            VersionSource::PreDelete,
            "Pre-delete snapshot",
            None,
            None,
        )
        .await
        {
            tracing::warn!("failed to record pre-delete snapshot for {}: {}", id, error);
        }
    }
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

/// W40: per-widget result returned by the batched dashboard refresh.
/// Mirrors the single-widget `refresh_widget` JSON shape so the frontend
/// can apply identical per-widget state transitions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DashboardWidgetRefreshResult {
    pub widget_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// W40: refresh many widgets on a dashboard in one pass. Widgets that
/// share the same datasource `workflow_id` execute the workflow exactly
/// once and then run their per-widget tail pipelines against the shared
/// output. Independent workflows run concurrently with a bounded cap
/// so one slow upstream call does not serialise the rest.
///
/// `widget_ids = None` refreshes every refreshable widget in layout
/// order. Widgets without a datasource are skipped (not reported as
/// errors) — the result vector only contains widgets that have a
/// workflow binding.
#[tauri::command]
pub async fn refresh_dashboard_widgets(
    app: AppHandle,
    state: State<'_, AppState>,
    dashboard_id: String,
    widget_ids: Option<Vec<String>>,
) -> Result<ApiResult<Vec<DashboardWidgetRefreshResult>>, String> {
    Ok(
        match refresh_dashboard_widgets_inner(app, &state, &dashboard_id, widget_ids.as_deref())
            .await
        {
            Ok(value) => ApiResult::ok(value),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W40: maximum concurrent workflow executions per batched dashboard
/// refresh. Kept small because each workflow may already run several
/// MCP/HTTP/LLM nodes; the goal is to overlap genuinely independent
/// upstream calls without stampeding the network.
const MAX_CONCURRENT_DASHBOARD_REFRESHES: usize = 4;

/// W40: build a `workflow_id -> consumer indexes` map from a list of
/// datasource-bound consumers. The grouping is the dedupe invariant
/// that prevents a shared workflow from running once per consumer —
/// pulled out as a pure helper so the dedupe contract has a unit test
/// that does not require spinning up the full Tauri runtime.
pub(crate) fn group_consumers_by_workflow(
    consumers: &[(usize, Widget, DatasourceConfig)],
) -> std::collections::BTreeMap<String, Vec<usize>> {
    let mut groups: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (consumer_idx, (_, _, ds)) in consumers.iter().enumerate() {
        groups
            .entry(ds.workflow_id.clone())
            .or_default()
            .push(consumer_idx);
    }
    groups
}

async fn refresh_dashboard_widgets_inner(
    app: AppHandle,
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_ids: Option<&[String]>,
) -> AnyResult<Vec<DashboardWidgetRefreshResult>> {
    use futures::stream::{FuturesUnordered, StreamExt};

    let dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;

    let filter: Option<std::collections::HashSet<&str>> =
        widget_ids.map(|ids| ids.iter().map(String::as_str).collect());

    // Preserve layout order in the returned vector so the UI can apply
    // updates without re-sorting.
    let consumers: Vec<(usize, Widget, DatasourceConfig)> = dashboard
        .layout
        .iter()
        .enumerate()
        .filter_map(|(idx, widget)| {
            if let Some(filter) = filter.as_ref() {
                if !filter.contains(widget.id()) {
                    return None;
                }
            }
            widget
                .datasource()
                .cloned()
                .map(|ds| (idx, widget.clone(), ds))
        })
        .collect();

    if consumers.is_empty() {
        return Ok(Vec::new());
    }

    // Group consumers by workflow_id so each unique workflow runs once.
    let groups = group_consumers_by_workflow(&consumers);

    let parameter_selections = resolve_dashboard_parameter_selections(state, &dashboard).await;
    reconnect_enabled_mcp_servers(state).await?;
    let app_provider = active_provider(state).await?;
    // W43: workflow runs are deduplicated by workflow_id; per-widget
    // model overrides cannot influence the shared workflow execution
    // here, so the dashboard-default policy (or app active provider) is
    // used to drive the run. The per-widget effective model is still
    // resolved below and applied to `finalize_widget_refresh`, which
    // executes the per-widget tail pipeline.
    let dashboard_provider = match dashboard.model_policy.as_ref() {
        Some(_) => {
            let providers = state.storage.list_providers().await?;
            match crate::modules::ai::resolve_effective_widget_model(
                // Pass a no-override probe widget so resolution picks
                // dashboard default; the source kind / participation does
                // not matter for resolution.
                consumers.first().map(|(_, w, _)| w).ok_or_else(|| {
                    anyhow!("internal: refresh_dashboard_widgets called with empty consumers")
                })?,
                &dashboard,
                &providers,
                app_provider.as_ref(),
            ) {
                Ok(Some(model)) => Some(crate::modules::ai::provider_with_model(&model)),
                Ok(None) => app_provider.clone(),
                Err(error) => return Err(anyhow!(error.to_string())),
            }
        }
        None => app_provider.clone(),
    };
    let provider = dashboard_provider;

    // Each `WorkflowExec` runs one unique workflow and yields the
    // resulting `WorkflowRun` (or error). We materialise the workflows
    // up front so a missing workflow turns into an explicit error per
    // consumer instead of a panic mid-stream.
    struct WorkflowExec {
        consumer_indexes: Vec<usize>,
        result: AnyResult<(Workflow, crate::models::workflow::WorkflowRun)>,
    }

    let mut tasks: FuturesUnordered<
        std::pin::Pin<Box<dyn std::future::Future<Output = WorkflowExec> + Send>>,
    > = FuturesUnordered::new();
    let mut pending: Vec<(String, Vec<usize>)> = groups.into_iter().collect();

    let mut completions: Vec<WorkflowExec> = Vec::with_capacity(pending.len());

    // Seed the queue up to the concurrency cap.
    while tasks.len() < MAX_CONCURRENT_DASHBOARD_REFRESHES && !pending.is_empty() {
        let (workflow_id, consumer_indexes) = pending.remove(0);
        let workflow_load = load_substituted_workflow(
            state,
            &dashboard,
            &workflow_id,
            &parameter_selections,
            &dashboard.parameters,
        )
        .await;
        match workflow_load {
            Ok(workflow) => {
                let app_clone = app.clone();
                let state_clone = state.inner().clone();
                let provider_clone = provider.clone();
                let workflow_clone = workflow.clone();
                let consumers_clone = consumer_indexes.clone();
                let dashboard_id_clone = dashboard.id.clone();
                tasks.push(Box::pin(async move {
                    let exec_result = run_workflow_via_state(
                        &state_clone,
                        &app_clone,
                        &workflow_clone,
                        provider_clone,
                        Some(dashboard_id_clone.as_str()),
                    )
                    .await
                    .map(|run| (workflow_clone, run));
                    WorkflowExec {
                        consumer_indexes: consumers_clone,
                        result: exec_result,
                    }
                }));
            }
            Err(error) => {
                completions.push(WorkflowExec {
                    consumer_indexes,
                    result: Err(error),
                });
            }
        }
    }

    while let Some(done) = tasks.next().await {
        completions.push(done);
        while tasks.len() < MAX_CONCURRENT_DASHBOARD_REFRESHES && !pending.is_empty() {
            let (workflow_id, consumer_indexes) = pending.remove(0);
            let workflow_load = load_substituted_workflow(
                state,
                &dashboard,
                &workflow_id,
                &parameter_selections,
                &dashboard.parameters,
            )
            .await;
            match workflow_load {
                Ok(workflow) => {
                    let app_clone = app.clone();
                    let state_clone = state.inner().clone();
                    let provider_clone = provider.clone();
                    let workflow_clone = workflow.clone();
                    let consumers_clone = consumer_indexes.clone();
                    let dashboard_id_clone = dashboard.id.clone();
                    tasks.push(Box::pin(async move {
                        let exec_result = run_workflow_via_state(
                            &state_clone,
                            &app_clone,
                            &workflow_clone,
                            provider_clone,
                            Some(dashboard_id_clone.as_str()),
                        )
                        .await
                        .map(|run| (workflow_clone, run));
                        WorkflowExec {
                            consumer_indexes: consumers_clone,
                            result: exec_result,
                        }
                    }));
                }
                Err(error) => {
                    completions.push(WorkflowExec {
                        consumer_indexes,
                        result: Err(error),
                    });
                }
            }
        }
    }

    // W42: claim a fresh stream run id per widget *before* execution
    // starts so the UI's RefreshStarted arrives ahead of any deltas.
    let stream_contexts: std::collections::HashMap<usize, WidgetStreamContext> = consumers
        .iter()
        .enumerate()
        .map(|(idx, (_, widget, _))| {
            let ctx = WidgetStreamContext::start(app.clone(), state, dashboard_id, widget.id());
            ctx.emit_refresh_started();
            (idx, ctx)
        })
        .collect();

    // Build per-widget results from the deduplicated workflow runs.
    let mut results: Vec<Option<DashboardWidgetRefreshResult>> =
        consumers.iter().map(|_| None).collect();
    for exec in completions {
        match exec.result {
            Err(error) => {
                let message = error.to_string();
                for consumer_idx in exec.consumer_indexes {
                    let (_, widget, _) = &consumers[consumer_idx];
                    if let Some(ctx) = stream_contexts.get(&consumer_idx) {
                        ctx.emit_failed(&message, None);
                    }
                    results[consumer_idx] = Some(DashboardWidgetRefreshResult {
                        widget_id: widget.id().to_string(),
                        status: "error".to_string(),
                        workflow_run_id: None,
                        data: None,
                        error: Some(message.clone()),
                    });
                }
            }
            Ok((workflow, run)) => {
                let node_results_opt = run.node_results.clone();
                for consumer_idx in exec.consumer_indexes {
                    let (_, widget, datasource) = &consumers[consumer_idx];
                    let ctx = stream_contexts.get(&consumer_idx);
                    // W43: each widget gets its own effective model for
                    // the tail pipeline so per-widget overrides apply
                    // even when the upstream workflow run is shared.
                    let per_widget_provider = match resolve_widget_effective_model(
                        state,
                        &dashboard,
                        widget,
                        app_provider.as_ref(),
                    )
                    .await
                    {
                        Ok(model) => model.as_ref().map(crate::modules::ai::provider_with_model),
                        Err(error) => {
                            let message = error.to_string();
                            if let Some(ctx) = ctx {
                                ctx.emit_failed(&message, None);
                            }
                            results[consumer_idx] = Some(DashboardWidgetRefreshResult {
                                widget_id: widget.id().to_string(),
                                status: "error".to_string(),
                                workflow_run_id: None,
                                data: None,
                                error: Some(message),
                            });
                            continue;
                        }
                    };
                    let outcome = match node_results_opt.as_ref() {
                        Some(node_results) => {
                            finalize_widget_refresh(
                                app.clone(),
                                state,
                                &dashboard,
                                widget,
                                datasource,
                                &workflow.id,
                                &run,
                                node_results,
                                &parameter_selections,
                                per_widget_provider.as_ref(),
                                ctx,
                            )
                            .await
                        }
                        None => Err(anyhow!("Datasource workflow returned no node results")),
                    };
                    let row = match &outcome {
                        Ok(data) => {
                            if let Some(ctx) = ctx {
                                ctx.emit_final(data, Some(run.id.as_str()));
                            }
                            DashboardWidgetRefreshResult {
                                widget_id: widget.id().to_string(),
                                status: "ok".to_string(),
                                workflow_run_id: Some(run.id.clone()),
                                data: Some(data.clone()),
                                error: None,
                            }
                        }
                        Err(error) => {
                            if let Some(ctx) = ctx {
                                ctx.emit_failed(&error.to_string(), None);
                            }
                            DashboardWidgetRefreshResult {
                                widget_id: widget.id().to_string(),
                                status: "error".to_string(),
                                workflow_run_id: Some(run.id.clone()),
                                data: None,
                                error: Some(error.to_string()),
                            }
                        }
                    };
                    results[consumer_idx] = Some(row);
                }
            }
        }
    }

    Ok(results
        .into_iter()
        .map(|row| row.expect("every consumer index must be filled by the batched refresh loop"))
        .collect())
}

/// W40: standalone version of `execute_dashboard_workflow` that takes a
/// cloned `AppState` rather than a `tauri::State`. The batched refresh
/// path needs to spawn execution futures that are owned by the
/// `FuturesUnordered` queue, which requires a `'static` future — we
/// can't carry the original `State<'_, AppState>` across the await
/// boundary.
async fn run_workflow_via_state(
    state: &AppState,
    app: &AppHandle,
    workflow: &Workflow,
    provider: Option<crate::models::provider::LLMProvider>,
    dashboard_id: Option<&str>,
) -> AnyResult<crate::models::workflow::WorkflowRun> {
    // W47: dashboard scope drives the language directive when the
    // workflow runs as part of a dashboard refresh; standalone runs
    // (no dashboard) fall back to the app default.
    let language_directive = crate::commands::language::resolve_effective_language(
        state.storage.as_ref(),
        dashboard_id,
        None,
    )
    .await
    .ok()
    .and_then(|resolved| resolved.system_directive());
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        provider,
    )
    .with_storage(state.storage.as_ref())
    .with_language(language_directive);
    let execution = engine.execute(workflow, None).await?;
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
                .clone()
                .unwrap_or_else(|| "unknown workflow error".to_string())
        ));
    }

    Ok(run)
}

/// W36: return the dashboard's last-known-good widget snapshots, after
/// pruning any whose config or parameter fingerprint no longer matches
/// the current widget state (or whose widget has since been removed).
/// Snapshots are display-only — the caller still has to kick off the
/// normal `refresh_widget` path to get live data.
#[tauri::command]
pub async fn list_widget_snapshots(
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<ApiResult<Vec<WidgetRuntimeSnapshot>>, String> {
    Ok(
        match list_widget_snapshots_inner(&state, &dashboard_id).await {
            Ok(value) => ApiResult::ok(value),
            Err(error) => ApiResult::err(error.to_string()),
        },
    )
}

async fn list_widget_snapshots_inner(
    state: &State<'_, AppState>,
    dashboard_id: &str,
) -> AnyResult<Vec<WidgetRuntimeSnapshot>> {
    let Some(dashboard) = state.storage.get_dashboard(dashboard_id).await? else {
        return Ok(Vec::new());
    };
    let snapshots = state.storage.list_widget_snapshots(dashboard_id).await?;
    if snapshots.is_empty() {
        return Ok(Vec::new());
    }

    let parameter_selections = state
        .storage
        .get_dashboard_parameter_values(dashboard_id)
        .await
        .unwrap_or_default();
    let current_param_fp = parameter_values_fingerprint(&parameter_selections);

    let mut by_id: std::collections::HashMap<&str, &Widget> = std::collections::HashMap::new();
    for widget in &dashboard.layout {
        by_id.insert(widget.id(), widget);
    }

    let mut out = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let widget = by_id.get(snapshot.widget_id.as_str());
        let config_match = widget
            .map(|w| widget_config_fingerprint(w) == snapshot.config_fingerprint)
            .unwrap_or(false);
        let param_match = current_param_fp == snapshot.parameter_fingerprint;
        if config_match && param_match {
            out.push(snapshot);
        } else if let Err(error) = state
            .storage
            .delete_widget_snapshot(&snapshot.dashboard_id, &snapshot.widget_id)
            .await
        {
            tracing::warn!(
                "failed to prune incompatible snapshot {}/{}: {}",
                snapshot.dashboard_id,
                snapshot.widget_id,
                error
            );
        }
    }
    Ok(out)
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
    // W25: optional parameter declarations + values for the dry-run path.
    parameters: Option<Vec<crate::models::dashboard::DashboardParameter>>,
    parameter_values: Option<
        std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue>,
    >,
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
    let resolved_params = match (parameters, parameter_values) {
        (Some(params), Some(values)) => ResolvedParameters::resolve(&params, &values).ok(),
        (Some(params), None) => ResolvedParameters::resolve(&params, &Default::default()).ok(),
        (None, Some(values)) => Some(ResolvedParameters::from_map(values)),
        (None, None) => None,
    };
    Ok(
        match dry_run_widget_inner(&state, &resolved, resolved_params.as_ref()).await {
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
        },
    )
}

/// If the widget's datasource_plan is kind='shared', resolve it against the
/// matching shared_datasource entry and inline the source + base pipeline
/// in front of the consumer pipeline. Returns a standalone equivalent that
/// dry_run can build a per-widget workflow from.
pub(crate) fn inline_shared_into_widget(
    mut proposal: crate::models::dashboard::BuildWidgetProposal,
    shared_datasources: Vec<crate::models::dashboard::SharedDatasource>,
) -> AnyResult<crate::models::dashboard::BuildWidgetProposal> {
    let Some(plan) = proposal.datasource_plan.as_ref() else {
        return Ok(proposal);
    };
    match plan.kind {
        BuildDatasourcePlanKind::Shared => {
            let inlined = inline_shared_plan(plan, &shared_datasources)?;
            proposal.datasource_plan = Some(inlined);
        }
        BuildDatasourcePlanKind::Compose => {
            let new_plan = inline_shared_in_compose(plan, &shared_datasources)?;
            proposal.datasource_plan = Some(new_plan);
        }
        _ => {}
    }
    Ok(proposal)
}

/// Inline a single `kind: shared` plan against the proposal's
/// shared_datasources list. Produces a standalone plan that can be turned
/// into a workflow directly.
fn inline_shared_plan(
    plan: &BuildDatasourcePlan,
    shared_datasources: &[crate::models::dashboard::SharedDatasource],
) -> AnyResult<BuildDatasourcePlan> {
    let key = plan
        .source_key
        .as_deref()
        .ok_or_else(|| anyhow!("Shared datasource_plan requires source_key"))?;
    let shared = shared_datasources
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| {
            anyhow!(
                "Shared source_key '{}' not provided; pass shared_datasources alongside the widget proposal",
                key
            )
        })?;
    let mut combined_pipeline = shared.pipeline.clone();
    combined_pipeline.extend(plan.pipeline.clone());
    Ok(BuildDatasourcePlan {
        kind: shared.kind.clone(),
        tool_name: shared.tool_name.clone(),
        server_id: shared.server_id.clone(),
        arguments: shared.arguments.clone(),
        prompt: shared.prompt.clone(),
        output_path: plan.output_path.clone(),
        refresh_cron: None,
        pipeline: combined_pipeline,
        source_key: None,
        inputs: None,
    })
}

/// Walk a `kind: compose` plan and resolve any inner input with
/// `kind: shared` against the shared_datasources list. Nested compose is
/// rejected (also caught later in workflow build, but failing here gives a
/// clearer error to the LLM).
fn inline_shared_in_compose(
    plan: &BuildDatasourcePlan,
    shared_datasources: &[crate::models::dashboard::SharedDatasource],
) -> AnyResult<BuildDatasourcePlan> {
    let inputs = plan
        .inputs
        .as_ref()
        .ok_or_else(|| anyhow!("compose datasource_plan requires `inputs`"))?;
    let mut resolved = std::collections::BTreeMap::new();
    for (key, inner) in inputs.iter() {
        if matches!(inner.kind, BuildDatasourcePlanKind::Compose) {
            return Err(anyhow!(
                "nested compose is not supported (input '{}' is also compose)",
                key
            ));
        }
        let inner_plan = if matches!(inner.kind, BuildDatasourcePlanKind::Shared) {
            inline_shared_plan(inner, shared_datasources)?
        } else {
            inner.clone()
        };
        resolved.insert(key.clone(), inner_plan);
    }
    Ok(BuildDatasourcePlan {
        kind: BuildDatasourcePlanKind::Compose,
        tool_name: None,
        server_id: None,
        arguments: None,
        prompt: None,
        output_path: plan.output_path.clone(),
        refresh_cron: plan.refresh_cron.clone(),
        pipeline: plan.pipeline.clone(),
        source_key: None,
        inputs: Some(resolved),
    })
}

async fn dry_run_widget_inner(
    state: &State<'_, AppState>,
    proposal: &crate::models::dashboard::BuildWidgetProposal,
    resolved_params: Option<&ResolvedParameters>,
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

    let (widget, mut workflow) = proposal_widget(proposal, 0, now)?;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow!("Widget has no datasource workflow"))?;
    let workflow_node_ids: Vec<String> = workflow.nodes.iter().map(|n| n.id.clone()).collect();

    if let Some(params) = resolved_params {
        parameter_engine::substitute_workflow(&mut workflow, params, SubstituteOptions::default());
    }

    reconnect_enabled_mcp_servers(state).await?;
    let started = std::time::Instant::now();
    // W47: dry-run has no dashboard scope (the proposal isn't applied
    // yet), so fall back to the app default language.
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
    mut req: ApplyBuildProposalRequest,
) -> AnyResult<Dashboard> {
    if req.proposal.widgets.is_empty() && req.proposal.remove_widget_ids.is_empty() {
        return Err(anyhow!(
            "Build proposal contains no widget changes to apply"
        ));
    }

    // W18: collected at every replacement/append site so the post-apply
    // reflection registry can index by widget_id. We need `replaced` to
    // tell the reflection turn whether the widget is fresh or edited.
    let mut reflection_targets: Vec<(String, String, &'static str, bool)> = Vec::new();
    let now = chrono::Utc::now().timestamp_millis();

    // Compose plans with `kind: shared` inputs must be inlined against the
    // proposal's shared_datasources before workflow build (the head builder
    // refuses to embed a Shared kind directly). Walk every widget once and
    // resolve in place.
    let shared_for_inline = req.proposal.shared_datasources.clone();
    for widget in req.proposal.widgets.iter_mut() {
        if matches!(
            widget.datasource_plan.as_ref().map(|p| &p.kind),
            Some(BuildDatasourcePlanKind::Compose)
        ) {
            if let Some(plan) = widget.datasource_plan.as_ref() {
                let resolved = inline_shared_in_compose(plan, &shared_for_inline)?;
                widget.datasource_plan = Some(resolved);
            }
        }
    }

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
    // W31: try to reuse a saved DatasourceDefinition whose signature
    // (kind + server_id + tool_name + arguments + prompt + pipeline)
    // matches an incoming shared datasource. When matched, widgets bind
    // to the saved definition (`datasource_definition_id`) and reuse
    // its backing `workflow_id` instead of minting a fresh shared
    // fan-out. Mismatched signatures fall through to the legacy path.
    let mut shared_reuse_targets: std::collections::HashMap<String, String> = Default::default();
    let mut shared_reuse_workflow_ids: std::collections::HashMap<String, String> =
        Default::default();
    if !consumers_by_key.is_empty() {
        let saved = state.storage.list_datasource_definitions().await?;
        for key in consumers_by_key.keys() {
            let shared = shared_by_key.get(key.as_str()).copied().unwrap();
            if let Some(reused) = saved
                .iter()
                .find(|def| shared_matches_definition(shared, def))
            {
                shared_reuse_targets.insert(key.clone(), reused.id.clone());
                shared_reuse_workflow_ids.insert(key.clone(), reused.workflow_id.clone());
            }
        }
    }

    let mut shared_workflow_ids: std::collections::HashMap<String, String> = Default::default();
    let mut prebuilt_widget_ids: std::collections::HashMap<usize, String> = Default::default();
    for key in consumers_by_key.keys() {
        let workflow_id = shared_reuse_workflow_ids
            .get(key)
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        shared_workflow_ids.insert(key.clone(), workflow_id);
    }
    for indices in consumers_by_key.values() {
        for &idx in indices {
            prebuilt_widget_ids.insert(idx, uuid::Uuid::new_v4().to_string());
        }
    }

    // W39: auto-materialize inline single-widget datasource plans into
    // saved DatasourceDefinitions so Build Chat creates real catalog
    // entries instead of anonymous per-widget workflows. Eligible plans
    // are non-shared, non-compose, no `output_path`, no `inputs`. Widgets
    // in the same proposal that share a canonical signature collapse to
    // one definition; plans matching a saved definition reuse it.
    use crate::modules::datasource_signature::DatasourceSignature;
    let mut inline_resolutions: std::collections::HashMap<usize, InlineMaterialization> =
        Default::default();
    let mut new_inline_definitions: Vec<(DatasourceDefinition, Workflow)> = Vec::new();
    {
        let saved_defs = state.storage.list_datasource_definitions().await?;
        let mut sig_to_def: std::collections::HashMap<DatasourceSignature, (String, String)> =
            Default::default();
        for def in &saved_defs {
            if let Some(sig) = DatasourceSignature::from_definition(def) {
                sig_to_def
                    .entry(sig)
                    .or_insert_with(|| (def.id.clone(), def.workflow_id.clone()));
            }
        }
        for (idx, widget) in req.proposal.widgets.iter().enumerate() {
            let Some(plan) = widget.datasource_plan.as_ref() else {
                continue;
            };
            if matches!(
                plan.kind,
                BuildDatasourcePlanKind::Shared | BuildDatasourcePlanKind::Compose
            ) {
                continue;
            }
            // Plans with an output_path or inputs object are not
            // collapse-safe yet: their per-widget shaping would not run
            // on the shared definition's workflow.
            if plan
                .output_path
                .as_deref()
                .map(|p| !p.trim().is_empty())
                .unwrap_or(false)
            {
                continue;
            }
            if plan.inputs.as_ref().is_some_and(|m| !m.is_empty()) {
                continue;
            }
            let Some(sig) = DatasourceSignature::from_inline_plan(plan) else {
                continue;
            };
            // W39: HTTP datasources go through the safety gate before we
            // ever consider materializing them. Apply-time validation
            // already runs validate_build_proposal first, but a defence
            // in depth catches drift between the gate and the actual
            // catalog write.
            if matches!(plan.kind, BuildDatasourcePlanKind::BuiltinTool)
                && plan.tool_name.as_deref() == Some("http_request")
            {
                if let Some(args) = plan.arguments.as_ref() {
                    crate::modules::tool_engine::validate_http_request_arguments(args).map_err(
                        |e| {
                            anyhow!(
                                "Cannot materialize HTTP datasource for widget '{}': {}",
                                widget.title,
                                e
                            )
                        },
                    )?;
                } else {
                    return Err(anyhow!(
                        "Cannot materialize HTTP datasource for widget '{}': missing arguments object",
                        widget.title
                    ));
                }
            }
            if let Some((def_id, workflow_id)) = sig_to_def.get(&sig) {
                inline_resolutions.insert(
                    idx,
                    InlineMaterialization::Reuse {
                        def_id: def_id.clone(),
                        workflow_id: workflow_id.clone(),
                    },
                );
                continue;
            }
            let def_id = uuid::Uuid::new_v4().to_string();
            let workflow_id = uuid::Uuid::new_v4().to_string();
            let new_def = DatasourceDefinition {
                id: def_id.clone(),
                name: derive_definition_name(widget, plan, &new_inline_definitions),
                description: Some(format!(
                    "Auto-materialized by Build Chat from widget '{}'.",
                    widget.title
                )),
                kind: plan.kind.clone(),
                tool_name: plan.tool_name.clone(),
                server_id: plan.server_id.clone(),
                arguments: plan.arguments.clone(),
                prompt: plan.prompt.clone(),
                pipeline: plan.pipeline.clone(),
                refresh_cron: plan.refresh_cron.clone().filter(|s| !s.trim().is_empty()),
                workflow_id: workflow_id.clone(),
                created_at: now,
                updated_at: now,
                health: None,
                originated_external_source_id: None,
            };
            let synthetic_proposal = BuildWidgetProposal {
                widget_type: widget.widget_type.clone(),
                title: new_def.name.clone(),
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
            let workflow = datasource_plan_workflow(
                workflow_id.clone(),
                format!("Datasource: {}", new_def.name),
                &synthetic_proposal,
                plan,
                now,
            )?;
            sig_to_def.insert(sig, (def_id.clone(), workflow_id.clone()));
            inline_resolutions.insert(
                idx,
                InlineMaterialization::Create {
                    def_id,
                    workflow_id,
                },
            );
            new_inline_definitions.push((new_def, workflow));
        }
    }

    // Build the shared fan-out workflows up front. Each one combines the
    // shared source, optional shared pipeline, and a per-consumer tail
    // ending at `output_<widget_id>`. Cron is attached to the shared
    // workflow so a single tick refreshes every consumer.
    // Reused datasources skip workflow rebuild: their saved workflow
    // already runs the same source pipeline, and rewriting it here would
    // drop the consumer tails of unrelated dashboards.
    let mut shared_workflows: Vec<Workflow> = Vec::new();
    for (key, consumer_indices) in &consumers_by_key {
        if shared_reuse_targets.contains_key(key) {
            continue;
        }
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
        Some(id) => {
            let existing = state
                .storage
                .get_dashboard(id)
                .await?
                .ok_or_else(|| anyhow!("Dashboard not found"))?;
            // W19: snapshot the pre-apply state so the user can Undo or
            // restore. `session_id` is recorded so the History drawer can
            // jump back to the chat that produced the snapshot.
            let summary = proposal_summary(&req);
            record_dashboard_version(
                state,
                &existing,
                VersionSource::AgentApply,
                &summary,
                req.session_id.as_deref(),
                None,
            )
            .await?;
            existing
        }
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
            parameters: Vec::new(),
            model_policy: None,
            language_policy: None,
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

    // W39: persist auto-materialized DatasourceDefinitions + their backing
    // workflows. The widget loop below binds consumer widgets to these
    // workflow_ids; the saved definitions show up in the Workbench catalog
    // immediately without a separate rescan.
    for (def, workflow) in &new_inline_definitions {
        state.storage.create_workflow(workflow).await?;
        schedule_workflow_if_cron(app, state, workflow.clone()).await?;
        state.storage.insert_datasource_definition(def).await?;
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
                    let reused_id = shared_reuse_targets.get(key.as_str()).cloned();
                    let output_key = if reused_id.is_some() {
                        "output.data".to_string()
                    } else {
                        format!("output_{}.data", widget_id)
                    };
                    let tail_pipeline =
                        consumer_tail_for_reuse(widget_proposal, reused_id.as_ref());
                    let datasource = DatasourceConfig {
                        workflow_id: shared_workflow_id,
                        output_key,
                        post_process: None,
                        capture_traces: false,
                        datasource_definition_id: reused_id,
                        binding_source: Some(
                            crate::models::widget::DatasourceBindingSource::BuildChat,
                        ),
                        bound_at: Some(now),
                        tail_pipeline,
                        model_override: None,
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
                } else if let Some(resolution) = inline_resolutions.get(&widget_index).cloned() {
                    let widget_id = uuid::Uuid::new_v4().to_string();
                    let datasource = DatasourceConfig {
                        workflow_id: resolution.workflow_id().to_string(),
                        output_key: "output.data".to_string(),
                        post_process: None,
                        capture_traces: false,
                        datasource_definition_id: Some(resolution.def_id().to_string()),
                        binding_source: Some(
                            crate::models::widget::DatasourceBindingSource::BuildChat,
                        ),
                        bound_at: Some(now),
                        tail_pipeline: Vec::new(),
                        model_override: None,
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
                    && widget_proposal.size_preset.is_none()
                {
                    overwrite_widget_position(&mut widget, &preserved);
                }
                if let Some(workflow) = workflow_opt {
                    state.storage.create_workflow(&workflow).await?;
                    schedule_workflow_if_cron(app, state, workflow.clone()).await?;
                    dashboard.workflows.push(workflow);
                }
                let replaced_id = widget.id().to_string();
                let replaced_title = widget.title().to_string();
                let replaced_kind = widget_kind_label(&widget);
                dashboard.layout[index] = widget;
                reflection_targets.push((replaced_id, replaced_title, replaced_kind, true));
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
            let reused_id = shared_reuse_targets.get(key.as_str()).cloned();
            let output_key = if reused_id.is_some() {
                "output.data".to_string()
            } else {
                format!("output_{}.data", widget_id)
            };
            let tail_pipeline = consumer_tail_for_reuse(widget_proposal, reused_id.as_ref());
            let datasource = DatasourceConfig {
                workflow_id: shared_workflow_id,
                output_key,
                post_process: None,
                capture_traces: false,
                datasource_definition_id: reused_id,
                binding_source: Some(crate::models::widget::DatasourceBindingSource::BuildChat),
                bound_at: Some(now),
                tail_pipeline,
                model_override: None,
            };
            (
                build_widget_shell(widget_proposal, auto_cursor_y, widget_id, Some(datasource))?,
                None,
            )
        } else if let Some(resolution) = inline_resolutions.get(&widget_index).cloned() {
            let widget_id = uuid::Uuid::new_v4().to_string();
            let datasource = DatasourceConfig {
                workflow_id: resolution.workflow_id().to_string(),
                output_key: "output.data".to_string(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: Some(resolution.def_id().to_string()),
                binding_source: Some(crate::models::widget::DatasourceBindingSource::BuildChat),
                bound_at: Some(now),
                tail_pipeline: Vec::new(),
                model_override: None,
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
        let added_id = widget.id().to_string();
        let added_title = widget.title().to_string();
        let added_kind = widget_kind_label(&widget);
        dashboard.layout.push(widget);
        reflection_targets.push((added_id, added_title, added_kind, false));
    }
    // W25: merge proposed parameters into the dashboard. Existing entries
    // with the same `id` are replaced; new ones are appended. An empty
    // `proposal.parameters` list is a no-op (preserves existing).
    for proposed in &req.proposal.parameters {
        if let Some(existing) = dashboard
            .parameters
            .iter_mut()
            .find(|p| p.id == proposed.id)
        {
            *existing = proposed.clone();
        } else {
            dashboard.parameters.push(proposed.clone());
        }
    }

    dashboard.updated_at = now;

    if req.dashboard_id.is_some() {
        state.storage.update_dashboard(&dashboard).await?;
    } else {
        state.storage.create_dashboard(&dashboard).await?;
    }

    // W18: register a one-shot reflection job for each widget the agent
    // just shipped, scoped to the chat session that produced the
    // proposal. The first successful `refresh_widget` for any of these
    // ids consumes the entry and triggers `enqueue_reflection_turn`.
    if let Some(session_id) = req.session_id.as_deref() {
        let dashboard_id = dashboard.id.clone();
        for (widget_id, title, kind, replaced) in reflection_targets {
            state.pending_reflections.insert(
                widget_id.clone(),
                ReflectionPending {
                    session_id: session_id.to_string(),
                    dashboard_id: dashboard_id.clone(),
                    widget_id,
                    widget_title: title,
                    widget_kind: kind,
                    replaced,
                    applied_at: now,
                },
            );
        }
    }

    Ok(dashboard)
}

pub(crate) fn widget_kind_label(widget: &Widget) -> &'static str {
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
        capture_traces: false,
        datasource_definition_id: None,
        binding_source: Some(crate::models::widget::DatasourceBindingSource::BuildChat),
        bound_at: Some(now),
        tail_pipeline: Vec::new(),
        model_override: None,
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
    // W45: auto-pack always wins for new widgets; explicit x/y is ignored
    // here (the outer apply loop also overwrites position).
    let x = 0;
    let y = default_y;
    // W45: if the agent supplied a size_preset, resolve it to (w, h) up
    // front and use it as the preferred default. Explicit `w`/`h` is
    // still honoured when set without a preset; the validator forbids
    // setting both at the same time.
    let preset_size = proposal
        .size_preset
        .map(|preset| preset.resolve(&proposal.widget_type));
    let preset_w = preset_size.map(|(w, _)| w);
    let preset_h = preset_size.map(|(_, h)| h);

    let widget = match proposal.widget_type {
        BuildWidgetType::Text => Widget::Text {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: preset_w.or(proposal.w).unwrap_or(6),
            h: preset_h.or(proposal.h).unwrap_or(3),
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
            w: preset_w.or(proposal.w).unwrap_or(4),
            h: preset_h.or(proposal.h).unwrap_or(4),
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
            w: preset_w.or(proposal.w).unwrap_or(8),
            h: preset_h.or(proposal.h).unwrap_or(5),
            config: proposal_config(proposal)
                .unwrap_or_else(|| table_config_from_data(&proposal.data)),
            datasource,
        },
        BuildWidgetType::Chart => Widget::Chart {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: preset_w.or(proposal.w).unwrap_or(8),
            h: preset_h.or(proposal.h).unwrap_or(5),
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
            w: preset_w.or(proposal.w).unwrap_or(6),
            h: preset_h.or(proposal.h).unwrap_or(4),
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
            w: preset_w.or(proposal.w).unwrap_or(3),
            h: preset_h.or(proposal.h).unwrap_or(2),
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
            w: preset_w.or(proposal.w).unwrap_or(12),
            h: preset_h.or(proposal.h).unwrap_or(6),
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
            w: preset_w.or(proposal.w).unwrap_or(8),
            h: preset_h.or(proposal.h).unwrap_or(5),
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
            w: preset_w.or(proposal.w).unwrap_or(8),
            h: preset_h.or(proposal.h).unwrap_or(4),
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
            w: preset_w.or(proposal.w).unwrap_or(12),
            h: preset_h.or(proposal.h).unwrap_or(6),
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
        BuildWidgetType::Gallery => Widget::Gallery {
            id: widget_id,
            title: proposal.title.clone(),
            x,
            y,
            w: preset_w.or(proposal.w).unwrap_or(8),
            h: preset_h.or(proposal.h).unwrap_or(6),
            config: proposal_config(proposal).unwrap_or(crate::models::widget::GalleryConfig {
                layout: crate::models::widget::GalleryLayout::Grid,
                thumbnail_aspect: crate::models::widget::GalleryAspect::Landscape,
                max_visible_items: 24,
                show_caption: true,
                show_source: false,
                fullscreen_enabled: true,
                fit: crate::models::widget::ImageFit::Cover,
                border_radius: 4,
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
/// W31: heuristic match between an incoming `SharedDatasource` and a
/// saved `DatasourceDefinition`. Compared fields cover everything that
/// would otherwise produce a duplicate workflow: kind, server, tool,
/// prompt, deterministic pipeline, plus the **arguments** JSON. We
/// intentionally ignore name / description / refresh cron because the
/// catalog is the source of truth for those; the proposal's intent is
/// captured by the signature.
/// W31.1: when a shared datasource is reused (saved single-output
/// workflow), the per-consumer pipeline declared on the proposal
/// becomes the widget's `tail_pipeline`. For non-reused (fresh fan-out)
/// shared datasources the consumer pipeline is already baked into the
/// fan-out workflow's per-consumer tail nodes, so the widget tail stays
/// empty.
fn consumer_tail_for_reuse(
    proposal: &BuildWidgetProposal,
    reused_definition_id: Option<&String>,
) -> Vec<crate::models::pipeline::PipelineStep> {
    if reused_definition_id.is_none() {
        return Vec::new();
    }
    proposal
        .datasource_plan
        .as_ref()
        .map(|p| p.pipeline.clone())
        .unwrap_or_default()
}

/// W39: outcome of resolving an inline widget `datasource_plan` against
/// the saved datasource catalog. Either the apply path reuses a saved
/// definition that already has the same canonical signature, or it
/// materializes a fresh one (and persists it later in the same
/// transaction).
#[derive(Debug, Clone)]
enum InlineMaterialization {
    Reuse { def_id: String, workflow_id: String },
    Create { def_id: String, workflow_id: String },
}

impl InlineMaterialization {
    fn def_id(&self) -> &str {
        match self {
            InlineMaterialization::Reuse { def_id, .. }
            | InlineMaterialization::Create { def_id, .. } => def_id,
        }
    }
    fn workflow_id(&self) -> &str {
        match self {
            InlineMaterialization::Reuse { workflow_id, .. }
            | InlineMaterialization::Create { workflow_id, .. } => workflow_id,
        }
    }
}

/// W39: produce a Workbench-friendly name for an auto-materialized
/// datasource. Falls back to a kind+tool label so the catalog list
/// stays readable when widgets repeat titles.
fn derive_definition_name(
    widget: &BuildWidgetProposal,
    plan: &BuildDatasourcePlan,
    pending: &[(DatasourceDefinition, Workflow)],
) -> String {
    let trimmed = widget.title.trim();
    let base = if trimmed.is_empty() {
        match plan.kind {
            BuildDatasourcePlanKind::BuiltinTool => plan
                .tool_name
                .clone()
                .unwrap_or_else(|| "builtin source".to_string()),
            BuildDatasourcePlanKind::McpTool => plan
                .tool_name
                .clone()
                .unwrap_or_else(|| "mcp source".to_string()),
            BuildDatasourcePlanKind::ProviderPrompt => "provider prompt".to_string(),
            BuildDatasourcePlanKind::Shared => "shared".to_string(),
            BuildDatasourcePlanKind::Compose => "compose".to_string(),
        }
    } else {
        trimmed.to_string()
    };
    if !pending.iter().any(|(d, _)| d.name == base) {
        return base;
    }
    for suffix in 2..=64u32 {
        let candidate = format!("{} ({})", base, suffix);
        if !pending.iter().any(|(d, _)| d.name == candidate) {
            return candidate;
        }
    }
    format!("{} ({})", base, chrono::Utc::now().timestamp_millis())
}

pub(crate) fn shared_matches_definition(
    shared: &crate::models::dashboard::SharedDatasource,
    def: &DatasourceDefinition,
) -> bool {
    // W39: route through the canonical signature so reordered JSON keys
    // and equivalent whitespace dedupe instead of producing a duplicate
    // workflow. The signature ignores name/description/refresh_cron by
    // design.
    let shared_sig = crate::modules::datasource_signature::DatasourceSignature::from_shared(shared);
    let def_sig = crate::modules::datasource_signature::DatasourceSignature::from_definition(def);
    match (shared_sig, def_sig) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

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
        inputs: None,
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
        pause_state: Default::default(),
        last_paused_at: None,
        last_pause_reason: None,
        last_run: None,
        created_at: now,
        updated_at: now,
    })
}

pub(crate) fn datasource_plan_workflow(
    workflow_id: String,
    name: String,
    proposal: &BuildWidgetProposal,
    plan: &BuildDatasourcePlan,
    now: i64,
) -> AnyResult<Workflow> {
    let output_path = plan
        .output_path
        .as_deref()
        .filter(|path| !path.trim().is_empty());

    let (mut nodes, mut edges, mut tail_node, source_kind_label) = datasource_head_nodes(plan)?;

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
        pause_state: Default::default(),
        last_paused_at: None,
        last_pause_reason: None,
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
    datasource_source_node_with_id(plan, "source")
}

fn datasource_source_node_with_id(
    plan: &BuildDatasourcePlan,
    node_id: &str,
) -> AnyResult<(WorkflowNode, &'static str)> {
    match plan.kind {
        BuildDatasourcePlanKind::BuiltinTool => {
            let tool_name = plan
                .tool_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| anyhow!("builtin_tool datasource_plan requires tool_name"))?;
            Ok((
                WorkflowNode {
                    id: node_id.to_string(),
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
                    id: node_id.to_string(),
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
                    id: node_id.to_string(),
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
        BuildDatasourcePlanKind::Compose => Err(anyhow!(
            "Compose datasource_plan must be expanded by datasource_head_nodes, not handled as a single source node"
        )),
    }
}

/// Build the head of a widget workflow: returns the nodes/edges needed to
/// produce a single tail value, the id of that tail node, and a human label
/// describing the source kind. For a single-source plan, this is just the
/// usual `source` node. For `kind: compose`, this expands into N independent
/// source+pipeline branches feeding a Merge node aliased to the user-facing
/// input keys.
fn datasource_head_nodes(
    plan: &BuildDatasourcePlan,
) -> AnyResult<(Vec<WorkflowNode>, Vec<WorkflowEdge>, String, &'static str)> {
    if !matches!(plan.kind, BuildDatasourcePlanKind::Compose) {
        let (node, label) = datasource_source_node(plan)?;
        let id = node.id.clone();
        return Ok((vec![node], Vec::new(), id, label));
    }
    let inputs = plan
        .inputs
        .as_ref()
        .ok_or_else(|| anyhow!("compose datasource_plan requires `inputs`"))?;
    if inputs.is_empty() {
        return Err(anyhow!(
            "compose datasource_plan `inputs` must be non-empty"
        ));
    }
    const RESERVED: &[&str] = &["source", "shape", "pipeline", "output", "merge"];
    let mut nodes: Vec<WorkflowNode> = Vec::new();
    let mut edges: Vec<WorkflowEdge> = Vec::new();
    let mut merge_keys: Vec<String> = Vec::new();
    let mut key_map = serde_json::Map::new();
    for (input_key, inner) in inputs.iter() {
        let trimmed = input_key.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("compose input names must be non-empty"));
        }
        if RESERVED.contains(&trimmed) {
            return Err(anyhow!(
                "compose input name '{}' collides with a reserved workflow node id",
                trimmed
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(anyhow!(
                "compose input name '{}' must contain only alphanumerics, '_' or '-'",
                trimmed
            ));
        }
        if matches!(inner.kind, BuildDatasourcePlanKind::Compose) {
            return Err(anyhow!(
                "nested compose is not supported (input '{}' is also compose)",
                trimmed
            ));
        }
        if matches!(inner.kind, BuildDatasourcePlanKind::Shared) {
            return Err(anyhow!(
                "compose input '{}' is kind='shared' but was not inlined; resolve shared_datasources before building the workflow",
                trimmed
            ));
        }
        let source_id = format!("__compose__{}__source", trimmed);
        let (source_node, _label) = datasource_source_node_with_id(inner, &source_id)?;
        nodes.push(source_node);
        let mut branch_tail = source_id.clone();
        if let Some(path) = inner
            .output_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            let id = format!("__compose__{}__shape", trimmed);
            nodes.push(WorkflowNode {
                id: id.clone(),
                kind: NodeKind::Transform,
                label: format!("compose[{}] output_path", trimmed),
                position: None,
                config: Some(json!({
                    "input_key": branch_tail,
                    "transform": "pick_path",
                    "path": path,
                })),
            });
            edges.push(WorkflowEdge {
                id: format!("{}-to-{}", branch_tail, id),
                source: branch_tail.clone(),
                target: id.clone(),
                condition: None,
            });
            branch_tail = id;
        }
        if !inner.pipeline.is_empty() {
            let id = format!("__compose__{}__pipeline", trimmed);
            nodes.push(WorkflowNode {
                id: id.clone(),
                kind: NodeKind::Transform,
                label: format!(
                    "compose[{}] pipeline ({} step(s))",
                    trimmed,
                    inner.pipeline.len()
                ),
                position: None,
                config: Some(json!({
                    "input_key": branch_tail,
                    "transform": "pipeline",
                    "steps": inner.pipeline,
                })),
            });
            edges.push(WorkflowEdge {
                id: format!("{}-to-{}", branch_tail, id),
                source: branch_tail.clone(),
                target: id.clone(),
                condition: None,
            });
            branch_tail = id;
        }
        merge_keys.push(branch_tail.clone());
        key_map.insert(branch_tail, Value::String(trimmed.to_string()));
    }
    let merge_id = "merge".to_string();
    nodes.push(WorkflowNode {
        id: merge_id.clone(),
        kind: NodeKind::Merge,
        label: format!("compose merge ({} inputs)", inputs.len()),
        position: None,
        config: Some(json!({
            "keys": merge_keys.clone(),
            "key_map": Value::Object(key_map),
        })),
    });
    for branch_tail in &merge_keys {
        edges.push(WorkflowEdge {
            id: format!("{}-to-{}", branch_tail, merge_id),
            source: branch_tail.clone(),
            target: merge_id.clone(),
            condition: None,
        });
    }
    Ok((nodes, edges, merge_id, "compose: multi-source merge"))
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
        .ok_or_else(|| anyhow!("Widget not found"))?
        .clone();
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow!("Widget has no datasource workflow"))?
        .clone();

    // W42: claim the active stream run id for this widget. The UI uses
    // the id to drop deltas from a superseded refresh — if a newer
    // refresh starts mid-flight, our `Final`/`Failed` envelope will
    // arrive with the older id and the UI ignores it.
    let stream_ctx = WidgetStreamContext::start(app.clone(), state, dashboard_id, widget_id);
    stream_ctx.emit_refresh_started();

    let parameter_selections = resolve_dashboard_parameter_selections(state, &dashboard).await;
    let workflow = match load_substituted_workflow(
        state,
        &dashboard,
        &datasource.workflow_id,
        &parameter_selections,
        &dashboard.parameters,
    )
    .await
    {
        Ok(workflow) => workflow,
        Err(error) => {
            stream_ctx.emit_failed(&error.to_string(), None);
            return Err(error);
        }
    };

    if let Err(error) = reconnect_enabled_mcp_servers(state).await {
        stream_ctx.emit_failed(&error.to_string(), None);
        return Err(error);
    }
    let app_provider = match active_provider(state).await {
        Ok(provider) => provider,
        Err(error) => {
            stream_ctx.emit_failed(&error.to_string(), None);
            return Err(error);
        }
    };
    // W43: pick the widget-effective provider/model — widget override
    // beats dashboard default, dashboard default beats the app active
    // provider. Capability checks fail closed with a typed error rather
    // than silently falling through to the app default.
    let effective_model =
        match resolve_widget_effective_model(state, &dashboard, &widget, app_provider.as_ref())
            .await
        {
            Ok(model) => model,
            Err(error) => {
                stream_ctx.emit_failed(&error.to_string(), None);
                return Err(error);
            }
        };
    let provider = effective_model
        .as_ref()
        .map(crate::modules::ai::provider_with_model);
    let run = match run_workflow_via_state(
        state.inner(),
        &app,
        &workflow,
        provider.clone(),
        Some(dashboard.id.as_str()),
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            stream_ctx.emit_failed(&error.to_string(), None);
            return Err(error);
        }
    };

    let node_results = match run.node_results.clone() {
        Some(value) => value,
        None => {
            let error = anyhow!("Datasource workflow returned no node results");
            stream_ctx.emit_failed(&error.to_string(), None);
            return Err(error);
        }
    };

    let data = match finalize_widget_refresh(
        app.clone(),
        state,
        &dashboard,
        &widget,
        &datasource,
        &workflow.id,
        &run,
        &node_results,
        &parameter_selections,
        provider.as_ref(),
        Some(&stream_ctx),
    )
    .await
    {
        Ok(data) => data,
        Err(error) => {
            stream_ctx.emit_failed(&error.to_string(), None);
            return Err(error);
        }
    };

    stream_ctx.emit_final(&data, Some(run.id.as_str()));

    Ok(json!({
        "status": "ok",
        "workflow_run_id": run.id,
        "data": data,
    }))
}

/// W42: per-refresh stream emitter. Owns the run id, sequence
/// counter, and the dashboard/widget targets so the rest of the
/// refresh path can emit typed events without rebuilding the
/// envelope. Cheaply clonable.
pub(crate) struct WidgetStreamContext {
    app: AppHandle,
    dashboard_id: String,
    widget_id: String,
    refresh_run_id: String,
    sequence: std::sync::atomic::AtomicU32,
    registry: std::sync::Arc<dashmap::DashMap<String, String>>,
}

impl WidgetStreamContext {
    pub(crate) fn start(
        app: AppHandle,
        state: &State<'_, AppState>,
        dashboard_id: &str,
        widget_id: &str,
    ) -> Self {
        let refresh_run_id = uuid::Uuid::new_v4().to_string();
        state
            .widget_refresh_runs
            .insert(widget_id.to_string(), refresh_run_id.clone());
        Self {
            app,
            dashboard_id: dashboard_id.to_string(),
            widget_id: widget_id.to_string(),
            refresh_run_id,
            sequence: std::sync::atomic::AtomicU32::new(0),
            registry: state.widget_refresh_runs.clone(),
        }
    }

    pub(crate) fn refresh_run_id(&self) -> &str {
        &self.refresh_run_id
    }

    fn next_sequence(&self) -> u32 {
        self.sequence
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Returns true when our `refresh_run_id` is still the active one
    /// for this widget. Used to skip emitting deltas from a refresh
    /// that was already superseded by a newer call.
    fn is_current(&self) -> bool {
        self.registry
            .get(&self.widget_id)
            .map(|entry| entry.value() == &self.refresh_run_id)
            .unwrap_or(false)
    }

    fn emit(&self, kind: WidgetStreamKind, payload: WidgetStreamPayload) {
        // Only the active run publishes deltas. Terminal events
        // (`Final` / `Failed`) still publish unconditionally so the UI
        // can clean up partial state belonging to this run id.
        if !matches!(
            kind,
            WidgetStreamKind::Final | WidgetStreamKind::Failed | WidgetStreamKind::Superseded
        ) && !self.is_current()
        {
            return;
        }
        let envelope = WidgetStreamEnvelope {
            dashboard_id: self.dashboard_id.clone(),
            widget_id: self.widget_id.clone(),
            refresh_run_id: self.refresh_run_id.clone(),
            sequence: self.next_sequence(),
            kind,
            payload,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        };
        if let Err(error) = self.app.emit(WIDGET_STREAM_EVENT_CHANNEL, envelope) {
            tracing::warn!(
                "widget stream emit failed for {}/{}: {}",
                self.dashboard_id,
                self.widget_id,
                error
            );
        }
    }

    pub(crate) fn emit_refresh_started(&self) {
        self.emit(
            WidgetStreamKind::RefreshStarted,
            WidgetStreamPayload::default(),
        );
    }

    pub(crate) fn emit_text_delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emit(
            WidgetStreamKind::TextDelta,
            WidgetStreamPayload {
                text: Some(text.to_string()),
                ..Default::default()
            },
        );
    }

    pub(crate) fn emit_reasoning_delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emit(
            WidgetStreamKind::ReasoningDelta,
            WidgetStreamPayload {
                text: Some(text.to_string()),
                ..Default::default()
            },
        );
    }

    pub(crate) fn emit_status(&self, status: &str) {
        self.emit(
            WidgetStreamKind::Status,
            WidgetStreamPayload {
                status: Some(status.to_string()),
                ..Default::default()
            },
        );
    }

    pub(crate) fn emit_final(&self, data: &Value, workflow_run_id: Option<&str>) {
        self.emit(
            WidgetStreamKind::Final,
            WidgetStreamPayload {
                final_data: Some(data.clone()),
                workflow_run_id: workflow_run_id.map(str::to_string),
                ..Default::default()
            },
        );
        // After a terminal event our run is no longer "active". Remove
        // the registry entry only if we still own it — a newer refresh
        // may have claimed it already, in which case we leave it alone.
        self.clear_if_current();
    }

    pub(crate) fn emit_failed(&self, error: &str, partial_text: Option<&str>) {
        self.emit(
            WidgetStreamKind::Failed,
            WidgetStreamPayload {
                error: Some(error.to_string()),
                partial_text: partial_text.map(str::to_string),
                ..Default::default()
            },
        );
        self.clear_if_current();
    }

    fn clear_if_current(&self) {
        self.registry
            .remove_if(&self.widget_id, |_, value| value == &self.refresh_run_id);
    }
}

/// W25/W36: resolve and persist the dashboard's parameter selections
/// once per refresh so every consumer widget sees the same dropdown
/// state and produces an identical parameter fingerprint.
async fn resolve_dashboard_parameter_selections(
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
) -> std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue> {
    if dashboard.parameters.is_empty() {
        std::collections::BTreeMap::new()
    } else {
        state
            .storage
            .get_dashboard_parameter_values(&dashboard.id)
            .await
            .unwrap_or_default()
    }
}

/// W40: load a workflow by id (preferring storage, falling back to the
/// dashboard's inline workflow list) and apply parameter substitution
/// onto the returned clone. Substitution errors leave the workflow
/// untouched so the existing W25 graceful-degrade behavior is
/// preserved.
async fn load_substituted_workflow(
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
    workflow_id: &str,
    parameter_selections: &std::collections::BTreeMap<
        String,
        crate::models::dashboard::ParameterValue,
    >,
    parameters: &[crate::models::dashboard::DashboardParameter],
) -> AnyResult<Workflow> {
    let mut workflow = match state.storage.get_workflow(workflow_id).await? {
        Some(workflow) => workflow,
        None => dashboard
            .workflows
            .iter()
            .find(|workflow| workflow.id == workflow_id)
            .cloned()
            .ok_or_else(|| anyhow!("Datasource workflow not found"))?,
    };
    if !parameters.is_empty() {
        if let Ok(resolved) = ResolvedParameters::resolve(parameters, parameter_selections) {
            parameter_engine::substitute_workflow(
                &mut workflow,
                &resolved,
                SubstituteOptions::default(),
            );
        }
    }
    Ok(workflow)
}

/// W40: shape one widget's runtime data from a workflow's `node_results`
/// and run all per-widget side effects (snapshot, trace capture, alert
/// evaluation, pending reflection). Pulled out of
/// `refresh_widget_inner` so the batched dashboard refresh path can
/// reuse it across multiple consumers of one shared workflow run
/// without re-executing the workflow.
#[allow(clippy::too_many_arguments)]
async fn finalize_widget_refresh(
    app: AppHandle,
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
    widget: &Widget,
    datasource: &DatasourceConfig,
    workflow_id: &str,
    run: &crate::models::workflow::WorkflowRun,
    node_results: &Value,
    parameter_selections: &std::collections::BTreeMap<
        String,
        crate::models::dashboard::ParameterValue,
    >,
    provider: Option<&crate::models::provider::LLMProvider>,
    stream_ctx: Option<&WidgetStreamContext>,
) -> AnyResult<Value> {
    if datasource
        .post_process
        .as_ref()
        .is_some_and(|steps| !steps.is_empty())
    {
        return Err(anyhow!(
            "Widget post_process steps are unavailable in the MVP vertical slice"
        ));
    }

    let output = extract_output(node_results, &datasource.output_key)
        .ok_or_else(|| anyhow!("Workflow output '{}' not found", datasource.output_key))?;

    // W47: resolve the assistant language directive once per refresh so
    // both the streaming and trace tail paths inject the same prompt
    // suffix. Dashboard override wins over the app default; session
    // scope doesn't apply to widget refresh.
    let language_directive = crate::commands::language::resolve_effective_language(
        state.storage.as_ref(),
        Some(dashboard.id.as_str()),
        None,
    )
    .await
    .ok()
    .and_then(|resolved| resolved.system_directive());

    // W31.1: per-widget tail pipeline runs against this consumer's
    // copy of the workflow output. Empty tail short-circuits; non-empty
    // tails go through the same pipeline engine as workflow transforms
    // so pluck / map / aggregate / mcp_call / llm_postprocess remain
    // consistent. W40: the tail is the only thing that varies across
    // shared-workflow consumers, so paying it per widget is expected.
    //
    // W42: if this widget is a Text widget and the tail terminates in
    // `LlmPostprocess { expect: text }`, route through the streaming
    // pipeline runner so the user sees reasoning + text deltas while
    // the provider generates. Other shapes fall back to the blocking
    // path because partial deltas have no useful interpretation.
    let shaped_output = if datasource.tail_pipeline.is_empty() {
        output.clone()
    } else if let Some(ctx) =
        stream_ctx.filter(|_| tail_supports_text_streaming(widget, &datasource.tail_pipeline))
    {
        let partial_acc = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let (final_value_result, _) = {
            let partial_acc = partial_acc.clone();
            crate::modules::workflow_engine::run_pipeline_with_streaming(
                output.clone(),
                &datasource.tail_pipeline,
                Some(state.ai_engine.as_ref()),
                provider,
                Some(state.mcp_manager.as_ref()),
                language_directive.as_deref(),
                |event| match event {
                    crate::modules::workflow_engine::PipelineStreamEvent::StepStarted {
                        ..
                    } => {}
                    crate::modules::workflow_engine::PipelineStreamEvent::ReasoningDelta(text) => {
                        ctx.emit_reasoning_delta(&text);
                    }
                    crate::modules::workflow_engine::PipelineStreamEvent::TextDelta(text) => {
                        if let Ok(mut guard) = partial_acc.lock() {
                            guard.push_str(&text);
                        }
                        ctx.emit_text_delta(&text);
                    }
                    crate::modules::workflow_engine::PipelineStreamEvent::NonStreamingProgress => {
                        ctx.emit_status(
                            "provider does not support streaming; waiting for final response",
                        );
                    }
                },
            )
            .await
        };
        match final_value_result {
            Ok(value) => value,
            Err(error) => {
                let partial = partial_acc
                    .lock()
                    .ok()
                    .map(|guard| guard.clone())
                    .filter(|text| !text.is_empty());
                ctx.emit_failed(&error.to_string(), partial.as_deref());
                return Err(error);
            }
        }
    } else {
        let (final_value, _) = crate::modules::workflow_engine::run_pipeline_with_trace(
            output.clone(),
            &datasource.tail_pipeline,
            Some(state.ai_engine.as_ref()),
            provider,
            Some(state.mcp_manager.as_ref()),
            language_directive.as_deref(),
        )
        .await;
        final_value
    };
    let data = widget_runtime_data(widget, &shaped_output)?;

    let dashboard_id = dashboard.id.as_str();
    let widget_id = widget.id();

    // W36: persist the rendered runtime value as the widget's "last
    // known good" snapshot. Best-effort — a failed persist must not
    // fail the live refresh.
    let snapshot = WidgetRuntimeSnapshot {
        dashboard_id: dashboard_id.to_string(),
        widget_id: widget_id.to_string(),
        widget_kind: widget_kind_for_log(widget).to_string(),
        runtime_data: data.clone(),
        captured_at: chrono::Utc::now().timestamp_millis(),
        workflow_id: Some(workflow_id.to_string()),
        workflow_run_id: Some(run.id.clone()),
        datasource_definition_id: datasource.datasource_definition_id.clone(),
        config_fingerprint: widget_config_fingerprint(widget),
        parameter_fingerprint: parameter_values_fingerprint(parameter_selections),
    };
    if let Err(error) = state.storage.upsert_widget_snapshot(&snapshot).await {
        tracing::warn!(
            "snapshot persist failed for {}/{}: {}",
            dashboard_id,
            widget_id,
            error
        );
    }

    // W23: capture pipeline trace if the widget opted in.
    if datasource.capture_traces {
        crate::commands::debug::capture_trace_after_refresh(state, dashboard_id, widget_id).await;
    }

    // W21: evaluate alerts against the rendered runtime data.
    if let Err(error) = evaluate_widget_alerts(
        &app,
        state,
        dashboard_id,
        widget_id,
        widget.title(),
        &data,
        Some(&run.id),
    )
    .await
    {
        tracing::warn!(
            "alert evaluation failed for widget {}: {}",
            widget_id,
            error
        );
    }

    // W18: post-apply reflection turn if one was queued.
    if let Some((widget_id_key, pending)) = state.pending_reflections.remove(widget_id) {
        const REFLECTION_STALENESS_MS: i64 = 5 * 60 * 1000;
        if chrono::Utc::now().timestamp_millis() - pending.applied_at < REFLECTION_STALENESS_MS {
            crate::commands::chat::enqueue_reflection_turn(
                app.clone(),
                state.inner().clone(),
                pending,
                data.clone(),
            );
        } else {
            tracing::info!(
                "skipping post-apply reflection for stale widget {}",
                widget_id_key
            );
        }
    }

    Ok(data)
}

/// W21: post-refresh alert pass. Walks every alert configured for
/// `widget_id`, applies cooldown, persists firing events, emits a UI
/// event + OS notification, and (for autonomous triggers) spawns a
/// background chat session capped by `max_runs_per_day`.
async fn evaluate_widget_alerts(
    app: &AppHandle,
    state: &State<'_, AppState>,
    dashboard_id: &str,
    widget_id: &str,
    widget_title: &str,
    data: &Value,
    workflow_run_id: Option<&str>,
) -> AnyResult<()> {
    let alerts = state.storage.get_widget_alerts(widget_id).await?;
    if alerts.is_empty() {
        return Ok(());
    }
    let last_fired = state.storage.last_fired_at_for_widget(widget_id).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let fired = alert_engine::evaluate(&alerts, data, &last_fired, now);
    if fired.is_empty() {
        return Ok(());
    }

    for hit in fired {
        // 1) Optionally spawn the autonomous turn first so we can record
        //    the session id alongside the event. Budget = number of
        //    autonomous spawns for this alert in the last 24h.
        let mut triggered_session_id: Option<String> = None;
        if let Some(action) = hit.alert.agent_action.clone() {
            let since = now - 24 * 60 * 60 * 1000;
            let already_spawned = state
                .storage
                .count_agent_actions_in_window(&hit.alert.id, since)
                .await
                .unwrap_or(0);
            if (already_spawned as u32) < action.max_runs_per_day {
                let prompt = render_agent_prompt(
                    &action.prompt_template,
                    widget_title,
                    &hit.message,
                    &hit.context,
                );
                let title = format!("[alert] {}", hit.alert.name);
                match crate::commands::chat::spawn_autonomous_alert_turn(
                    app.clone(),
                    state.inner().clone(),
                    action.mode,
                    Some(dashboard_id.to_string()),
                    Some(widget_id.to_string()),
                    title,
                    prompt,
                    Some(action.max_cost_usd),
                )
                .await
                {
                    Ok(session_id) => triggered_session_id = Some(session_id),
                    Err(error) => tracing::warn!(
                        "autonomous alert turn failed for alert {}: {}",
                        hit.alert.id,
                        error
                    ),
                }
            } else {
                tracing::info!(
                    "autonomous alert turn skipped for {} — daily budget {} exhausted",
                    hit.alert.id,
                    action.max_runs_per_day
                );
            }
        }

        // 2) Persist the event.
        let event = AlertEvent {
            id: uuid::Uuid::new_v4().to_string(),
            widget_id: widget_id.to_string(),
            dashboard_id: dashboard_id.to_string(),
            alert_id: hit.alert.id.clone(),
            fired_at: now,
            severity: hit.severity,
            message: hit.message.clone(),
            context: hit.context.clone(),
            acknowledged_at: None,
            triggered_session_id: triggered_session_id.clone(),
            workflow_run_id: workflow_run_id.map(str::to_string),
        };
        if let Err(error) = state.storage.insert_alert_event(&event).await {
            tracing::warn!("failed to persist alert event {}: {}", event.id, error);
            continue;
        }

        // 3) Emit UI event + fire OS notification. Failure here is
        //    non-fatal; the event is already in the DB so the UI will
        //    catch up on the next refresh.
        if let Err(error) = app.emit(ALERT_EVENT_CHANNEL, &event) {
            tracing::warn!("failed to emit alert event: {}", error);
        }
        let notify_title = format!("{} • {}", widget_title, hit.alert.name);
        if let Err(error) = app
            .notification()
            .builder()
            .title(notify_title)
            .body(hit.message.clone())
            .show()
        {
            tracing::warn!("OS notification failed for alert {}: {}", event.id, error);
        }
    }
    Ok(())
}

fn render_agent_prompt(
    template: &str,
    widget_title: &str,
    message: &str,
    context: &Value,
) -> String {
    let value = context
        .get("value")
        .map(value_to_string)
        .unwrap_or_default();
    let path = context
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let threshold = context
        .get("threshold")
        .map(value_to_string)
        .unwrap_or_default();
    let mut out = template.to_string();
    for (placeholder, replacement) in [
        ("{widget}", widget_title),
        ("{message}", message),
        ("{value}", value.as_str()),
        ("{path}", path.as_str()),
        ("{threshold}", threshold.as_str()),
    ] {
        out = out.replace(placeholder, replacement);
    }
    if out.trim().is_empty() {
        return format!(
            "Alert fired on widget \"{}\". Message: {}. Suggest next steps.",
            widget_title, message
        );
    }
    out
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) async fn active_provider_public(
    state: &State<'_, AppState>,
) -> AnyResult<Option<crate::models::provider::LLMProvider>> {
    active_provider(state).await
}

/// W43: resolve the widget-effective LLM model for one refresh.
///
/// Surfaces typed [`crate::models::provider::WidgetModelError`] codes
/// when the policy provider is missing, disabled, mis-configured, or
/// when the requested model lacks a required capability — exactly the
/// "fail closed with remediation" behaviour the W43 spec asks for.
/// Returns `Ok(None)` when no dashboard/widget policy and no app active
/// provider exist; deterministic widgets behave exactly as they did
/// before W43 in that case.
pub(crate) async fn resolve_widget_effective_model(
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
    widget: &Widget,
    app_provider: Option<&crate::models::provider::LLMProvider>,
) -> AnyResult<Option<crate::models::provider::EffectiveWidgetModel>> {
    let providers = state.storage.list_providers().await?;
    crate::modules::ai::resolve_effective_widget_model(widget, dashboard, &providers, app_provider)
        .map_err(|error| anyhow!(error.to_string()))
}

pub(crate) async fn active_provider(
    state: &State<'_, AppState>,
) -> AnyResult<Option<crate::models::provider::LLMProvider>> {
    // W29: dashboard refresh / scheduling does not have a chat send
    // surface to relay a typed correction state on. Return `None` when
    // resolution fails; pipeline / workflow nodes that actually need
    // an LLM step will report a visible per-widget error.
    match crate::resolve_active_provider(state.storage.as_ref()).await? {
        Ok(provider) => Ok(Some(provider)),
        Err(_setup_error) => Ok(None),
    }
}

pub(crate) async fn reconnect_enabled_mcp_servers(state: &State<'_, AppState>) -> AnyResult<()> {
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

pub(crate) async fn schedule_workflow_if_cron(
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
        | Widget::Heatmap { x, y, w, h, .. }
        | Widget::Gallery { x, y, w, h, .. } => WidgetPosition {
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
        | Widget::Heatmap { x, y, w, h, .. }
        | Widget::Gallery { x, y, w, h, .. } => {
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
        | Widget::Heatmap { datasource, .. }
        | Widget::Gallery { datasource, .. } => datasource.as_ref().map(|d| d.workflow_id.clone()),
    }
}

pub(crate) async fn drop_workflow(
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
        | Widget::Heatmap { y, h, .. }
        | Widget::Gallery { y, h, .. } => y + h,
    }
}

/// W42: a tail pipeline can stream text deltas iff (a) the widget is a
/// Text widget — partial deltas only have a meaningful rendering for
/// markdown/plain text — and (b) the terminal step is
/// `LlmPostprocess { expect: text }`. Aggregating tables/charts whose
/// shape comes from a deterministic step don't gain anything by
/// streaming the intermediate LLM step because the final value still
/// has to be re-shaped after.
pub(crate) fn tail_supports_text_streaming(
    widget: &Widget,
    tail: &[crate::models::pipeline::PipelineStep],
) -> bool {
    use crate::models::pipeline::{LlmExpect, PipelineStep};
    if !matches!(widget, Widget::Text { .. }) {
        return false;
    }
    matches!(
        tail.last(),
        Some(PipelineStep::LlmPostprocess {
            expect: LlmExpect::Text,
            ..
        })
    )
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
        Widget::Gallery { .. } => "gallery",
    }
}

/// W36: fingerprint the parts of a widget that influence the *shape* of
/// its rendered runtime data — variant + datasource binding + tail
/// pipeline + saved-definition link. Layout (x/y/w/h), title, and
/// presentation config (axis labels, colors, etc.) are deliberately
/// excluded: changing them keeps the cached snapshot valid because
/// `widget_runtime_data` already strips them out of its output.
pub(crate) fn widget_config_fingerprint(widget: &Widget) -> String {
    let datasource_part = widget
        .datasource()
        .map(|ds| {
            serde_json::json!({
                "workflow_id": ds.workflow_id,
                "output_key": ds.output_key,
                "tail_pipeline": ds.tail_pipeline,
                "datasource_definition_id": ds.datasource_definition_id,
            })
        })
        .unwrap_or(serde_json::Value::Null);
    hash_value(&serde_json::json!({
        "kind": widget_kind_for_log(widget),
        "datasource": datasource_part,
    }))
}

/// W36: fingerprint the dashboard's resolved parameter values. Any
/// dropdown change shifts this hash so every snapshot in the dashboard
/// is dropped on next hydrate — a stale value never paints over a
/// fresh selection. Empty maps fingerprint identically across widgets.
pub(crate) fn parameter_values_fingerprint(
    values: &std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue>,
) -> String {
    let canonical = serde_json::to_value(values).unwrap_or(serde_json::Value::Null);
    hash_value(&canonical)
}

fn hash_value(value: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(value).unwrap_or_default();
    let mut hasher = Sha1::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
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
        Widget::Gallery { config, .. } => {
            let items = gallery_items(normalized).ok_or_else(|| {
                anyhow!(
                    "Gallery workflow output must be an array of items with an image url/src/path or an object containing 'items'"
                )
            })?;
            let max = config.max_visible_items as usize;
            let trimmed = if max > 0 && items.len() > max {
                items.into_iter().take(max).collect::<Vec<_>>()
            } else {
                items
            };
            if trimmed.is_empty() {
                return Err(anyhow!(
                    "Gallery workflow output produced no usable image items"
                ));
            }
            Ok(json!({ "kind": "gallery", "items": trimmed }))
        }
    }
}

/// W44: deterministic coercion of pipeline output into typed gallery
/// item objects. Accepts a top-level array, an object wrapping `items`,
/// or a single object (rendered as a single-item gallery). Items can be
/// bare URL strings or objects with any of `src`/`url`/`image`/`path`
/// plus optional caption/title/alt/source/link fields. Items that do
/// not produce a usable image source are dropped so a partial result
/// still renders rather than failing closed.
fn gallery_items(value: &Value) -> Option<Vec<Value>> {
    let raw_array = value
        .as_array()
        .cloned()
        .or_else(|| value.get("items").and_then(Value::as_array).cloned())
        .or_else(|| value.get("images").and_then(Value::as_array).cloned())
        .or_else(|| {
            if value.is_object() {
                Some(vec![value.clone()])
            } else if let Some(s) = value.as_str() {
                Some(vec![Value::String(s.to_string())])
            } else {
                None
            }
        })?;
    let mut items = Vec::new();
    for raw in raw_array {
        if let Some(item) = gallery_item(&raw) {
            items.push(item);
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn gallery_item(value: &Value) -> Option<Value> {
    if let Some(src) = value.as_str() {
        let trimmed = src.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({ "src": trimmed }));
    }
    let obj = value.as_object()?;
    let src = obj
        .get("src")
        .or_else(|| obj.get("url"))
        .or_else(|| obj.get("image"))
        .or_else(|| obj.get("path"))
        .or_else(|| obj.get("thumbnail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())?;
    let mut out = serde_json::Map::new();
    out.insert("src".to_string(), Value::String(src));
    if let Some(s) = obj
        .get("title")
        .or_else(|| obj.get("caption"))
        .or_else(|| obj.get("name"))
        .and_then(Value::as_str)
    {
        out.insert("title".to_string(), Value::String(s.to_string()));
    }
    if let Some(s) = obj
        .get("caption")
        .or_else(|| obj.get("description"))
        .or_else(|| obj.get("summary"))
        .and_then(Value::as_str)
    {
        out.insert("caption".to_string(), Value::String(s.to_string()));
    }
    if let Some(s) = obj.get("alt").and_then(Value::as_str) {
        out.insert("alt".to_string(), Value::String(s.to_string()));
    }
    if let Some(s) = obj
        .get("source")
        .or_else(|| obj.get("attribution"))
        .or_else(|| obj.get("provider"))
        .and_then(Value::as_str)
    {
        out.insert("source".to_string(), Value::String(s.to_string()));
    }
    if let Some(s) = obj
        .get("link")
        .or_else(|| obj.get("href"))
        .or_else(|| obj.get("page"))
        .and_then(Value::as_str)
    {
        out.insert("link".to_string(), Value::String(s.to_string()));
    }
    if let Some(s) = obj.get("id").and_then(|v| v.as_str().map(str::to_string)) {
        out.insert("id".to_string(), Value::String(s));
    }
    Some(Value::Object(out))
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
        pause_state: Default::default(),
        last_paused_at: None,
        last_pause_reason: None,
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        pause_state: Default::default(),
        last_paused_at: None,
        last_pause_reason: None,
        last_run: None,
        created_at: now,
        updated_at: now,
    }
}

// ─── W19: snapshot helper + version commands ─────────────────────────────────

/// Persist a snapshot of `dashboard` before any state-changing mutation.
/// Returns the new version row's summary (the inserted `id` is also
/// referenced through that summary). Callers pass `parent_version_id` for
/// restores so the heuristic in W18 reflection can correlate them.
async fn record_dashboard_version(
    state: &State<'_, AppState>,
    dashboard: &Dashboard,
    source: VersionSource,
    summary: &str,
    source_session_id: Option<&str>,
    parent_version_id: Option<&str>,
) -> AnyResult<DashboardVersionSummary> {
    let version_id = uuid::Uuid::new_v4().to_string();
    let applied_at = chrono::Utc::now().timestamp_millis();
    let saved = state
        .storage
        .insert_dashboard_version(
            &version_id,
            dashboard,
            source,
            summary,
            source_session_id,
            parent_version_id,
            applied_at,
        )
        .await?;
    Ok(saved)
}

fn proposal_summary(req: &ApplyBuildProposalRequest) -> String {
    if let Some(summary) = req
        .proposal
        .summary
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return summary.trim().to_string();
    }
    let title = req.proposal.title.trim();
    if !title.is_empty() {
        return format!("Agent apply: {title}");
    }
    let added = req.proposal.widgets.len();
    let removed = req.proposal.remove_widget_ids.len();
    format!("Agent apply (+{added}/-{removed})")
}

#[tauri::command]
pub async fn list_dashboard_versions(
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<ApiResult<Vec<DashboardVersionSummary>>, String> {
    Ok(
        match state.storage.list_dashboard_versions(&dashboard_id).await {
            Ok(versions) => ApiResult::ok(versions),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn get_dashboard_version(
    state: State<'_, AppState>,
    version_id: String,
) -> Result<ApiResult<DashboardVersion>, String> {
    Ok(
        match state.storage.get_dashboard_version(&version_id).await {
            Ok(Some(version)) => ApiResult::ok(version),
            Ok(None) => ApiResult::err("Version not found".to_string()),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn diff_dashboard_versions(
    state: State<'_, AppState>,
    from_id: String,
    to_id: String,
) -> Result<ApiResult<DashboardDiff>, String> {
    Ok(match diff_versions_inner(&state, &from_id, &to_id).await {
        Ok(diff) => ApiResult::ok(diff),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn diff_versions_inner(
    state: &State<'_, AppState>,
    from_id: &str,
    to_id: &str,
) -> AnyResult<DashboardDiff> {
    let from = state
        .storage
        .get_dashboard_version(from_id)
        .await?
        .ok_or_else(|| anyhow!("from version not found"))?;
    let to = state
        .storage
        .get_dashboard_version(to_id)
        .await?
        .ok_or_else(|| anyhow!("to version not found"))?;
    Ok(compute_dashboard_diff(
        &from.snapshot,
        &to.snapshot,
        from_id,
        to_id,
    ))
}

#[tauri::command]
pub async fn restore_dashboard_version(
    app: AppHandle,
    state: State<'_, AppState>,
    version_id: String,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(
        match restore_dashboard_version_inner(&app, &state, &version_id).await {
            Ok(dashboard) => ApiResult::ok(dashboard),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn restore_dashboard_version_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    version_id: &str,
) -> AnyResult<Dashboard> {
    let target = state
        .storage
        .get_dashboard_version(version_id)
        .await?
        .ok_or_else(|| anyhow!("Version not found"))?;
    let current = state
        .storage
        .get_dashboard(&target.dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard no longer exists; cannot restore"))?;

    record_dashboard_version(
        state,
        &current,
        VersionSource::Restore,
        &format!("Restored from {}", short_version_id(version_id)),
        None,
        Some(version_id),
    )
    .await?;

    let now = chrono::Utc::now().timestamp_millis();
    let mut restored = target.snapshot.clone();
    restored.updated_at = now;

    apply_workflow_swap(app, state, &current, &restored).await?;
    state.storage.update_dashboard(&restored).await?;

    Ok(restored)
}

fn short_version_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Make storage + scheduler match `restored.workflows` exactly. Workflows
/// only in the current dashboard are unscheduled + deleted; workflows in
/// the restored snapshot are upserted and rescheduled if they have a cron.
/// Errors on unschedule/delete are tolerated so a stale scheduler entry
/// does not block a restore.
async fn apply_workflow_swap(
    app: &AppHandle,
    state: &State<'_, AppState>,
    current: &Dashboard,
    restored: &Dashboard,
) -> AnyResult<()> {
    let restored_ids: std::collections::HashSet<&str> =
        restored.workflows.iter().map(|w| w.id.as_str()).collect();

    for workflow in &current.workflows {
        if !restored_ids.contains(workflow.id.as_str()) {
            let _ = state.scheduler.lock().await.unschedule(&workflow.id).await;
            if let Err(error) = state.storage.delete_workflow(&workflow.id).await {
                tracing::warn!(
                    "restore: failed to delete workflow {}: {}",
                    workflow.id,
                    error
                );
            }
        }
    }

    for workflow in &restored.workflows {
        state.storage.upsert_workflow(workflow).await?;
        let _ = state.scheduler.lock().await.unschedule(&workflow.id).await;
        schedule_workflow_if_cron(app, state, workflow.clone()).await?;
    }
    Ok(())
}

fn compute_dashboard_diff(
    from: &Dashboard,
    to: &Dashboard,
    from_id: &str,
    to_id: &str,
) -> DashboardDiff {
    use std::collections::HashMap;

    let from_widgets: HashMap<&str, &Widget> = from.layout.iter().map(|w| (w.id(), w)).collect();
    let to_widgets: HashMap<&str, &Widget> = to.layout.iter().map(|w| (w.id(), w)).collect();

    let added_widgets = to
        .layout
        .iter()
        .filter(|w| !from_widgets.contains_key(w.id()))
        .map(widget_summary)
        .collect::<Vec<_>>();
    let removed_widgets = from
        .layout
        .iter()
        .filter(|w| !to_widgets.contains_key(w.id()))
        .map(widget_summary)
        .collect::<Vec<_>>();

    let mut modified_widgets = Vec::new();
    for (id, from_w) in &from_widgets {
        if let Some(to_w) = to_widgets.get(*id) {
            if let Some(diff) = widget_diff(from_w, to_w) {
                modified_widgets.push(diff);
            }
        }
    }

    let name_changed = if from.name != to.name {
        Some((from.name.clone(), to.name.clone()))
    } else {
        None
    };
    let description_changed = if from.description != to.description {
        Some((from.description.clone(), to.description.clone()))
    } else {
        None
    };

    let layout_changed = from.layout.len() != to.layout.len()
        || from.layout.iter().any(|fw| {
            to_widgets.get(fw.id()).is_none_or(|tw| {
                let fp = existing_position(fw);
                let tp = existing_position(tw);
                fp.x != tp.x || fp.y != tp.y || fp.w != tp.w || fp.h != tp.h
            })
        });

    DashboardDiff {
        from_version_id: from_id.to_string(),
        to_version_id: to_id.to_string(),
        added_widgets,
        removed_widgets,
        modified_widgets,
        name_changed,
        description_changed,
        layout_changed,
    }
}

fn widget_summary(widget: &Widget) -> WidgetSummary {
    WidgetSummary {
        id: widget.id().to_string(),
        title: widget.title().to_string(),
        kind: widget_kind_label(widget).to_string(),
    }
}

fn widget_diff(from: &Widget, to: &Widget) -> Option<WidgetDiff> {
    let from_kind = widget_kind_label(from).to_string();
    let to_kind = widget_kind_label(to).to_string();
    let kind_changed = if from_kind != to_kind {
        Some((from_kind.clone(), to_kind.clone()))
    } else {
        None
    };
    let title_changed = if from.title() != to.title() {
        Some((from.title().to_string(), to.title().to_string()))
    } else {
        None
    };

    let from_value = serde_json::to_value(from).unwrap_or(Value::Null);
    let to_value = serde_json::to_value(to).unwrap_or(Value::Null);
    let mut config_changes = Vec::new();
    if let (Some(from_obj), Some(to_obj)) = (from_value.get("config"), to_value.get("config")) {
        diff_json("config", from_obj, to_obj, &mut config_changes);
    }
    let datasource_plan_changed = from_value.get("datasource") != to_value.get("datasource");

    // W31: split the datasource diff into "identity" (workflow_id,
    // definition_id, output_key) and "tail" (post_process, capture
    // traces, provenance metadata) so the UI can call them out
    // separately.
    let (binding_changed, tail_changed) =
        match (from_widget_datasource(from), to_widget_datasource(to)) {
            (None, None) => (false, false),
            (None, Some(_)) | (Some(_), None) => (true, false),
            (Some(a), Some(b)) => {
                let identity_changed = a.workflow_id != b.workflow_id
                    || a.output_key != b.output_key
                    || a.datasource_definition_id != b.datasource_definition_id;
                let tail_only = !identity_changed
                    && (serde_json::to_value(&a.post_process).unwrap_or(Value::Null)
                        != serde_json::to_value(&b.post_process).unwrap_or(Value::Null)
                        || a.capture_traces != b.capture_traces
                        || a.binding_source != b.binding_source);
                (identity_changed, tail_only)
            }
        };

    if kind_changed.is_none()
        && title_changed.is_none()
        && config_changes.is_empty()
        && !datasource_plan_changed
    {
        return None;
    }

    Some(WidgetDiff {
        widget_id: from.id().to_string(),
        widget_title: to.title().to_string(),
        kind_changed,
        title_changed,
        config_changes,
        datasource_plan_changed,
        binding_changed,
        tail_changed,
    })
}

fn from_widget_datasource(w: &Widget) -> Option<&DatasourceConfig> {
    crate::commands::datasource::widget_datasource(w)
}
fn to_widget_datasource(w: &Widget) -> Option<&DatasourceConfig> {
    crate::commands::datasource::widget_datasource(w)
}

/// Recursive JSON-Pointer-style diff. Records a leaf change whenever
/// values differ; for objects, also reports keys present on only one side.
fn diff_json(path: &str, from: &Value, to: &Value, out: &mut Vec<JsonPathChange>) {
    if from == to {
        return;
    }
    match (from, to) {
        (Value::Object(from_map), Value::Object(to_map)) => {
            let mut keys: std::collections::BTreeSet<&String> = from_map.keys().collect();
            keys.extend(to_map.keys());
            for key in keys {
                let next_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let from_v = from_map.get(key).cloned().unwrap_or(Value::Null);
                let to_v = to_map.get(key).cloned().unwrap_or(Value::Null);
                diff_json(&next_path, &from_v, &to_v, out);
            }
        }
        _ => {
            out.push(JsonPathChange {
                path: path.to_string(),
                before: from.clone(),
                after: to.clone(),
            });
        }
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
            | Widget::Heatmap { datasource, .. }
            | Widget::Gallery { datasource, .. } => datasource.as_ref(),
        }
    }
}

// ─── W25: Dashboard parameter commands ──────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DashboardParameterState {
    pub parameter: crate::models::dashboard::DashboardParameter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<crate::models::dashboard::ParameterValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<crate::models::dashboard::ParameterOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_error: Option<String>,
}

#[tauri::command]
pub async fn list_dashboard_parameters(
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<ApiResult<Vec<DashboardParameterState>>, String> {
    let result = list_dashboard_parameters_inner(&state, &dashboard_id).await;
    Ok(match result {
        Ok(state) => ApiResult::ok(state),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn list_dashboard_parameters_inner(
    state: &State<'_, AppState>,
    dashboard_id: &str,
) -> AnyResult<Vec<DashboardParameterState>> {
    let dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await?
        .ok_or_else(|| anyhow!("Dashboard not found"))?;
    let selections = state
        .storage
        .get_dashboard_parameter_values(dashboard_id)
        .await
        .unwrap_or_default();
    // W34: resolve every parameter's options through the runtime so
    // mcp_query / http_query / datasource_query produce real dropdowns.
    // The helper iterates in topological order so cascading selectors
    // can substitute upstream values into their queries.
    let (options_map, errors_map, _resolved) =
        crate::modules::parameter_options::resolve_all_parameter_options(
            state,
            &dashboard.parameters,
            &selections,
        )
        .await;
    let mut out = Vec::with_capacity(dashboard.parameters.len());
    for param in &dashboard.parameters {
        let value = selections.get(&param.name).cloned();
        let options = options_map.get(&param.name).cloned().unwrap_or_default();
        let options_error = errors_map.get(&param.name).cloned();
        out.push(DashboardParameterState {
            parameter: param.clone(),
            value,
            options,
            options_error,
        });
    }
    Ok(out)
}

/// W34: re-resolve options for one parameter using the dashboard's current
/// selections as upstream context. Used by the UI when a user explicitly
/// clicks "refresh options" without changing any value.
#[tauri::command]
pub async fn refresh_dashboard_parameter_options(
    state: State<'_, AppState>,
    dashboard_id: String,
    param_name: String,
) -> Result<ApiResult<DashboardParameterState>, String> {
    let outcome = async {
        let dashboard = state
            .storage
            .get_dashboard(&dashboard_id)
            .await?
            .ok_or_else(|| anyhow!("Dashboard not found"))?;
        let param = dashboard
            .parameters
            .iter()
            .find(|p| p.name == param_name)
            .ok_or_else(|| anyhow!("Parameter '{}' not declared on dashboard", param_name))?
            .clone();
        let selections = state
            .storage
            .get_dashboard_parameter_values(&dashboard_id)
            .await
            .unwrap_or_default();
        let upstream = ResolvedParameters::resolve(&dashboard.parameters, &selections)
            .unwrap_or_else(|_| ResolvedParameters::from_map(selections.clone()));
        let value = selections.get(&param.name).cloned();
        let (options, options_error) =
            match crate::modules::parameter_options::resolve_options_for_parameter(
                &state, &param, &upstream,
            )
            .await
            {
                Ok(opts) => (opts, None),
                Err(error) => (Vec::new(), Some(error.to_string())),
            };
        Ok::<_, anyhow::Error>(DashboardParameterState {
            parameter: param,
            value,
            options,
            options_error,
        })
    }
    .await;
    Ok(match outcome {
        Ok(payload) => ApiResult::ok(payload),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

#[tauri::command]
pub async fn get_dashboard_parameter_values(
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<
    ApiResult<std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue>>,
    String,
> {
    Ok(
        match state
            .storage
            .get_dashboard_parameter_values(&dashboard_id)
            .await
        {
            Ok(values) => ApiResult::ok(values),
            Err(error) => ApiResult::err(error.to_string()),
        },
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetDashboardParameterResult {
    pub affected_widget_ids: Vec<String>,
    /// W34: dependent parameters re-resolved against the new selection.
    /// Empty when no other parameter declared `depends_on: [param_name]`.
    /// The UI merges these states back into its local map so cascading
    /// selectors update without re-listing every parameter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downstream: Vec<DashboardParameterState>,
}

#[tauri::command]
pub async fn set_dashboard_parameter_value(
    state: State<'_, AppState>,
    dashboard_id: String,
    param_name: String,
    value: crate::models::dashboard::ParameterValue,
) -> Result<ApiResult<SetDashboardParameterResult>, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let outcome = async {
        state
            .storage
            .set_dashboard_parameter_value(&dashboard_id, &param_name, &value, now)
            .await?;
        // Compute affected widgets by walking every widget's datasource
        // workflow for `$param_name` references.
        let dashboard = state
            .storage
            .get_dashboard(&dashboard_id)
            .await?
            .ok_or_else(|| anyhow!("Dashboard not found"))?;
        let mut affected = Vec::new();
        for widget in &dashboard.layout {
            let Some(ds) = widget.datasource() else {
                continue;
            };
            let workflow = match state.storage.get_workflow(&ds.workflow_id).await? {
                Some(wf) => wf,
                None => match dashboard
                    .workflows
                    .iter()
                    .find(|wf| wf.id == ds.workflow_id)
                {
                    Some(wf) => wf.clone(),
                    None => continue,
                },
            };
            let mut referenced = std::collections::BTreeSet::new();
            for node in &workflow.nodes {
                if let Some(cfg) = &node.config {
                    let names = ResolvedParameters::referenced_names(cfg);
                    referenced.extend(names);
                }
            }
            if referenced.contains(&param_name) {
                affected.push(widget.id().to_string());
            }
        }

        // W34: re-resolve any downstream parameter that depends on the
        // one that just changed, so cascading selectors update without a
        // full `listParameters` round-trip. The dependents set comes from
        // declared `depends_on` edges; query-backed kinds are the only
        // ones whose options can change, but we re-resolve every
        // dependent uniformly so static_list parents with downstream
        // children also get re-emitted with the freshest selections.
        let dependent_names = crate::modules::parameter_options::downstream_dependents(
            &dashboard.parameters,
            &param_name,
        );
        let mut downstream: Vec<DashboardParameterState> = Vec::new();
        if !dependent_names.is_empty() {
            let selections = state
                .storage
                .get_dashboard_parameter_values(&dashboard_id)
                .await
                .unwrap_or_default();
            let upstream = ResolvedParameters::resolve(&dashboard.parameters, &selections)
                .unwrap_or_else(|_| ResolvedParameters::from_map(selections.clone()));
            for name in dependent_names {
                let Some(param) = dashboard.parameters.iter().find(|p| p.name == name) else {
                    continue;
                };
                let value = selections.get(&param.name).cloned();
                let (options, options_error) =
                    match crate::modules::parameter_options::resolve_options_for_parameter(
                        &state, param, &upstream,
                    )
                    .await
                    {
                        Ok(opts) => (opts, None),
                        Err(error) => (Vec::new(), Some(error.to_string())),
                    };
                downstream.push(DashboardParameterState {
                    parameter: param.clone(),
                    value,
                    options,
                    options_error,
                });
            }
        }
        Ok::<_, anyhow::Error>(SetDashboardParameterResult {
            affected_widget_ids: affected,
            downstream,
        })
    }
    .await;
    Ok(match outcome {
        Ok(payload) => ApiResult::ok(payload),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolveDashboardParametersResult {
    pub values: std::collections::BTreeMap<String, crate::models::dashboard::ParameterValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<Vec<String>>,
}

#[tauri::command]
pub async fn resolve_dashboard_parameters(
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<ApiResult<ResolveDashboardParametersResult>, String> {
    let outcome = async {
        let dashboard = state
            .storage
            .get_dashboard(&dashboard_id)
            .await?
            .ok_or_else(|| anyhow!("Dashboard not found"))?;
        if let Some(cycle) = crate::modules::parameter_engine::detect_cycle(&dashboard.parameters) {
            return Ok(ResolveDashboardParametersResult {
                values: Default::default(),
                cycle: Some(cycle),
            });
        }
        let selections = state
            .storage
            .get_dashboard_parameter_values(&dashboard_id)
            .await
            .unwrap_or_default();
        let resolved = ResolvedParameters::resolve(&dashboard.parameters, &selections)?;
        Ok::<_, anyhow::Error>(ResolveDashboardParametersResult {
            values: resolved.as_map().clone(),
            cycle: None,
        })
    }
    .await;
    Ok(match outcome {
        Ok(payload) => ApiResult::ok(payload),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

#[cfg(test)]
mod gallery_tests {
    use super::*;
    use crate::models::widget::{GalleryAspect, GalleryConfig, GalleryLayout, ImageFit};

    fn sample_gallery_widget() -> Widget {
        Widget::Gallery {
            id: "gal_1".into(),
            title: "test".into(),
            x: 0,
            y: 0,
            w: 8,
            h: 6,
            config: GalleryConfig {
                layout: GalleryLayout::Grid,
                thumbnail_aspect: GalleryAspect::Landscape,
                max_visible_items: 4,
                show_caption: true,
                show_source: false,
                fullscreen_enabled: true,
                fit: ImageFit::Cover,
                border_radius: 4,
            },
            datasource: None,
        }
    }

    #[test]
    fn coerces_string_array_to_gallery_items() {
        let widget = sample_gallery_widget();
        let output = json!(["https://a/1.jpg", "https://a/2.jpg"]);
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        assert_eq!(runtime["kind"], "gallery");
        let items = runtime["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["src"], "https://a/1.jpg");
    }

    #[test]
    fn coerces_object_with_title_caption_and_source() {
        let widget = sample_gallery_widget();
        let output = json!([
            {"url": "https://a/x.jpg", "title": "X", "description": "desc", "attribution": "Wiki"},
            {"image": "https://a/y.jpg", "name": "Y", "page": "https://wiki/Y"},
        ]);
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        let items = runtime["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["src"], "https://a/x.jpg");
        assert_eq!(items[0]["title"], "X");
        assert_eq!(items[0]["caption"], "desc");
        assert_eq!(items[0]["source"], "Wiki");
        assert_eq!(items[1]["src"], "https://a/y.jpg");
        assert_eq!(items[1]["link"], "https://wiki/Y");
    }

    #[test]
    fn drops_items_with_no_image_source() {
        let widget = sample_gallery_widget();
        let output = json!([
            {"title": "no src"},
            {"src": "https://a/ok.jpg"},
        ]);
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        let items = runtime["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["src"], "https://a/ok.jpg");
    }

    #[test]
    fn applies_max_visible_items_cap() {
        let widget = sample_gallery_widget();
        let output = json!([
            "https://a/1.jpg",
            "https://a/2.jpg",
            "https://a/3.jpg",
            "https://a/4.jpg",
            "https://a/5.jpg",
        ]);
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        let items = runtime["items"].as_array().unwrap();
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn unwraps_items_envelope() {
        let widget = sample_gallery_widget();
        let output = json!({"items": [{"src": "https://a/1.jpg"}]});
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        let items = runtime["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn empty_output_falls_back_to_text_kind() {
        // No items at all → strict path errors → fallback widget runtime is a
        // text payload explaining the parse failure.
        let widget = sample_gallery_widget();
        let output = json!([{"title": "no src"}]);
        let runtime = widget_runtime_data(&widget, &output).unwrap();
        assert_eq!(runtime["kind"], "text");
        assert_eq!(runtime["fallback"], true);
    }
}

#[cfg(test)]
mod compose_tests {
    use super::*;
    use crate::models::dashboard::{BuildDatasourcePlan, BuildDatasourcePlanKind, BuildWidgetType};
    use std::collections::BTreeMap;

    fn dummy_proposal() -> BuildWidgetProposal {
        BuildWidgetProposal {
            widget_type: BuildWidgetType::Text,
            title: "compose smoke".into(),
            data: Value::Null,
            datasource_plan: None,
            config: None,
            x: None,
            y: None,
            w: None,
            h: None,
            replace_widget_id: None,
            size_preset: None,
            layout_pattern: None,
        }
    }

    fn http_plan(url: &str) -> BuildDatasourcePlan {
        BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(json!({"method": "GET", "url": url})),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        }
    }

    #[test]
    fn compose_workflow_emits_merge_with_key_map_and_outer_pipeline() {
        let mut inputs: BTreeMap<String, BuildDatasourcePlan> = BTreeMap::new();
        inputs.insert("primary".into(), http_plan("https://example.test/a"));
        inputs.insert("secondary".into(), http_plan("https://example.test/b"));
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: vec![crate::models::pipeline::PipelineStep::Pick {
                path: "primary".into(),
            }],
            source_key: None,
            inputs: Some(inputs),
        };
        let proposal = dummy_proposal();
        let workflow = datasource_plan_workflow("wf-1".into(), "test".into(), &proposal, &plan, 0)
            .expect("compose workflow builds");

        let source_count = workflow
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::McpTool))
            .count();
        assert_eq!(source_count, 2);
        assert!(workflow
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Merge)));
        assert!(workflow
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Output)));

        let merge_node = workflow
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Merge))
            .expect("merge node");
        let cfg = merge_node.config.as_ref().expect("merge config");
        let key_map = cfg.get("key_map").and_then(|v| v.as_object()).unwrap();
        let aliases: std::collections::HashSet<String> = key_map
            .values()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        assert!(aliases.contains("primary"));
        assert!(aliases.contains("secondary"));
    }

    #[test]
    fn compose_rejects_nested_compose() {
        let mut inner_inputs: BTreeMap<String, BuildDatasourcePlan> = BTreeMap::new();
        inner_inputs.insert("a".into(), http_plan("https://example.test/a"));
        let inner = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: Some(inner_inputs),
        };
        let mut inputs: BTreeMap<String, BuildDatasourcePlan> = BTreeMap::new();
        inputs.insert("nested".into(), inner);
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: Some(inputs),
        };
        let proposal = dummy_proposal();
        let err = datasource_plan_workflow("wf-2".into(), "n".into(), &proposal, &plan, 0)
            .expect_err("nested compose must error");
        assert!(err.to_string().contains("nested compose"));
    }

    #[test]
    fn compose_rejects_empty_inputs() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: Some(BTreeMap::new()),
        };
        let proposal = dummy_proposal();
        let err = datasource_plan_workflow("wf-3".into(), "e".into(), &proposal, &plan, 0)
            .expect_err("empty inputs must error");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn compose_rejects_reserved_input_name() {
        let mut inputs: BTreeMap<String, BuildDatasourcePlan> = BTreeMap::new();
        inputs.insert("source".into(), http_plan("https://example.test/x"));
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Compose,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: Some(inputs),
        };
        let proposal = dummy_proposal();
        let err = datasource_plan_workflow("wf-4".into(), "r".into(), &proposal, &plan, 0)
            .expect_err("reserved id must error");
        assert!(err.to_string().contains("reserved"));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::models::dashboard::ParameterValue;
    use crate::models::pipeline::PipelineStep;

    fn gauge_widget(id: &str, workflow_id: &str, output_key: &str) -> Widget {
        Widget::Gauge {
            id: id.to_string(),
            title: "Latency".into(),
            x: 0,
            y: 0,
            w: 4,
            h: 3,
            config: GaugeConfig {
                min: 0.0,
                max: 100.0,
                unit: None,
                thresholds: None,
                show_value: true,
            },
            datasource: Some(DatasourceConfig {
                workflow_id: workflow_id.into(),
                output_key: output_key.into(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: None,
                binding_source: None,
                bound_at: None,
                tail_pipeline: Vec::new(),
                model_override: None,
            }),
        }
    }

    /// Layout fields (x/y/w/h, title) and visual config (thresholds,
    /// unit, show_value) must not move the fingerprint — otherwise
    /// every drag/rename would silently invalidate the cached value.
    /// The fingerprint covers only what changes the *runtime data
    /// shape* (variant + datasource binding + tail pipeline).
    #[test]
    fn config_fingerprint_ignores_layout_and_visual_config() {
        let base = gauge_widget("w-1", "wf-1", "out");
        let datasource = match &base {
            Widget::Gauge { datasource, .. } => datasource.clone(),
            _ => unreachable!(),
        };
        let moved = Widget::Gauge {
            id: "w-1".into(),
            title: "Renamed".into(),
            x: 99,
            y: 99,
            w: 12,
            h: 12,
            config: GaugeConfig {
                min: 0.0,
                max: 100.0,
                unit: Some("ms".into()),
                thresholds: Some(vec![GaugeThreshold {
                    value: 50.0,
                    color: "#0f0".into(),
                    label: None,
                }]),
                show_value: false,
            },
            datasource,
        };
        assert_eq!(
            widget_config_fingerprint(&base),
            widget_config_fingerprint(&moved),
            "layout + visual config changes must not invalidate snapshots"
        );
    }

    /// Datasource binding swap (different workflow) must produce a
    /// different fingerprint — otherwise a snapshot from the prior
    /// binding would paint over the new datasource on hydrate.
    #[test]
    fn config_fingerprint_changes_when_datasource_binding_changes() {
        let a = gauge_widget("w-1", "wf-1", "out");
        let b = gauge_widget("w-1", "wf-2", "out");
        assert_ne!(widget_config_fingerprint(&a), widget_config_fingerprint(&b));

        let c = gauge_widget("w-1", "wf-1", "other-key");
        assert_ne!(widget_config_fingerprint(&a), widget_config_fingerprint(&c));

        let mut with_tail = gauge_widget("w-1", "wf-1", "out");
        if let Widget::Gauge { datasource, .. } = &mut with_tail {
            datasource.as_mut().unwrap().tail_pipeline = vec![PipelineStep::Pick {
                path: "value".into(),
            }];
        }
        assert_ne!(
            widget_config_fingerprint(&a),
            widget_config_fingerprint(&with_tail),
            "tail pipeline edits must invalidate the snapshot"
        );
    }

    /// Different widget variants on the same datasource have different
    /// fingerprints, because `widget_runtime_data` emits a different
    /// shape for each variant — a cached gauge value can't be served
    /// to a stat widget without breaking the renderer contract.
    #[test]
    fn config_fingerprint_changes_when_widget_kind_changes() {
        let gauge = gauge_widget("w-1", "wf-1", "out");
        let text = Widget::Text {
            id: "w-1".into(),
            title: "Latency".into(),
            x: 0,
            y: 0,
            w: 4,
            h: 3,
            config: TextConfig {
                format: TextFormat::Markdown,
                font_size: 14,
                color: None,
                align: TextAlign::Left,
            },
            datasource: Some(DatasourceConfig {
                workflow_id: "wf-1".into(),
                output_key: "out".into(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: None,
                binding_source: None,
                bound_at: None,
                tail_pipeline: Vec::new(),
                model_override: None,
            }),
        };
        assert_ne!(
            widget_config_fingerprint(&gauge),
            widget_config_fingerprint(&text)
        );
    }

    /// W39: a saved DatasourceDefinition whose stored arguments object
    /// has different key order than the incoming shared proposal entry
    /// must still match — otherwise Build Chat would create a duplicate
    /// workflow every time the model serialises JSON in a new order.
    #[test]
    fn shared_matches_definition_ignores_argument_key_order() {
        let shared = crate::models::dashboard::SharedDatasource {
            key: "feed".into(),
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(serde_json::json!({
                "method": "GET",
                "url": "https://example.com/api",
                "headers": { "Accept": "application/json", "User-Agent": "datrina" },
            })),
            prompt: None,
            pipeline: Vec::new(),
            refresh_cron: None,
            label: None,
        };
        let def = DatasourceDefinition {
            id: "d1".into(),
            name: "Existing".into(),
            description: None,
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            // Same content, different field order. Old `==` JSON
            // comparison would say these were different sources.
            arguments: Some(serde_json::json!({
                "url": "https://example.com/api",
                "headers": { "User-Agent": "datrina", "Accept": "application/json" },
                "method": "GET",
            })),
            prompt: None,
            pipeline: Vec::new(),
            refresh_cron: None,
            workflow_id: "w1".into(),
            created_at: 0,
            updated_at: 0,
            health: None,
            originated_external_source_id: None,
        };
        assert!(shared_matches_definition(&shared, &def));
    }

    #[test]
    fn shared_matches_definition_separates_distinct_urls() {
        let shared = crate::models::dashboard::SharedDatasource {
            key: "feed".into(),
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(serde_json::json!({
                "method": "GET",
                "url": "https://a.example.com",
            })),
            prompt: None,
            pipeline: Vec::new(),
            refresh_cron: None,
            label: None,
        };
        let def = DatasourceDefinition {
            id: "d1".into(),
            name: "Existing".into(),
            description: None,
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: Some(serde_json::json!({
                "method": "GET",
                "url": "https://b.example.com",
            })),
            prompt: None,
            pipeline: Vec::new(),
            refresh_cron: None,
            workflow_id: "w1".into(),
            created_at: 0,
            updated_at: 0,
            health: None,
            originated_external_source_id: None,
        };
        assert!(!shared_matches_definition(&shared, &def));
    }

    #[test]
    fn derive_definition_name_dedupes_in_pending_batch() {
        let widget = BuildWidgetProposal {
            widget_type: BuildWidgetType::Table,
            title: "Trending repos".into(),
            data: serde_json::Value::Null,
            datasource_plan: None,
            config: None,
            x: None,
            y: None,
            w: None,
            h: None,
            replace_widget_id: None,
            size_preset: None,
            layout_pattern: None,
        };
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".into()),
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        };
        let mut pending: Vec<(DatasourceDefinition, Workflow)> = Vec::new();
        let n1 = derive_definition_name(&widget, &plan, &pending);
        assert_eq!(n1, "Trending repos");
        // Insert a definition with that name to simulate prior batch entry
        pending.push((
            DatasourceDefinition {
                id: "d1".into(),
                name: n1.clone(),
                description: None,
                kind: plan.kind.clone(),
                tool_name: plan.tool_name.clone(),
                server_id: None,
                arguments: None,
                prompt: None,
                pipeline: Vec::new(),
                refresh_cron: None,
                workflow_id: "w1".into(),
                created_at: 0,
                updated_at: 0,
                health: None,
                originated_external_source_id: None,
            },
            Workflow {
                id: "w1".into(),
                name: "w1".into(),
                description: None,
                nodes: Vec::new(),
                edges: Vec::new(),
                trigger: WorkflowTrigger {
                    kind: TriggerKind::Manual,
                    config: None,
                },
                is_enabled: true,
                pause_state: Default::default(),
                last_paused_at: None,
                last_pause_reason: None,
                last_run: None,
                created_at: 0,
                updated_at: 0,
            },
        ));
        let n2 = derive_definition_name(&widget, &plan, &pending);
        assert_eq!(n2, "Trending repos (2)");
    }

    /// W40: dedupe-by-workflow grouping is the invariant that keeps a
    /// shared workflow from running once per consumer widget. The
    /// inner refresh loop iterates `groups.into_iter()` and fires
    /// exactly one execution per entry, so verifying the grouping
    /// itself proves the dedupe contract for the batched refresh.
    #[test]
    fn group_consumers_dedupes_shared_workflow_ids() {
        use crate::models::widget::{DatasourceConfig, TextAlign, TextConfig, TextFormat};

        fn text_widget(id: &str, workflow_id: &str) -> Widget {
            Widget::Text {
                id: id.into(),
                title: format!("w-{id}"),
                x: 0,
                y: 0,
                w: 4,
                h: 2,
                config: TextConfig {
                    format: TextFormat::Markdown,
                    font_size: 14,
                    color: None,
                    align: TextAlign::Left,
                },
                datasource: Some(DatasourceConfig {
                    workflow_id: workflow_id.into(),
                    output_key: "output.data".into(),
                    post_process: None,
                    capture_traces: false,
                    datasource_definition_id: None,
                    binding_source: None,
                    bound_at: None,
                    tail_pipeline: Vec::new(),
                    model_override: None,
                }),
            }
        }

        let consumers = vec![
            (
                0_usize,
                text_widget("a", "shared-wf"),
                text_widget("a", "shared-wf").datasource().unwrap().clone(),
            ),
            (
                1_usize,
                text_widget("b", "shared-wf"),
                text_widget("b", "shared-wf").datasource().unwrap().clone(),
            ),
            (
                2_usize,
                text_widget("c", "lonely-wf"),
                text_widget("c", "lonely-wf").datasource().unwrap().clone(),
            ),
        ];
        let groups = group_consumers_by_workflow(&consumers);
        assert_eq!(
            groups.len(),
            2,
            "two distinct workflow ids must produce two execution groups"
        );
        let shared = groups.get("shared-wf").expect("shared-wf group present");
        assert_eq!(
            shared,
            &vec![0, 1],
            "consumers sharing a workflow_id collapse into one group with both indexes",
        );
        let lonely = groups.get("lonely-wf").expect("lonely-wf group present");
        assert_eq!(lonely, &vec![2], "unique workflow_id gets its own group");
    }

    /// Parameter fingerprint reflects the resolved selections. An
    /// empty map fingerprints the same regardless of which dashboard
    /// it came from (so widgets without params don't churn), but any
    /// value change shifts the fingerprint so the snapshot is dropped
    /// on hydrate.
    #[test]
    fn parameter_fingerprint_tracks_selected_values() {
        let empty: std::collections::BTreeMap<String, ParameterValue> = Default::default();
        assert_eq!(
            parameter_values_fingerprint(&empty),
            parameter_values_fingerprint(&Default::default())
        );

        let mut with_one = empty.clone();
        with_one.insert("env".into(), ParameterValue::String("prod".into()));
        let fp_prod = parameter_values_fingerprint(&with_one);
        assert_ne!(fp_prod, parameter_values_fingerprint(&empty));

        let mut with_other = empty.clone();
        with_other.insert("env".into(), ParameterValue::String("stage".into()));
        let fp_stage = parameter_values_fingerprint(&with_other);
        assert_ne!(fp_prod, fp_stage);

        // BTreeMap canonicalizes key order, so two semantically
        // identical maps always fingerprint the same.
        let mut also_prod = empty.clone();
        also_prod.insert("env".into(), ParameterValue::String("prod".into()));
        assert_eq!(fp_prod, parameter_values_fingerprint(&also_prod));
    }
}

#[cfg(test)]
mod widget_stream_tests {
    use super::*;
    use crate::models::pipeline::{LlmExpect, PipelineStep};

    fn text_widget() -> Widget {
        Widget::Text {
            id: "w1".into(),
            title: "t".into(),
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            config: TextConfig {
                format: TextFormat::Markdown,
                font_size: 14,
                color: None,
                align: TextAlign::Left,
            },
            datasource: None,
        }
    }

    fn stat_widget() -> Widget {
        Widget::Stat {
            id: "w2".into(),
            title: "s".into(),
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            config: crate::models::widget::StatConfig {
                unit: None,
                prefix: None,
                suffix: None,
                decimals: None,
                color_mode: crate::models::widget::StatColorMode::Value,
                thresholds: None,
                show_sparkline: true,
                graph_mode: crate::models::widget::StatGraphMode::None,
                align: TextAlign::Left,
            },
            datasource: None,
        }
    }

    #[test]
    fn tail_supports_text_streaming_text_widget_terminal_llm_postprocess() {
        let tail = vec![PipelineStep::LlmPostprocess {
            prompt: "summarize".into(),
            expect: LlmExpect::Text,
        }];
        assert!(tail_supports_text_streaming(&text_widget(), &tail));
    }

    #[test]
    fn tail_supports_text_streaming_rejects_non_terminal_llm_postprocess() {
        // LLM step is not the last → no streaming. Downstream
        // deterministic steps need the materialised value.
        let tail = vec![
            PipelineStep::LlmPostprocess {
                prompt: "x".into(),
                expect: LlmExpect::Text,
            },
            PipelineStep::Length,
        ];
        assert!(!tail_supports_text_streaming(&text_widget(), &tail));
    }

    #[test]
    fn tail_supports_text_streaming_rejects_json_expectation() {
        let tail = vec![PipelineStep::LlmPostprocess {
            prompt: "x".into(),
            expect: LlmExpect::Json,
        }];
        assert!(!tail_supports_text_streaming(&text_widget(), &tail));
    }

    #[test]
    fn tail_supports_text_streaming_rejects_non_text_widget() {
        let tail = vec![PipelineStep::LlmPostprocess {
            prompt: "x".into(),
            expect: LlmExpect::Text,
        }];
        assert!(!tail_supports_text_streaming(&stat_widget(), &tail));
    }

    #[test]
    fn tail_supports_text_streaming_rejects_empty_or_deterministic_tail() {
        assert!(!tail_supports_text_streaming(&text_widget(), &[]));
        let tail = vec![PipelineStep::Pick { path: "a".into() }];
        assert!(!tail_supports_text_streaming(&text_widget(), &tail));
    }
}
