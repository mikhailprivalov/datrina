use tauri::State;
use tracing::info;

use crate::models::chat::{
    ChatMessage, ChatMode, ChatSession, CreateSessionRequest, MessageMetadata, MessageRole,
    SendMessageRequest,
};
use crate::models::ApiResult;
use crate::AppState;

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<ChatSession>>, String> {
    Ok(match state.storage.list_chat_sessions().await {
        Ok(sessions) => ApiResult::ok(sessions),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<ChatSession>, String> {
    Ok(match state.storage.get_chat_session(&id).await {
        Ok(Some(session)) => ApiResult::ok(session),
        Ok(None) => ApiResult::err("Session not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, AppState>,
    req: CreateSessionRequest,
) -> Result<ApiResult<ChatSession>, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let session = ChatSession {
        id: uuid::Uuid::new_v4().to_string(),
        mode: req.mode.clone(),
        dashboard_id: req.dashboard_id,
        widget_id: req.widget_id,
        title: match req.mode {
            ChatMode::Build => "New Dashboard Build".to_string(),
            ChatMode::Context => "Data Analysis".to_string(),
        },
        messages: vec![],
        created_at: now,
        updated_at: now,
    };

    Ok(match state.storage.create_chat_session(&session).await {
        Ok(()) => {
            info!("💬 Created chat session: {}", session.id);
            ApiResult::ok(session)
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    req: SendMessageRequest,
) -> Result<ApiResult<ChatMessage>, String> {
    let now = chrono::Utc::now().timestamp_millis();

    // Get session
    let mut session = {
        match state.storage.get_chat_session(&session_id).await {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(ApiResult::err("Session not found".to_string())),
            Err(e) => return Ok(ApiResult::err(e.to_string())),
        }
    };

    let user_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        mode: session.mode.clone(),
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: now,
    };
    session.messages.push(user_msg);
    session.updated_at = chrono::Utc::now().timestamp_millis();

    if let Err(e) = state.storage.update_chat_session(&session).await {
        return Ok(ApiResult::err(e.to_string()));
    }

    let providers = match state.storage.list_providers().await {
        Ok(providers) => providers,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let active_provider_id = match state.storage.get_config("active_provider_id").await {
        Ok(value) => value.filter(|id| !id.trim().is_empty()),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let provider = match active_provider_id
        .as_deref()
        .and_then(|id| {
            providers
                .iter()
                .find(|provider| provider.id == id && provider.is_enabled)
        })
        .or_else(|| providers.iter().find(|provider| provider.is_enabled))
        .cloned()
    {
        Some(provider) => provider,
        None => {
            return Ok(ApiResult::err(
                "AI chat unavailable: configure an enabled provider or local_mock provider first"
                    .to_string(),
            ));
        }
    };

    let provider_messages = match grounded_messages(&state, &session).await {
        Ok(messages) => messages,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let ai_response = match state
        .ai_engine
        .complete_chat(&provider, &provider_messages)
        .await
    {
        Ok(response) => response,
        Err(e) => return Ok(ApiResult::err(format!("AI provider call failed: {}", e))),
    };

    let assistant_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::Assistant,
        content: ai_response.content,
        mode: session.mode.clone(),
        tool_calls: None,
        tool_results: None,
        metadata: Some(MessageMetadata {
            model: Some(ai_response.model),
            provider: Some(ai_response.provider_id),
            tokens: ai_response.tokens,
            latency_ms: Some(ai_response.latency_ms),
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    session.messages.push(assistant_msg.clone());
    session.updated_at = chrono::Utc::now().timestamp_millis();

    // Save session
    if let Err(e) = state.storage.update_chat_session(&session).await {
        return Ok(ApiResult::err(e.to_string()));
    }

    Ok(ApiResult::ok(assistant_msg))
}

async fn grounded_messages(
    state: &State<'_, AppState>,
    session: &ChatSession,
) -> anyhow::Result<Vec<ChatMessage>> {
    let mut messages = Vec::new();
    if matches!(session.mode, ChatMode::Context) {
        if let Some(dashboard_id) = session.dashboard_id.as_deref() {
            if let Some(dashboard) = state.storage.get_dashboard(dashboard_id).await? {
                let mut workflow_summaries = Vec::new();
                for workflow_ref in &dashboard.workflows {
                    let workflow = state
                        .storage
                        .get_workflow(&workflow_ref.id)
                        .await?
                        .unwrap_or_else(|| workflow_ref.clone());
                    let last_run = workflow.last_run.as_ref().map(|run| {
                        serde_json::json!({
                            "id": run.id,
                            "status": run.status,
                            "error": run.error,
                            "node_results": run.node_results,
                        })
                    });
                    workflow_summaries.push(serde_json::json!({
                        "id": workflow.id,
                        "name": workflow.name,
                        "last_run": last_run,
                    }));
                }

                let context = serde_json::json!({
                    "dashboard": {
                        "id": dashboard.id,
                        "name": dashboard.name,
                        "description": dashboard.description,
                    },
                    "widgets": dashboard.layout.iter().map(|widget| {
                        serde_json::json!({
                            "id": widget.id(),
                            "title": widget.title(),
                        })
                    }).collect::<Vec<_>>(),
                    "workflows": workflow_summaries,
                });
                messages.push(system_message(format!(
                    "You are answering context chat for the selected Datrina dashboard. Ground the answer only in this local dashboard/runtime context unless the user asks for general guidance. Context JSON: {}",
                    context
                )));
            }
        }
    } else {
        messages.push(system_message(
            "You are in build chat. You may suggest dashboard changes, but changes are applied only through explicit Datrina apply commands after user confirmation. Do not claim that a dashboard was changed unless the UI applied it.".to_string(),
        ));
    }

    messages.extend(session.messages.clone());
    Ok(messages)
}

fn system_message(content: String) -> ChatMessage {
    ChatMessage {
        id: "runtime-system-context".to_string(),
        role: MessageRole::System,
        content,
        mode: ChatMode::Context,
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

#[tauri::command]
pub async fn delete_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_chat_session(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}
