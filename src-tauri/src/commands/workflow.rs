use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::models::workflow::{Workflow, WorkflowRun, WORKFLOW_EVENT_CHANNEL};
use crate::models::ApiResult;
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

    let engine = WorkflowEngine::with_tool_engine(state.tool_engine.as_ref());
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

#[tauri::command]
pub async fn create_workflow(
    state: State<'_, AppState>,
    workflow: Workflow,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.create_workflow(&workflow).await {
        Ok(()) => {
            info!("📋 Created workflow: {}", workflow.name);
            ApiResult::ok(true)
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn delete_workflow(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_workflow(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}
