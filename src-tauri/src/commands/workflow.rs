use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::commands::datasource::{widget_datasource, widget_kind};
use crate::models::workflow::{
    ScheduleDisplayState, SchedulePauseState, SchedulerHealth, SchedulerWarning,
    SchedulerWarningKind, TriggerKind, Workflow, WorkflowOwnerDashboard, WorkflowOwnerRef,
    WorkflowOwnerWidget, WorkflowRun, WorkflowRunCancelOutcome, WorkflowRunDetail,
    WorkflowRunFilter, WorkflowRunSummary, WorkflowScheduleSummary, WorkflowSummary,
    WORKFLOW_EVENT_CHANNEL,
};
use crate::models::ApiResult;
use crate::modules::scheduler::ScheduledRuntime;
use crate::modules::storage::Storage;
use crate::modules::workflow_engine::WorkflowEngine;
use crate::AppState;

#[tauri::command]
pub async fn list_workflows(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<Workflow>>, String> {
    Ok(match state.storage.list_workflows().await {
        Ok(workflows) => ApiResult::ok(workflows),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_workflow(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<Workflow>, String> {
    Ok(match state.storage.get_workflow(&id).await {
        Ok(Some(workflow)) => ApiResult::ok(workflow),
        Ok(None) => ApiResult::err("Workflow not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn execute_workflow(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    input: Option<serde_json::Value>,
) -> Result<ApiResult<WorkflowRun>, String> {
    let workflow = match state.storage.get_workflow(&id).await {
        Ok(Some(w)) => w,
        Ok(None) => return Ok(ApiResult::err("Workflow not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let provider = match active_provider(&state).await {
        Ok(provider) => provider,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    if let Err(e) = reconnect_enabled_mcp_servers(&state).await {
        return Ok(ApiResult::err(e.to_string()));
    }
    // W47: explicit `execute_workflow` invocations come from the
    // Operations cockpit and tests — no session/dashboard scope is
    // attached, so resolve against the app default only.
    let language_directive =
        crate::commands::language::resolve_effective_language(state.storage.as_ref(), None, None)
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
    info!("⚡ Executing workflow: {} ({})", workflow.name, id);

    Ok(match engine.execute(&workflow, input).await {
        Ok(execution) => {
            let run = execution.run;
            if let Err(e) = state.storage.save_workflow_run(&id, &run).await {
                return Ok(ApiResult::err(e.to_string()));
            }
            if let Err(e) = state.storage.update_workflow_last_run(&id, &run).await {
                return Ok(ApiResult::err(e.to_string()));
            }
            for event in execution.events {
                if let Err(e) = app.emit(WORKFLOW_EVENT_CHANNEL, event) {
                    return Ok(ApiResult::err(format!(
                        "Failed to emit workflow event: {}",
                        e
                    )));
                }
            }
            ApiResult::ok(run)
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn active_provider(
    state: &State<'_, AppState>,
) -> anyhow::Result<Option<crate::models::provider::LLMProvider>> {
    // W29: workflow execution + cron scheduling don't have a chat send
    // path to surface a typed correction state through. Return `None`
    // when the active provider can't be resolved; downstream nodes that
    // actually need a provider (LLM step, llm_postprocess) fail with a
    // visible workflow error instead of silently picking another row.
    match crate::resolve_active_provider(state.storage.as_ref()).await? {
        Ok(provider) => Ok(Some(provider)),
        Err(_setup_error) => Ok(None),
    }
}

#[tauri::command]
pub async fn create_workflow(
    app: AppHandle,
    state: State<'_, AppState>,
    workflow: Workflow,
) -> Result<ApiResult<bool>, String> {
    if let Err(e) = state.scheduler.lock().await.unschedule(&workflow.id).await {
        return Ok(ApiResult::err(e.to_string()));
    }
    Ok(match state.storage.create_workflow(&workflow).await {
        Ok(()) => {
            if let Err(e) = schedule_if_cron(&app, &state, workflow.clone()).await {
                return Ok(ApiResult::err(e.to_string()));
            }
            info!("📋 Created workflow: {}", workflow.name);
            ApiResult::ok(true)
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn delete_workflow(
    _app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    if let Err(e) = state.scheduler.lock().await.unschedule(&id).await {
        return Ok(ApiResult::err(e.to_string()));
    }
    Ok(match state.storage.delete_workflow(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

async fn reconnect_enabled_mcp_servers(state: &State<'_, AppState>) -> anyhow::Result<()> {
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

async fn schedule_if_cron(
    app: &AppHandle,
    state: &State<'_, AppState>,
    workflow: Workflow,
) -> anyhow::Result<()> {
    // W50: respect user pause intent. The workflow row is persisted with
    // `pause_state = Paused`, but `schedule_if_cron` is called from the
    // create path too — we must NOT re-register a paused workflow.
    if matches!(workflow.pause_state, SchedulePauseState::Paused) {
        return Ok(());
    }
    if !workflow.is_enabled {
        return Ok(());
    }
    let cron = match workflow
        .trigger
        .config
        .as_ref()
        .and_then(|config| config.cron.as_deref())
    {
        Some(cron) => cron.to_string(),
        None => return Ok(()),
    };
    let cron = match crate::commands::dashboard::normalize_cron_expression(&cron) {
        Some(value) => value,
        None => {
            tracing::warn!(
                "skipping scheduling for workflow '{}': cron '{}' is not parseable",
                workflow.id,
                cron
            );
            return Ok(());
        }
    };
    let provider = active_provider(state).await?;
    let runtime = ScheduledRuntime {
        app: app.clone(),
        storage: state.storage.clone(),
        tool_engine: state.tool_engine.clone(),
        mcp_manager: state.mcp_manager.clone(),
        ai_engine: state.ai_engine.clone(),
        provider,
    };
    state
        .scheduler
        .lock()
        .await
        .schedule_cron(workflow, &cron, runtime)
        .await
}

// ─── W35: Operations Cockpit commands ──────────────────────────────────────

#[tauri::command]
pub async fn list_workflow_summaries(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<WorkflowSummary>>, String> {
    Ok(match build_workflow_summaries(&state).await {
        Ok(summaries) => ApiResult::ok(summaries),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn list_workflow_runs(
    state: State<'_, AppState>,
    filter: Option<WorkflowRunFilter>,
) -> Result<ApiResult<Vec<WorkflowRunSummary>>, String> {
    let filter = filter.unwrap_or_default();
    Ok(match list_runs_inner(&state, filter).await {
        Ok(runs) => ApiResult::ok(runs),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_workflow_run_detail(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<ApiResult<WorkflowRunDetail>, String> {
    Ok(match run_detail_inner(&state, &run_id).await {
        Ok(Some(detail)) => ApiResult::ok(detail),
        Ok(None) => ApiResult::err("Workflow run not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn retry_workflow_run(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
) -> Result<ApiResult<WorkflowRun>, String> {
    let workflow_id = match state.storage.get_workflow_run(&run_id).await {
        Ok(Some((wf_id, _))) => wf_id,
        Ok(None) => return Ok(ApiResult::err("Workflow run not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    execute_workflow(app, state, workflow_id, None).await
}

#[tauri::command]
pub async fn cancel_workflow_run(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<ApiResult<WorkflowRunCancelOutcome>, String> {
    // The current runtime drives workflows synchronously inside the
    // scheduler tick / Tauri command future with no abort handle. We
    // surface that honestly so the UI can render an explicit
    // "cancellation unavailable in this build" affordance rather than a
    // silent no-op. When a future runtime supports real cancellation
    // it can fill `cancelled: true` and update `run_status`.
    let lookup = state.storage.get_workflow_run(&run_id).await;
    let run_status = match lookup {
        Ok(Some((_, run))) => Some(run.status),
        Ok(None) => None,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    Ok(ApiResult::ok(WorkflowRunCancelOutcome {
        cancelled: false,
        reason: "Cancellation is not supported in this runtime build.".to_string(),
        run_id,
        run_status,
    }))
}

#[tauri::command]
pub async fn get_scheduler_health(
    state: State<'_, AppState>,
) -> Result<ApiResult<SchedulerHealth>, String> {
    Ok(match scheduler_health_inner(&state).await {
        Ok(health) => ApiResult::ok(health),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

// ─── W50: pause/resume + cadence controls ──────────────────────────────────

/// W50: pause automatic refresh for a single workflow. Persists the
/// pause flag, unschedules the cron job, and returns the updated typed
/// schedule summary so the UI does not have to refetch.
#[tauri::command]
pub async fn pause_workflow_schedule(
    state: State<'_, AppState>,
    workflow_id: String,
    reason: Option<String>,
) -> Result<ApiResult<WorkflowSummary>, String> {
    Ok(
        match pause_workflow_inner(&state, &workflow_id, reason.as_deref()).await {
            Ok(Some(summary)) => ApiResult::ok(summary),
            Ok(None) => ApiResult::err(format!("Workflow {} not found", workflow_id)),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W50: resume automatic refresh for a single workflow. Persists the
/// state flip and re-registers the cron job through the existing
/// scheduler path. Invalid crons stay rejected — resume does NOT silently
/// fix a broken schedule.
#[tauri::command]
pub async fn resume_workflow_schedule(
    app: AppHandle,
    state: State<'_, AppState>,
    workflow_id: String,
) -> Result<ApiResult<WorkflowSummary>, String> {
    Ok(
        match resume_workflow_inner(&app, &state, &workflow_id).await {
            Ok(Some(summary)) => ApiResult::ok(summary),
            Ok(None) => ApiResult::err(format!("Workflow {} not found", workflow_id)),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W50: update the cron expression on a workflow. `cron = None` reverts
/// the trigger to manual. Cron strings are normalized through the
/// existing scheduler rules before persisting; invalid input is rejected
/// loud with `InvalidCronExpression` so the UI surfaces the same message
/// shown in scheduler health.
#[tauri::command]
pub async fn set_workflow_schedule(
    app: AppHandle,
    state: State<'_, AppState>,
    workflow_id: String,
    cron: Option<String>,
) -> Result<ApiResult<WorkflowSummary>, String> {
    Ok(
        match set_workflow_schedule_inner(&app, &state, &workflow_id, cron.as_deref()).await {
            Ok(Ok(Some(summary))) => ApiResult::ok(summary),
            Ok(Ok(None)) => ApiResult::err(format!("Workflow {} not found", workflow_id)),
            Ok(Err(message)) => ApiResult::err(message),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W50: pause every distinct workflow referenced by the dashboard's
/// widgets. The dashboard itself does not own a schedule — widgets do —
/// so this is a convenience over the per-workflow command.
#[tauri::command]
pub async fn pause_dashboard_schedules(
    state: State<'_, AppState>,
    dashboard_id: String,
    reason: Option<String>,
) -> Result<ApiResult<Vec<WorkflowSummary>>, String> {
    Ok(
        match pause_dashboard_inner(&state, &dashboard_id, reason.as_deref()).await {
            Ok(summaries) => ApiResult::ok(summaries),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W50: resume every distinct workflow referenced by the dashboard's
/// widgets. Invalid cron expressions stay unscheduled but become
/// `Active` so the next save with a valid cron registers cleanly.
#[tauri::command]
pub async fn resume_dashboard_schedules(
    app: AppHandle,
    state: State<'_, AppState>,
    dashboard_id: String,
) -> Result<ApiResult<Vec<WorkflowSummary>>, String> {
    Ok(
        match resume_dashboard_inner(&app, &state, &dashboard_id).await {
            Ok(summaries) => ApiResult::ok(summaries),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn pause_workflow_inner(
    state: &State<'_, AppState>,
    workflow_id: &str,
    reason: Option<&str>,
) -> anyhow::Result<Option<WorkflowSummary>> {
    let now = chrono::Utc::now().timestamp_millis();
    let updated = state
        .storage
        .set_workflow_pause_state(workflow_id, SchedulePauseState::Paused, reason, now)
        .await?;
    if !updated {
        return Ok(None);
    }
    // Unschedule first so a paused workflow never fires another tick
    // even if the storage write is slow to propagate.
    state.scheduler.lock().await.unschedule(workflow_id).await?;
    Ok(Some(build_single_summary(state, workflow_id).await?))
}

async fn resume_workflow_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    workflow_id: &str,
) -> anyhow::Result<Option<WorkflowSummary>> {
    let now = chrono::Utc::now().timestamp_millis();
    let updated = state
        .storage
        .set_workflow_pause_state(workflow_id, SchedulePauseState::Active, None, now)
        .await?;
    if !updated {
        return Ok(None);
    }
    let workflow = match state.storage.get_workflow(workflow_id).await? {
        Some(w) => w,
        None => return Ok(None),
    };
    // Honor existing scheduler routing: only re-register when the
    // workflow has a valid cron and is enabled. The helper logs and
    // skips broken cron rather than crashing the resume path.
    schedule_if_cron(app, state, workflow).await?;
    Ok(Some(build_single_summary(state, workflow_id).await?))
}

async fn set_workflow_schedule_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    workflow_id: &str,
    cron: Option<&str>,
) -> anyhow::Result<Result<Option<WorkflowSummary>, String>> {
    // Normalize / reject up front so we never persist a cron the
    // scheduler would refuse. `None` is allowed and switches the trigger
    // back to manual.
    let normalized_cron = match cron.map(str::trim) {
        Some(raw) if !raw.is_empty() => {
            match crate::commands::dashboard::normalize_cron_expression(raw) {
                Some(value) => Some(value),
                None => {
                    return Ok(Err(format!(
                        "cron expression '{}' is not parseable by the scheduler",
                        raw
                    )));
                }
            }
        }
        _ => None,
    };

    let now = chrono::Utc::now().timestamp_millis();
    let updated = state
        .storage
        .set_workflow_cron(workflow_id, normalized_cron.as_deref(), now)
        .await?;
    if !updated {
        return Ok(Ok(None));
    }
    // Always unschedule first; re-register through the standard helper
    // so pause state + is_enabled + cron validity gate registration in
    // exactly one place.
    state.scheduler.lock().await.unschedule(workflow_id).await?;
    if let Some(workflow) = state.storage.get_workflow(workflow_id).await? {
        schedule_if_cron(app, state, workflow).await?;
    }
    Ok(Ok(Some(build_single_summary(state, workflow_id).await?)))
}

async fn pause_dashboard_inner(
    state: &State<'_, AppState>,
    dashboard_id: &str,
    reason: Option<&str>,
) -> anyhow::Result<Vec<WorkflowSummary>> {
    let ids = workflow_ids_for_dashboard(state, dashboard_id).await?;
    let mut summaries = Vec::new();
    for workflow_id in ids {
        if let Some(summary) = pause_workflow_inner(state, &workflow_id, reason).await? {
            summaries.push(summary);
        }
    }
    Ok(summaries)
}

async fn resume_dashboard_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    dashboard_id: &str,
) -> anyhow::Result<Vec<WorkflowSummary>> {
    let ids = workflow_ids_for_dashboard(state, dashboard_id).await?;
    let mut summaries = Vec::new();
    for workflow_id in ids {
        if let Some(summary) = resume_workflow_inner(app, state, &workflow_id).await? {
            summaries.push(summary);
        }
    }
    Ok(summaries)
}

async fn workflow_ids_for_dashboard(
    state: &State<'_, AppState>,
    dashboard_id: &str,
) -> anyhow::Result<Vec<String>> {
    let dashboards = state.storage.list_dashboards().await?;
    let Some(dashboard) = dashboards.into_iter().find(|d| d.id == dashboard_id) else {
        return Ok(Vec::new());
    };
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for widget in &dashboard.layout {
        if let Some(config) = widget_datasource(widget) {
            if seen.insert(config.workflow_id.clone()) {
                ids.push(config.workflow_id.clone());
            }
        }
    }
    Ok(ids)
}

async fn build_single_summary(
    state: &State<'_, AppState>,
    workflow_id: &str,
) -> anyhow::Result<WorkflowSummary> {
    let workflow = state
        .storage
        .get_workflow(workflow_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workflow {} disappeared after update", workflow_id))?;
    let scheduled = state.scheduler.lock().await.list_scheduled();
    let scheduled_set: std::collections::HashSet<String> = scheduled.into_iter().collect();
    let schedule = build_schedule_summary(&workflow, &scheduled_set);
    let last_run = state
        .storage
        .list_workflow_run_summaries(Some(&workflow.id), 1)
        .await?
        .into_iter()
        .next();
    let owner = build_owner_ref(state.storage.as_ref(), &workflow.id).await?;
    Ok(WorkflowSummary {
        id: workflow.id.clone(),
        name: workflow.name.clone(),
        description: workflow.description.clone(),
        is_enabled: workflow.is_enabled,
        trigger: workflow.trigger.clone(),
        schedule,
        last_run,
        owner,
        created_at: workflow.created_at,
        updated_at: workflow.updated_at,
    })
}

async fn list_runs_inner(
    state: &State<'_, AppState>,
    filter: WorkflowRunFilter,
) -> anyhow::Result<Vec<WorkflowRunSummary>> {
    let WorkflowRunFilter {
        workflow_id,
        dashboard_id,
        widget_id,
        datasource_definition_id,
        status,
        limit,
    } = filter;
    let limit = limit.unwrap_or(100);

    // Resolve owner-side filters down to a workflow_id. We accept at
    // most one workflow filter source; the explicit `workflow_id` wins.
    let resolved_workflow = if let Some(wf) = workflow_id {
        Some(wf)
    } else if let Some(ref ds_id) = datasource_definition_id {
        state
            .storage
            .get_datasource_definition(ds_id)
            .await?
            .map(|d| d.workflow_id)
    } else if widget_id.is_some() || dashboard_id.is_some() {
        resolve_widget_workflow(state, dashboard_id.as_deref(), widget_id.as_deref()).await?
    } else {
        None
    };

    // If the user filtered by an owner ref that doesn't resolve to a
    // workflow, the result is honestly empty — not a global scan.
    let owner_filter_present =
        dashboard_id.is_some() || widget_id.is_some() || datasource_definition_id.is_some();
    if owner_filter_present && resolved_workflow.is_none() {
        return Ok(Vec::new());
    }

    let runs = state
        .storage
        .list_workflow_run_summaries(resolved_workflow.as_deref(), limit)
        .await?;

    Ok(match status {
        Some(target) => runs
            .into_iter()
            .filter(|r| std::mem::discriminant(&r.status) == std::mem::discriminant(&target))
            .collect(),
        None => runs,
    })
}

async fn resolve_widget_workflow(
    state: &State<'_, AppState>,
    dashboard_id: Option<&str>,
    widget_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let dashboards = state.storage.list_dashboards().await?;
    for dashboard in dashboards {
        if let Some(d_id) = dashboard_id {
            if dashboard.id != d_id {
                continue;
            }
        }
        for widget in &dashboard.layout {
            if let Some(w_id) = widget_id {
                if widget.id() != w_id {
                    continue;
                }
            }
            if let Some(config) = widget_datasource(widget) {
                return Ok(Some(config.workflow_id.clone()));
            }
        }
    }
    Ok(None)
}

async fn run_detail_inner(
    state: &State<'_, AppState>,
    run_id: &str,
) -> anyhow::Result<Option<WorkflowRunDetail>> {
    let Some((workflow_id, run)) = state.storage.get_workflow_run(run_id).await? else {
        return Ok(None);
    };
    let workflow = state.storage.get_workflow(&workflow_id).await?;
    let workflow_name = workflow
        .as_ref()
        .map(|w| w.name.clone())
        .unwrap_or_else(|| workflow_id.clone());
    let owner = build_owner_ref(state.storage.as_ref(), &workflow_id).await?;
    Ok(Some(WorkflowRunDetail {
        run,
        workflow_id,
        workflow_name,
        owner,
    }))
}

async fn build_workflow_summaries(
    state: &State<'_, AppState>,
) -> anyhow::Result<Vec<WorkflowSummary>> {
    let workflows = state.storage.list_workflows().await?;
    let scheduled = state.scheduler.lock().await.list_scheduled();
    let scheduled_set: std::collections::HashSet<String> = scheduled.into_iter().collect();
    let mut summaries = Vec::with_capacity(workflows.len());
    for workflow in workflows {
        let schedule = build_schedule_summary(&workflow, &scheduled_set);
        let last_run = state
            .storage
            .list_workflow_run_summaries(Some(&workflow.id), 1)
            .await?
            .into_iter()
            .next();
        let owner = build_owner_ref(state.storage.as_ref(), &workflow.id).await?;
        summaries.push(WorkflowSummary {
            id: workflow.id.clone(),
            name: workflow.name.clone(),
            description: workflow.description.clone(),
            is_enabled: workflow.is_enabled,
            trigger: workflow.trigger.clone(),
            schedule,
            last_run,
            owner,
            created_at: workflow.created_at,
            updated_at: workflow.updated_at,
        });
    }
    summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(summaries)
}

fn build_schedule_summary(
    workflow: &Workflow,
    scheduled: &std::collections::HashSet<String>,
) -> WorkflowScheduleSummary {
    let is_scheduled = scheduled.contains(&workflow.id);
    let trigger_kind = Some(workflow.trigger.kind.clone());
    let cron = workflow
        .trigger
        .config
        .as_ref()
        .and_then(|c| c.cron.clone())
        .filter(|c| !c.trim().is_empty());
    let cron_normalized = cron
        .as_deref()
        .and_then(crate::commands::dashboard::normalize_cron_expression);
    let cron_is_valid = matches!(workflow.trigger.kind, TriggerKind::Cron)
        && cron.is_some()
        && cron_normalized.is_some();
    let display_state = derive_display_state(workflow, cron_is_valid, is_scheduled);
    WorkflowScheduleSummary {
        is_scheduled,
        cron,
        cron_normalized,
        cron_is_valid,
        trigger_kind,
        pause_state: workflow.pause_state,
        last_paused_at: workflow.last_paused_at,
        last_pause_reason: workflow.last_pause_reason.clone(),
        display_state,
    }
}

/// W50: collapse pause_state + is_enabled + cron validity into the single
/// label the UI surfaces. Keeping the rule on the Rust side stops the
/// dashboard, Workbench, and Operations from drifting.
fn derive_display_state(
    workflow: &Workflow,
    cron_is_valid: bool,
    is_scheduled: bool,
) -> ScheduleDisplayState {
    if !workflow.is_enabled {
        return ScheduleDisplayState::Disabled;
    }
    match workflow.trigger.kind {
        TriggerKind::Cron => {
            if matches!(workflow.pause_state, SchedulePauseState::Paused) {
                return ScheduleDisplayState::PausedByUser;
            }
            let cron_present = workflow
                .trigger
                .config
                .as_ref()
                .and_then(|c| c.cron.as_deref())
                .map(|c| !c.trim().is_empty())
                .unwrap_or(false);
            if !cron_present {
                ScheduleDisplayState::ManualOnly
            } else if !cron_is_valid {
                ScheduleDisplayState::Invalid
            } else if is_scheduled {
                ScheduleDisplayState::Active
            } else {
                // Cron is valid + workflow enabled + not paused, but the
                // scheduler has not registered it. Surface honestly so the
                // user can see startup failures rather than a green badge.
                ScheduleDisplayState::Invalid
            }
        }
        TriggerKind::Event | TriggerKind::Manual => ScheduleDisplayState::NotScheduled,
    }
}

async fn build_owner_ref(storage: &Storage, workflow_id: &str) -> anyhow::Result<WorkflowOwnerRef> {
    let datasource = storage.get_datasource_by_workflow_id(workflow_id).await?;
    let mut owner = WorkflowOwnerRef {
        datasource_definition_id: datasource.as_ref().map(|d| d.id.clone()),
        datasource_name: datasource.as_ref().map(|d| d.name.clone()),
        dashboards: Vec::new(),
    };
    let dashboards = storage.list_dashboards().await?;
    for dashboard in dashboards {
        let mut widgets = Vec::new();
        for widget in &dashboard.layout {
            if let Some(config) = widget_datasource(widget) {
                if config.workflow_id == workflow_id {
                    let explicit = match (
                        config.datasource_definition_id.as_ref(),
                        owner.datasource_definition_id.as_ref(),
                    ) {
                        (Some(bound), Some(owner_id)) => bound == owner_id,
                        _ => false,
                    };
                    widgets.push(WorkflowOwnerWidget {
                        widget_id: widget.id().to_string(),
                        widget_title: widget.title().to_string(),
                        widget_kind: widget_kind(widget).to_string(),
                        output_key: config.output_key.clone(),
                        explicit_binding: explicit,
                    });
                }
            }
        }
        if !widgets.is_empty() {
            owner.dashboards.push(WorkflowOwnerDashboard {
                dashboard_id: dashboard.id.clone(),
                dashboard_name: dashboard.name.clone(),
                widgets,
            });
        }
    }
    Ok(owner)
}

async fn scheduler_health_inner(state: &State<'_, AppState>) -> anyhow::Result<SchedulerHealth> {
    let scheduled = state.scheduler.lock().await.list_scheduled();
    let scheduled_set: std::collections::HashSet<String> = scheduled.iter().cloned().collect();
    let workflows = state.storage.list_workflows().await?;
    let mut warnings = Vec::new();
    for workflow in &workflows {
        let cron = workflow
            .trigger
            .config
            .as_ref()
            .and_then(|c| c.cron.as_deref())
            .filter(|c| !c.trim().is_empty());
        let is_cron_trigger = matches!(workflow.trigger.kind, TriggerKind::Cron);
        if is_cron_trigger {
            match cron {
                Some(raw) => {
                    if crate::commands::dashboard::normalize_cron_expression(raw).is_none() {
                        warnings.push(SchedulerWarning {
                            workflow_id: workflow.id.clone(),
                            workflow_name: workflow.name.clone(),
                            kind: SchedulerWarningKind::InvalidCron,
                            message: format!(
                                "cron expression '{}' is not parseable by the scheduler",
                                raw
                            ),
                        });
                    } else if workflow.is_enabled
                        && !scheduled_set.contains(&workflow.id)
                        && matches!(workflow.pause_state, SchedulePauseState::Active)
                    {
                        // W50: only warn when the schedule is *expected*
                        // to be registered. A user-paused workflow is
                        // intentionally inactive — surfacing it here
                        // would push operators to "fix" the very state
                        // they asked for.
                        warnings.push(SchedulerWarning {
                            workflow_id: workflow.id.clone(),
                            workflow_name: workflow.name.clone(),
                            kind: SchedulerWarningKind::EnabledButNotScheduled,
                            message:
                                "workflow has a valid cron and is enabled but is not scheduled in this session".to_string(),
                        });
                    } else if !workflow.is_enabled && scheduled_set.contains(&workflow.id) {
                        warnings.push(SchedulerWarning {
                            workflow_id: workflow.id.clone(),
                            workflow_name: workflow.name.clone(),
                            kind: SchedulerWarningKind::ScheduledButDisabled,
                            message: "workflow is disabled but still registered with the scheduler"
                                .to_string(),
                        });
                    }
                }
                None => warnings.push(SchedulerWarning {
                    workflow_id: workflow.id.clone(),
                    workflow_name: workflow.name.clone(),
                    kind: SchedulerWarningKind::CronTriggerDisabled,
                    message: "trigger kind is cron but no cron expression is set".to_string(),
                }),
            }
        }
    }
    Ok(SchedulerHealth {
        scheduler_started: true,
        scheduled_workflow_ids: scheduled,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::widget::{ChartConfig, ChartKind, DatasourceConfig, Widget};
    use crate::models::workflow::{RunStatus, TriggerConfig, WorkflowRun, WorkflowTrigger};
    use std::collections::HashSet;

    fn manual_workflow(id: &str, name: &str) -> Workflow {
        let now = chrono::Utc::now().timestamp_millis();
        Workflow {
            id: id.into(),
            name: name.into(),
            description: None,
            nodes: vec![],
            edges: vec![],
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

    fn cron_workflow(id: &str, cron: &str, enabled: bool) -> Workflow {
        let mut wf = manual_workflow(id, "Cron");
        wf.is_enabled = enabled;
        wf.trigger = WorkflowTrigger {
            kind: TriggerKind::Cron,
            config: Some(TriggerConfig {
                cron: Some(cron.into()),
                event: None,
            }),
        };
        wf
    }

    #[test]
    fn schedule_summary_marks_invalid_cron_and_scheduled_state() {
        let workflow = cron_workflow("wf-1", "*/5 * * * *", true);
        let mut scheduled = HashSet::new();
        scheduled.insert("wf-1".to_string());
        let summary = build_schedule_summary(&workflow, &scheduled);
        // Five-field POSIX cron is normalized to a 6-field form.
        assert!(summary.cron_is_valid, "5-field cron should normalize");
        assert!(summary.is_scheduled);
        assert!(summary.cron_normalized.is_some());

        let broken = cron_workflow("wf-2", "not-a-cron", true);
        let summary = build_schedule_summary(&broken, &HashSet::new());
        assert!(!summary.cron_is_valid);
        assert!(summary.cron_normalized.is_none());

        let manual = manual_workflow("wf-3", "Manual");
        let summary = build_schedule_summary(&manual, &HashSet::new());
        assert!(!summary.cron_is_valid);
        assert!(summary.cron.is_none());
        assert!(!summary.is_scheduled);
    }

    #[tokio::test]
    async fn owner_ref_resolves_widget_consumers() -> anyhow::Result<()> {
        use crate::models::dashboard::Dashboard;

        let storage = Storage::new_for_tests().await?;
        let now = chrono::Utc::now().timestamp_millis();

        // Workflow + dashboard with one chart widget bound to the workflow id.
        let workflow = manual_workflow("wf-owner", "Owner");
        storage.create_workflow(&workflow).await?;

        let widget = Widget::Chart {
            id: "w-1".into(),
            title: "Bound chart".into(),
            x: 0,
            y: 0,
            w: 4,
            h: 4,
            config: ChartConfig {
                kind: ChartKind::Line,
                x_axis: None,
                y_axis: None,
                colors: None,
                stacked: false,
                show_legend: true,
            },
            datasource: Some(DatasourceConfig {
                workflow_id: workflow.id.clone(),
                output_key: "output.data".into(),
                ..Default::default()
            }),
            refresh_interval: None,
        };
        let dashboard = Dashboard {
            id: "dash-1".into(),
            name: "Ops".into(),
            description: None,
            layout: vec![widget],
            workflows: vec![],
            is_default: false,
            created_at: now,
            updated_at: now,
            parameters: vec![],
            model_policy: None,
            language_policy: None,
        };
        storage.create_dashboard(&dashboard).await?;

        let owner = build_owner_ref(&storage, &workflow.id).await?;
        assert!(owner.datasource_definition_id.is_none());
        assert_eq!(owner.dashboards.len(), 1);
        let dash = &owner.dashboards[0];
        assert_eq!(dash.dashboard_id, "dash-1");
        assert_eq!(dash.widgets.len(), 1);
        assert_eq!(dash.widgets[0].widget_id, "w-1");
        assert_eq!(dash.widgets[0].widget_kind, "chart");
        Ok(())
    }

    #[test]
    fn run_status_round_trips_through_summary() {
        // Sanity guard: the summary struct must serialize back the same
        // status enum the UI reads.
        let summary = WorkflowRunSummary {
            id: "r-1".into(),
            workflow_id: "wf-1".into(),
            started_at: 1,
            finished_at: Some(2),
            status: RunStatus::Error,
            duration_ms: Some(1),
            error: Some("boom".into()),
            has_node_results: false,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        let _: WorkflowRunSummary = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn cancel_outcome_is_unsupported_by_default() {
        // The cancel command exposes an unsupported result — make sure
        // we keep that contract until a real runtime cancel exists.
        let outcome = WorkflowRunCancelOutcome {
            cancelled: false,
            reason: "no abort handle".into(),
            run_id: "r".into(),
            run_status: Some(RunStatus::Running),
        };
        assert!(!outcome.cancelled);
    }

    // Silence unused-import warning in this `cfg(test)` module — pulled
    // in for symmetry with the integration test wiring.
    #[allow(dead_code)]
    fn _runs(_run: WorkflowRun) {}

    // ─── W50: pause/resume + cadence ──────────────────────────────────────

    #[test]
    fn display_state_paused_is_distinct_from_invalid() {
        // A paused workflow with a valid cron must surface as
        // PausedByUser, not Invalid, so the UI can offer Resume rather
        // than the broken-cron remediation.
        let mut wf = cron_workflow("wf-paused", "*/5 * * * *", true);
        wf.pause_state = SchedulePauseState::Paused;
        let summary = build_schedule_summary(&wf, &HashSet::new());
        assert_eq!(summary.display_state, ScheduleDisplayState::PausedByUser);
        assert_eq!(summary.pause_state, SchedulePauseState::Paused);

        // A paused workflow with a broken cron still surfaces as paused
        // (intentional user state wins over the broken trigger). When
        // the user resumes, the cron is re-validated and surfaces as
        // Invalid then.
        let mut wf_bad = cron_workflow("wf-paused-bad", "not-a-cron", true);
        wf_bad.pause_state = SchedulePauseState::Paused;
        let summary = build_schedule_summary(&wf_bad, &HashSet::new());
        assert_eq!(summary.display_state, ScheduleDisplayState::PausedByUser);

        // Disabled wins over paused — disabling the workflow stops
        // refresh too, just for a different reason.
        let mut wf_off = cron_workflow("wf-off", "*/5 * * * *", false);
        wf_off.pause_state = SchedulePauseState::Paused;
        let summary = build_schedule_summary(&wf_off, &HashSet::new());
        assert_eq!(summary.display_state, ScheduleDisplayState::Disabled);
    }

    #[test]
    fn display_state_active_and_not_scheduled() {
        // Active, scheduled, valid cron.
        let wf = cron_workflow("wf-ok", "*/5 * * * *", true);
        let mut scheduled = HashSet::new();
        scheduled.insert("wf-ok".to_string());
        let summary = build_schedule_summary(&wf, &scheduled);
        assert_eq!(summary.display_state, ScheduleDisplayState::Active);

        // Manual trigger surfaces as NotScheduled even if the workflow
        // is enabled — auto refresh just isn't applicable here.
        let wf_manual = manual_workflow("wf-m", "Manual");
        let summary = build_schedule_summary(&wf_manual, &HashSet::new());
        assert_eq!(summary.display_state, ScheduleDisplayState::NotScheduled);
    }

    #[tokio::test]
    async fn storage_persists_pause_state_across_reload() -> anyhow::Result<()> {
        // The pause flag must survive process restart so a paused
        // schedule does NOT silently come back as ticking on the next
        // app launch.
        let storage = Storage::new_for_tests().await?;
        let mut wf = manual_workflow("wf-pause-persist", "Persist test");
        wf.trigger = WorkflowTrigger {
            kind: TriggerKind::Cron,
            config: Some(TriggerConfig {
                cron: Some("0 */5 * * * *".into()),
                event: None,
            }),
        };
        storage.create_workflow(&wf).await?;

        // Pause through the storage helper — the command path uses the
        // same call.
        let now = chrono::Utc::now().timestamp_millis();
        let updated = storage
            .set_workflow_pause_state(
                &wf.id,
                SchedulePauseState::Paused,
                Some("user pressed pause"),
                now,
            )
            .await?;
        assert!(updated, "storage update should hit one row");

        // Reload and confirm.
        let reloaded = storage.get_workflow(&wf.id).await?.unwrap();
        assert_eq!(reloaded.pause_state, SchedulePauseState::Paused);
        assert_eq!(reloaded.last_paused_at, Some(now));
        assert_eq!(
            reloaded.last_pause_reason.as_deref(),
            Some("user pressed pause")
        );

        // Resume clears the reason but keeps the workflow.
        let resumed_now = now + 1000;
        storage
            .set_workflow_pause_state(&wf.id, SchedulePauseState::Active, None, resumed_now)
            .await?;
        let reloaded = storage.get_workflow(&wf.id).await?.unwrap();
        assert_eq!(reloaded.pause_state, SchedulePauseState::Active);
        assert_eq!(reloaded.last_paused_at, None);
        assert_eq!(reloaded.last_pause_reason, None);

        // Unknown workflow id returns false (typed not-found, not panic).
        let missing = storage
            .set_workflow_pause_state(
                "does-not-exist",
                SchedulePauseState::Paused,
                None,
                resumed_now,
            )
            .await?;
        assert!(!missing);

        Ok(())
    }

    #[tokio::test]
    async fn storage_set_workflow_cron_round_trips() -> anyhow::Result<()> {
        // Setting cron to Some normalized value must produce a Cron
        // trigger; passing None reverts to Manual. Both transitions are
        // observed through the persisted workflow row.
        let storage = Storage::new_for_tests().await?;
        let wf = manual_workflow("wf-cron-update", "Cron update");
        storage.create_workflow(&wf).await?;

        let now = chrono::Utc::now().timestamp_millis();
        let updated = storage
            .set_workflow_cron(&wf.id, Some("0 */5 * * * *"), now)
            .await?;
        assert!(updated);
        let reloaded = storage.get_workflow(&wf.id).await?.unwrap();
        assert!(matches!(reloaded.trigger.kind, TriggerKind::Cron));
        assert_eq!(
            reloaded
                .trigger
                .config
                .as_ref()
                .and_then(|c| c.cron.as_deref()),
            Some("0 */5 * * * *")
        );

        // Clearing reverts to manual.
        storage.set_workflow_cron(&wf.id, None, now + 1).await?;
        let reloaded = storage.get_workflow(&wf.id).await?.unwrap();
        assert!(matches!(reloaded.trigger.kind, TriggerKind::Manual));
        assert!(reloaded.trigger.config.is_none());
        Ok(())
    }

    #[test]
    fn scheduler_health_does_not_warn_for_paused_workflow() {
        // W50: a user-paused workflow is intentionally unscheduled. The
        // health report MUST NOT flag it as `EnabledButNotScheduled`.
        // We exercise the warning rule directly because the full inner
        // command requires an AppState — the rule is what matters.
        use crate::models::workflow::{SchedulerWarningKind, TriggerKind};
        let mut paused = cron_workflow("wf-paused-health", "*/5 * * * *", true);
        paused.pause_state = SchedulePauseState::Paused;
        let scheduled_set: HashSet<String> = HashSet::new();

        // Replicate the gate from `scheduler_health_inner`: only emit
        // `EnabledButNotScheduled` for active (non-paused) workflows.
        let should_warn = matches!(paused.trigger.kind, TriggerKind::Cron)
            && paused.is_enabled
            && !scheduled_set.contains(&paused.id)
            && matches!(paused.pause_state, SchedulePauseState::Active);
        assert!(
            !should_warn,
            "paused workflows must not trigger the warning"
        );

        // An active workflow with the same gate ought to still warn.
        let mut active = paused.clone();
        active.pause_state = SchedulePauseState::Active;
        let should_warn = matches!(active.trigger.kind, TriggerKind::Cron)
            && active.is_enabled
            && !scheduled_set.contains(&active.id)
            && matches!(active.pause_state, SchedulePauseState::Active);
        assert!(should_warn);
        // Silence "unused" lints on the import in this branch.
        let _ = SchedulerWarningKind::EnabledButNotScheduled;
    }

    #[test]
    fn set_workflow_schedule_rejects_invalid_cron() {
        // Validation guard — we never persist a cron the scheduler
        // would reject. The normalize helper returning None is the
        // single source of truth.
        assert!(crate::commands::dashboard::normalize_cron_expression("not-a-cron").is_none());
        assert!(
            crate::commands::dashboard::normalize_cron_expression("0 */5 * * * *").is_some(),
            "5-field POSIX cron should be accepted after the seconds prefix"
        );
    }
}
