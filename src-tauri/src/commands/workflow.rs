use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::models::workflow::{Workflow, WorkflowRun, WORKFLOW_EVENT_CHANNEL};
use crate::models::ApiResult;
use crate::modules::scheduler::ScheduledRuntime;
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
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        provider,
    );
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
    let cron = match workflow
        .trigger
        .config
        .as_ref()
        .and_then(|config| config.cron.as_deref())
    {
        Some(cron) => cron.to_string(),
        None => return Ok(()),
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
