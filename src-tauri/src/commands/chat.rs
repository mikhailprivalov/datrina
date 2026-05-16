use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures::FutureExt;
use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::models::chat::{
    AgentEvent, AgentPhase, AgentPhaseStatus, ChatEventEnvelope, ChatEventKind, ChatMessage,
    ChatMessagePart, ChatMode, ChatSession, CreateSessionRequest, MessageMetadata, MessageRole,
    SendMessageRequest, ToolCallTrace, ToolPolicyDecision, ToolResult, ToolResultTrace,
    ToolTraceStatus, CHAT_EVENT_CHANNEL,
};
use crate::models::dashboard::BuildProposal;
use crate::models::mcp::{MCPServer, MCPTransport};
use crate::models::memory::{MemoryHit, Scope};
use crate::models::validation::ValidationIssue;
use crate::models::ApiResult;
use crate::modules::ai::{AIStreamEvent, AIToolSpec};
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
pub async fn list_session_summaries(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<crate::models::chat::ChatSessionSummary>>, String> {
    Ok(match state.storage.list_chat_session_summaries().await {
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
        parts: text_parts(&req.content),
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
                "AI chat unavailable: configure an enabled provider or local_mock dev/test provider first"
                    .to_string(),
            ));
        }
    };

    let prompt_mcp_server = match extract_prompt_mcp_server(&req.content) {
        Some(server) => {
            if let Err(e) = state.tool_engine.validate_mcp_server(&server) {
                return Ok(ApiResult::err(format!(
                    "Prompt MCP server is not allowed by tool policy: {}",
                    e
                )));
            }
            if let Err(e) = state.storage.save_mcp_server(&server).await {
                return Ok(ApiResult::err(format!(
                    "Failed to save prompt MCP server: {}",
                    e
                )));
            }
            Some(server)
        }
        None => None,
    };

    let mut provider_messages = match grounded_messages(state.inner(), &session).await {
        Ok(messages) => messages,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    if let Some(server) = prompt_mcp_server.as_ref() {
        provider_messages.push(system_message(prompt_mcp_system_message(server)));
    }

    let tool_specs = match chat_tool_specs_silent(state.inner(), prompt_mcp_server.is_some()).await
    {
        Ok(specs) => specs,
        Err(e) => return Ok(ApiResult::err(format!("MCP tool discovery failed: {}", e))),
    };
    let ai_response = match state
        .ai_engine
        .complete_chat_with_tools(&provider, &provider_messages, &tool_specs)
        .await
    {
        Ok(response) => response,
        Err(e) => return Ok(ApiResult::err(format!("AI provider call failed: {}", e))),
    };

    let mut persisted_tool_calls = Vec::new();
    let mut persisted_tool_results = Vec::new();
    let mut final_content = ai_response.content.clone();
    let mut final_model = ai_response.model.clone();
    let mut final_provider_id = ai_response.provider_id.clone();
    let mut final_tokens = ai_response.tokens.clone();
    let mut final_latency_ms = ai_response.latency_ms;
    let mut final_reasoning = ai_response.reasoning.clone();

    if !ai_response.tool_calls.is_empty() {
        let assistant_tool_content = if ai_response.content.trim().is_empty() {
            "Tool call requested by provider.".to_string()
        } else {
            ai_response.content.clone()
        };
        let assistant_tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: assistant_tool_content.clone(),
            parts: assistant_parts(
                &assistant_tool_content,
                ai_response.reasoning.as_ref(),
                &ai_response.tool_calls,
                &[],
                None,
            ),
            mode: session.mode.clone(),
            tool_calls: Some(ai_response.tool_calls.clone()),
            tool_results: None,
            metadata: Some(MessageMetadata {
                model: Some(ai_response.model.clone()),
                provider: Some(ai_response.provider_id.clone()),
                tokens: ai_response.tokens.clone(),
                latency_ms: Some(ai_response.latency_ms),
                build_proposal: None,
                reasoning: ai_response.reasoning.clone(),
            }),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(assistant_tool_msg);

        for call in &ai_response.tool_calls {
            persisted_tool_results.push(execute_chat_tool(state.inner(), &session, call).await);
        }

        let tool_content = serde_json::to_string(&persisted_tool_results).unwrap_or_default();
        let tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: tool_content,
            parts: tool_result_parts(&persisted_tool_results),
            mode: session.mode.clone(),
            tool_calls: None,
            tool_results: Some(persisted_tool_results.clone()),
            metadata: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(tool_msg);

        let resumed_messages = match grounded_messages(state.inner(), &session).await {
            Ok(messages) => messages,
            Err(e) => return Ok(ApiResult::err(e.to_string())),
        };
        match state
            .ai_engine
            .complete_chat(&provider, &resumed_messages)
            .await
        {
            Ok(response) => {
                final_content = response.content;
                final_model = response.model;
                final_provider_id = response.provider_id;
                final_tokens = response.tokens;
                final_latency_ms = response.latency_ms;
                final_reasoning = response.reasoning;
                persisted_tool_calls.extend(response.tool_calls);
            }
            Err(e) => {
                final_content =
                    format!("Tool result was recorded, but provider resume failed: {e}");
            }
        }
    }

    let build_proposal = if matches!(session.mode, ChatMode::Build) {
        parse_build_proposal(&final_content)
    } else {
        None
    };

    let assistant_content = build_proposal
        .as_ref()
        .and_then(|proposal| proposal.summary.clone())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(final_content);
    let assistant_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::Assistant,
        content: assistant_content.clone(),
        parts: assistant_parts(
            &assistant_content,
            final_reasoning.as_ref(),
            &persisted_tool_calls,
            &persisted_tool_results,
            build_proposal.as_ref(),
        ),
        mode: session.mode.clone(),
        tool_calls: if persisted_tool_calls.is_empty() {
            None
        } else {
            Some(persisted_tool_calls)
        },
        tool_results: if persisted_tool_results.is_empty() {
            None
        } else {
            Some(persisted_tool_results)
        },
        metadata: Some(MessageMetadata {
            model: Some(final_model),
            provider: Some(final_provider_id),
            tokens: final_tokens,
            latency_ms: Some(final_latency_ms),
            build_proposal,
            reasoning: final_reasoning,
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

#[tauri::command]
pub async fn send_message_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    req: SendMessageRequest,
) -> Result<ApiResult<ChatMessage>, String> {
    let now = chrono::Utc::now().timestamp_millis();

    let mut session = match state.storage.get_chat_session(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(ApiResult::err("Session not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    let user_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        parts: text_parts(&req.content),
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
                "AI chat unavailable: configure an enabled provider or local_mock dev/test provider first"
                    .to_string(),
            ));
        }
    };

    let assistant_message_id = uuid::Uuid::new_v4().to_string();
    let synthetic_stream = !matches!(
        provider.kind,
        crate::models::provider::ProviderKind::Openrouter
            | crate::models::provider::ProviderKind::Custom
    );
    let abort_flag = Arc::new(AtomicBool::new(false));
    state
        .chat_abort_flags
        .insert(session_id.clone(), abort_flag.clone());

    let mut sequence = 0_u32;
    emit_chat_event(
        &app,
        ChatEventEnvelope {
            kind: ChatEventKind::MessageStarted,
            session_id: session_id.clone(),
            message_id: assistant_message_id.clone(),
            sequence: next_sequence(&mut sequence),
            agent_event: Some(AgentEvent::RunStarted),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: None,
            tool_result: None,
            build_proposal: None,
            final_message: None,
            error: None,
            synthetic: synthetic_stream,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );

    let draft = ChatMessage {
        id: assistant_message_id.clone(),
        role: MessageRole::Assistant,
        content: String::new(),
        parts: Vec::new(),
        mode: session.mode.clone(),
        tool_calls: None,
        tool_results: None,
        metadata: Some(MessageMetadata {
            model: Some(provider.default_model.clone()),
            provider: Some(provider.id.clone()),
            tokens: None,
            latency_ms: None,
            build_proposal: None,
            reasoning: None,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    };

    let app_for_task = app.clone();
    let state_for_task = state.inner().clone();
    let session_id_for_task = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut local_sequence = sequence;
        let outcome = AssertUnwindSafe(send_message_stream_inner(
            &app_for_task,
            &state_for_task,
            &mut session,
            &provider,
            &req,
            &assistant_message_id,
            &abort_flag,
            &mut local_sequence,
            synthetic_stream,
        ))
        .catch_unwind()
        .await;

        state_for_task.chat_abort_flags.remove(&session_id_for_task);

        let terminal: Option<(ChatEventKind, AgentEvent, String)> = match outcome {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some((
                failed_event_kind(&error),
                failed_agent_event(&error),
                error.to_string(),
            )),
            Err(panic) => {
                let message = panic_message(panic);
                tracing::error!(
                    "chat stream panicked: session={} message={} reason={}",
                    session_id_for_task,
                    assistant_message_id,
                    message
                );
                Some((
                    ChatEventKind::MessageFailed,
                    AgentEvent::RunError {
                        message: format!("chat_stream_panicked: {message}"),
                        recoverable: true,
                    },
                    format!("chat_stream_panicked: {message}"),
                ))
            }
        };

        if let Some((kind, agent_event, error_text)) = terminal {
            emit_chat_event(
                &app_for_task,
                ChatEventEnvelope {
                    kind,
                    session_id: session_id_for_task,
                    message_id: assistant_message_id,
                    sequence: next_sequence(&mut local_sequence),
                    agent_event: Some(agent_event),
                    provider_id: Some(provider.id),
                    model: Some(provider.default_model),
                    content_delta: None,
                    reasoning_delta: None,
                    reasoning: None,
                    tool_call: None,
                    tool_result: None,
                    build_proposal: None,
                    final_message: None,
                    error: Some(error_text),
                    synthetic: synthetic_stream,
                    emitted_at: chrono::Utc::now().timestamp_millis(),
                },
            );
        }
    });

    Ok(ApiResult::ok(draft))
}

#[tauri::command]
pub async fn cancel_chat_response(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ApiResult<bool>, String> {
    if let Some(flag) = state.chat_abort_flags.get(&session_id) {
        flag.store(true, Ordering::SeqCst);
        Ok(ApiResult::ok(true))
    } else {
        Ok(ApiResult::ok(false))
    }
}

async fn send_message_stream_inner(
    app: &AppHandle,
    state: &AppState,
    session: &mut ChatSession,
    provider: &crate::models::provider::LLMProvider,
    req: &SendMessageRequest,
    assistant_message_id: &str,
    abort_flag: &Arc<AtomicBool>,
    sequence: &mut u32,
    synthetic_stream: bool,
) -> anyhow::Result<ChatMessage> {
    let prompt_mcp_server = match extract_prompt_mcp_server(&req.content) {
        Some(server) => {
            if let Err(e) = state.tool_engine.validate_mcp_server(&server) {
                return Err(anyhow::anyhow!(
                    "Prompt MCP server is not allowed by tool policy: {}",
                    e
                ));
            }
            state
                .storage
                .save_mcp_server(&server)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to save prompt MCP server: {}", e))?;
            Some(server)
        }
        None => None,
    };

    let mut provider_messages = grounded_messages(state, session).await?;
    if let Some(server) = prompt_mcp_server.as_ref() {
        provider_messages.push(system_message(prompt_mcp_system_message(server)));
    }

    emit_phase_event(
        app,
        &session.id,
        assistant_message_id,
        sequence,
        provider,
        AgentPhase::McpReconnect,
        AgentPhaseStatus::Started,
        None,
        synthetic_stream,
    );
    let tool_specs = match chat_tool_specs(
        app,
        state,
        &session.id,
        assistant_message_id,
        sequence,
        provider,
        prompt_mcp_server.is_some(),
        synthetic_stream,
    )
    .await
    {
        Ok(specs) => {
            let mcp_tool_count = specs.iter().filter(|spec| spec.name == "mcp_tool").count();
            let detail = format!(
                "{} tool spec(s) ready ({} MCP)",
                specs.len(),
                mcp_tool_count
            );
            emit_phase_event(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                AgentPhase::McpReconnect,
                AgentPhaseStatus::Completed,
                Some(detail),
                synthetic_stream,
            );
            specs
        }
        Err(error) => {
            emit_phase_event(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                AgentPhase::McpReconnect,
                AgentPhaseStatus::Failed,
                Some(error.to_string()),
                synthetic_stream,
            );
            return Err(anyhow::anyhow!("MCP tool discovery failed: {}", error));
        }
    };

    emit_phase_event(
        app,
        &session.id,
        assistant_message_id,
        sequence,
        provider,
        AgentPhase::ProviderRequest,
        AgentPhaseStatus::Started,
        None,
        synthetic_stream,
    );
    let provider_request_started = Instant::now();
    let emit_app = app.clone();
    let emit_session_id = session.id.clone();
    let emit_message_id = assistant_message_id.to_string();
    let provider_id = provider.id.clone();
    let model = provider.default_model.clone();
    let cancel_for_provider = abort_flag.clone();
    let mut content_seen = String::new();
    let mut reasoning_seen = String::new();
    let mut first_byte_reported = false;

    let ai_response = state
        .ai_engine
        .complete_chat_with_tools_streaming(
            provider,
            &provider_messages,
            &tool_specs,
            |event| match event {
                AIStreamEvent::ContentDelta(delta) => {
                    if !first_byte_reported {
                        first_byte_reported = true;
                        let elapsed_ms = provider_request_started.elapsed().as_millis() as u64;
                        emit_phase_event(
                            &emit_app,
                            &emit_session_id,
                            &emit_message_id,
                            sequence,
                            provider,
                            AgentPhase::ProviderFirstByte,
                            AgentPhaseStatus::Completed,
                            Some(format!("first byte after {}ms", elapsed_ms)),
                            synthetic_stream,
                        );
                    }
                    if content_seen.is_empty() {
                        info!(
                            "chat first content delta: session={} message={} chars={}",
                            emit_session_id,
                            emit_message_id,
                            delta.chars().count()
                        );
                    }
                    content_seen.push_str(&delta);
                    emit_chat_event(
                        &emit_app,
                        ChatEventEnvelope {
                            kind: ChatEventKind::ContentDelta,
                            session_id: emit_session_id.clone(),
                            message_id: emit_message_id.clone(),
                            sequence: next_sequence(sequence),
                            agent_event: Some(AgentEvent::TextDelta {
                                text: delta.clone(),
                            }),
                            provider_id: Some(provider_id.clone()),
                            model: Some(model.clone()),
                            content_delta: Some(delta),
                            reasoning_delta: None,
                            reasoning: None,
                            tool_call: None,
                            tool_result: None,
                            build_proposal: None,
                            final_message: None,
                            error: None,
                            synthetic: synthetic_stream,
                            emitted_at: chrono::Utc::now().timestamp_millis(),
                        },
                    );
                }
                AIStreamEvent::ReasoningDelta(delta) => {
                    if !first_byte_reported {
                        first_byte_reported = true;
                        let elapsed_ms = provider_request_started.elapsed().as_millis() as u64;
                        emit_phase_event(
                            &emit_app,
                            &emit_session_id,
                            &emit_message_id,
                            sequence,
                            provider,
                            AgentPhase::ProviderFirstByte,
                            AgentPhaseStatus::Completed,
                            Some(format!("first byte after {}ms (reasoning)", elapsed_ms)),
                            synthetic_stream,
                        );
                    }
                    if reasoning_seen.is_empty() {
                        info!(
                            "chat first reasoning delta: session={} message={} chars={}",
                            emit_session_id,
                            emit_message_id,
                            delta.chars().count()
                        );
                    }
                    reasoning_seen.push_str(&delta);
                    emit_chat_event(
                        &emit_app,
                        ChatEventEnvelope {
                            kind: ChatEventKind::ReasoningDelta,
                            session_id: emit_session_id.clone(),
                            message_id: emit_message_id.clone(),
                            sequence: next_sequence(sequence),
                            agent_event: Some(AgentEvent::ReasoningDelta {
                                text: delta.clone(),
                            }),
                            provider_id: Some(provider_id.clone()),
                            model: Some(model.clone()),
                            content_delta: None,
                            reasoning_delta: Some(delta),
                            reasoning: None,
                            tool_call: None,
                            tool_result: None,
                            build_proposal: None,
                            final_message: None,
                            error: None,
                            synthetic: synthetic_stream,
                            emitted_at: chrono::Utc::now().timestamp_millis(),
                        },
                    );
                }
            },
            || cancel_for_provider.load(Ordering::SeqCst),
        )
        .await;

    let ai_response = match ai_response {
        Ok(response) => response,
        Err(e) if !content_seen.trim().is_empty() => {
            tracing::warn!(
                "first-pass stream errored mid-stream but kept {} content chars, treating as final",
                content_seen.chars().count()
            );
            emit_phase_event(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                AgentPhase::ProviderRequest,
                AgentPhaseStatus::Failed,
                Some(format!(
                    "mid-stream error after {} chars: {}",
                    content_seen.chars().count(),
                    e
                )),
                synthetic_stream,
            );
            crate::modules::ai::AIResponse {
                content: content_seen.clone(),
                provider_id: provider.id.clone(),
                model: provider.default_model.clone(),
                tokens: None,
                latency_ms: provider_request_started.elapsed().as_millis() as u64,
                tool_calls: Vec::new(),
                reasoning: non_empty(reasoning_seen.clone()),
            }
        }
        Err(e) => {
            emit_phase_event(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                AgentPhase::ProviderRequest,
                AgentPhaseStatus::Failed,
                Some(e.to_string()),
                synthetic_stream,
            );
            return Err(anyhow::anyhow!("AI provider stream failed: {}", e));
        }
    };
    emit_phase_event(
        app,
        &session.id,
        assistant_message_id,
        sequence,
        provider,
        AgentPhase::ProviderRequest,
        AgentPhaseStatus::Completed,
        Some(format!(
            "{} content chars, {} reasoning chars, {} tool call(s)",
            ai_response.content.chars().count(),
            ai_response
                .reasoning
                .as_ref()
                .map(|value| value.chars().count())
                .unwrap_or(0),
            ai_response.tool_calls.len()
        )),
        synthetic_stream,
    );
    info!(
        "chat stream first pass complete: session={} message={} content_chars={} reasoning_chars={} tool_calls={}",
        session.id,
        assistant_message_id,
        ai_response.content.chars().count(),
        ai_response.reasoning.as_ref().map(|value| value.chars().count()).unwrap_or(0),
        ai_response.tool_calls.len()
    );

    let mut persisted_tool_calls = Vec::new();
    let mut persisted_tool_results = Vec::new();
    let mut final_content = ai_response.content.clone();
    let mut final_model = ai_response.model.clone();
    let mut final_provider_id = ai_response.provider_id.clone();
    let mut final_tokens = ai_response.tokens.clone();
    let mut final_latency_ms = ai_response.latency_ms;
    let mut final_reasoning = ai_response
        .reasoning
        .clone()
        .or_else(|| non_empty(reasoning_seen.clone()));

    if !reasoning_seen.trim().is_empty() {
        emit_chat_event(
            app,
            ChatEventEnvelope {
                kind: ChatEventKind::ReasoningSnapshot,
                session_id: session.id.clone(),
                message_id: assistant_message_id.to_string(),
                sequence: next_sequence(sequence),
                agent_event: Some(AgentEvent::ReasoningEnd {
                    text: reasoning_seen.clone(),
                }),
                provider_id: Some(provider.id.clone()),
                model: Some(provider.default_model.clone()),
                content_delta: None,
                reasoning_delta: None,
                reasoning: Some(reasoning_seen.clone()),
                tool_call: None,
                tool_result: None,
                build_proposal: None,
                final_message: None,
                error: None,
                synthetic: synthetic_stream,
                emitted_at: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    const MAX_TOOL_ITERATIONS: u8 = 40;
    /// W16: tool-call dedup window. We track the last 5 `(name, canonical_args)`
    /// keys; a third identical call inside the window short-circuits with a
    /// synthetic `loop_detected` tool result so the agent's iteration budget
    /// is spent on novel work, not on stuck retries of the same MCP call.
    const TOOL_LOOP_WINDOW: usize = 5;
    const TOOL_LOOP_REPEAT_THRESHOLD: usize = 2; // > this many priors == abort
    let mut tool_call_history: VecDeque<(String, String)> =
        VecDeque::with_capacity(TOOL_LOOP_WINDOW);
    let mut pending_tool_response = Some(ai_response);
    let mut tool_iteration = 0_u8;
    while pending_tool_response
        .as_ref()
        .is_some_and(|response| !response.tool_calls.is_empty())
    {
        let response_with_tools = pending_tool_response
            .take()
            .expect("pending response was checked above");
        tool_iteration += 1;
        if tool_iteration > MAX_TOOL_ITERATIONS {
            final_content = format!(
                "Stopped after {MAX_TOOL_ITERATIONS} tool-call iterations to keep the agent loop bounded. Ask again to continue from here."
            );
            break;
        }
        persisted_tool_calls.extend(response_with_tools.tool_calls.clone());
        let assistant_tool_content = if response_with_tools.content.trim().is_empty() {
            "Tool call requested by provider.".to_string()
        } else {
            response_with_tools.content.clone()
        };
        let assistant_tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: assistant_tool_content.clone(),
            parts: assistant_parts(
                &assistant_tool_content,
                response_with_tools.reasoning.as_ref(),
                &response_with_tools.tool_calls,
                &[],
                None,
            ),
            mode: session.mode.clone(),
            tool_calls: Some(response_with_tools.tool_calls.clone()),
            tool_results: None,
            metadata: Some(MessageMetadata {
                model: Some(response_with_tools.model.clone()),
                provider: Some(response_with_tools.provider_id.clone()),
                tokens: response_with_tools.tokens.clone(),
                latency_ms: Some(response_with_tools.latency_ms),
                build_proposal: None,
                reasoning: response_with_tools.reasoning.clone(),
            }),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(assistant_tool_msg);

        let mut current_tool_results = Vec::new();
        for call in &response_with_tools.tool_calls {
            let dedup_key = (call.name.clone(), canonical_json_string(&call.arguments));
            let recent_repeats = count_recent_repeats(&tool_call_history, &dedup_key);
            tool_call_history.push_back(dedup_key.clone());
            while tool_call_history.len() > TOOL_LOOP_WINDOW {
                tool_call_history.pop_front();
            }

            emit_tool_call_event(
                app,
                session,
                assistant_message_id,
                sequence,
                provider,
                call,
                ToolTraceStatus::Requested,
                synthetic_stream,
            );

            let result = if recent_repeats > TOOL_LOOP_REPEAT_THRESHOLD {
                // Short-circuit: same tool + same arguments seen too many
                // times in the recent window. Skip the actual call and
                // hand the agent a synthetic "stop repeating yourself"
                // tool result instead of burning more provider turns.
                tracing::warn!(
                    "chat tool loop detected: session={} message={} iteration={} tool={} repeats={}",
                    session.id,
                    assistant_message_id,
                    tool_iteration,
                    call.name,
                    recent_repeats + 1
                );
                emit_phase_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    AgentPhase::LoopDetected {
                        tool_name: call.name.clone(),
                    },
                    AgentPhaseStatus::Failed,
                    Some(format!(
                        "skipped duplicate call ({} identical) — agent must vary the request or finalize",
                        recent_repeats + 1
                    )),
                    synthetic_stream,
                );
                emit_tool_call_event(
                    app,
                    session,
                    assistant_message_id,
                    sequence,
                    provider,
                    call,
                    ToolTraceStatus::Error,
                    synthetic_stream,
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    name: call.name.clone(),
                    result: serde_json::json!({
                        "status": "loop_detected",
                        "repeated_calls": recent_repeats + 1,
                    }),
                    error: Some(format!(
                        "loop_detected: tool '{}' was called with identical arguments {} times in a row. The result is unchanged. Either vary the arguments, switch tools, or finalize your answer/proposal.",
                        call.name,
                        recent_repeats + 1
                    )),
                }
            } else {
                info!(
                    "chat tool call started: session={} message={} iteration={} tool={}",
                    session.id, assistant_message_id, tool_iteration, call.name
                );
                emit_tool_call_event(
                    app,
                    session,
                    assistant_message_id,
                    sequence,
                    provider,
                    call,
                    ToolTraceStatus::Running,
                    synthetic_stream,
                );
                let result = execute_chat_tool(state, session, call).await;
                info!(
                    "chat tool call finished: session={} message={} iteration={} tool={} status={}",
                    session.id,
                    assistant_message_id,
                    tool_iteration,
                    call.name,
                    if result.error.is_some() {
                        "error"
                    } else {
                        "success"
                    }
                );
                result
            };

            emit_tool_result_event(
                app,
                session,
                assistant_message_id,
                sequence,
                provider,
                &result,
                synthetic_stream,
            );
            current_tool_results.push(result.clone());
            persisted_tool_results.push(result);
        }

        let tool_content = serde_json::to_string(&current_tool_results).unwrap_or_default();
        let tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: tool_content,
            parts: tool_result_parts(&current_tool_results),
            mode: session.mode.clone(),
            tool_calls: None,
            tool_results: Some(current_tool_results),
            metadata: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(tool_msg);

        let resumed_messages = grounded_messages(state, session).await?;
        info!(
            "chat stream resume started: session={} message={} iteration={} total_tool_results={}",
            session.id,
            assistant_message_id,
            tool_iteration,
            persisted_tool_results.len()
        );
        emit_phase_event(
            app,
            &session.id,
            assistant_message_id,
            sequence,
            provider,
            AgentPhase::ToolResume {
                iteration: tool_iteration,
            },
            AgentPhaseStatus::Started,
            Some(format!(
                "{} tool result(s) feeding into resume",
                persisted_tool_results.len()
            )),
            synthetic_stream,
        );
        let resume_cancel = abort_flag.clone();
        let mut resume_content_seen = String::new();
        let resumed = state
            .ai_engine
            .complete_chat_with_tools_streaming(
                provider,
                &resumed_messages,
                &tool_specs,
                |event| match event {
                    AIStreamEvent::ContentDelta(delta) => {
                        if resume_content_seen.is_empty() {
                            info!(
                                "chat resume first content delta: session={} message={} iteration={} chars={}",
                                session.id,
                                assistant_message_id,
                                tool_iteration,
                                delta.chars().count()
                            );
                        }
                        resume_content_seen.push_str(&delta);
                        emit_chat_event(
                            app,
                            ChatEventEnvelope {
                                kind: ChatEventKind::ContentDelta,
                                session_id: session.id.clone(),
                                message_id: assistant_message_id.to_string(),
                                sequence: next_sequence(sequence),
                                agent_event: Some(AgentEvent::TextDelta {
                                    text: delta.clone(),
                                }),
                                provider_id: Some(provider.id.clone()),
                                model: Some(provider.default_model.clone()),
                                content_delta: Some(delta),
                                reasoning_delta: None,
                                reasoning: None,
                                tool_call: None,
                                tool_result: None,
                                build_proposal: None,
                                final_message: None,
                                error: None,
                                synthetic: synthetic_stream,
                                emitted_at: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                    }
                    AIStreamEvent::ReasoningDelta(delta) => {
                        reasoning_seen.push_str(&delta);
                        emit_chat_event(
                            app,
                            ChatEventEnvelope {
                                kind: ChatEventKind::ReasoningDelta,
                                session_id: session.id.clone(),
                                message_id: assistant_message_id.to_string(),
                                sequence: next_sequence(sequence),
                                agent_event: Some(AgentEvent::ReasoningDelta {
                                    text: delta.clone(),
                                }),
                                provider_id: Some(provider.id.clone()),
                                model: Some(provider.default_model.clone()),
                                content_delta: None,
                                reasoning_delta: Some(delta),
                                reasoning: None,
                                tool_call: None,
                                tool_result: None,
                                build_proposal: None,
                                final_message: None,
                                error: None,
                                synthetic: synthetic_stream,
                                emitted_at: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                    }
                },
                || resume_cancel.load(Ordering::SeqCst),
            )
            .await;

        match resumed {
            Ok(response) => {
                info!(
                    "chat stream resume complete: session={} message={} iteration={} content_chars={} reasoning_chars={} tool_calls={}",
                    session.id,
                    assistant_message_id,
                    tool_iteration,
                    response.content.chars().count(),
                    response.reasoning.as_ref().map(|value| value.chars().count()).unwrap_or(0),
                    response.tool_calls.len()
                );
                emit_phase_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    AgentPhase::ToolResume {
                        iteration: tool_iteration,
                    },
                    AgentPhaseStatus::Completed,
                    Some(format!(
                        "{} content chars, {} tool call(s)",
                        response.content.chars().count(),
                        response.tool_calls.len()
                    )),
                    synthetic_stream,
                );
                final_content = response.content.clone();
                final_model = response.model.clone();
                final_provider_id = response.provider_id.clone();
                final_tokens = response.tokens.clone();
                final_latency_ms = response.latency_ms;
                final_reasoning = response
                    .reasoning
                    .clone()
                    .or_else(|| non_empty(reasoning_seen.clone()));
                pending_tool_response = Some(response);
            }
            Err(e) => {
                info!(
                    "chat stream resume failed: session={} message={} iteration={} error={} resume_chars={}",
                    session.id,
                    assistant_message_id,
                    tool_iteration,
                    e,
                    resume_content_seen.chars().count()
                );
                emit_phase_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    AgentPhase::ToolResume {
                        iteration: tool_iteration,
                    },
                    AgentPhaseStatus::Failed,
                    Some(e.to_string()),
                    synthetic_stream,
                );
                // If we already streamed real content before the connection
                // broke, treat it as the final answer instead of dropping the
                // run. This recovers from transient mid-stream decode errors
                // on long Kimi/OpenRouter responses.
                if !resume_content_seen.trim().is_empty() {
                    tracing::warn!(
                        "chat stream resume errored mid-stream but kept {} chars of accumulated content",
                        resume_content_seen.chars().count()
                    );
                    final_content = resume_content_seen.clone();
                    final_reasoning = non_empty(reasoning_seen.clone()).or(final_reasoning);
                } else {
                    final_content =
                        format!("Tool result was recorded, but provider resume failed: {e}");
                }
                pending_tool_response = None;
            }
        }

        if !pending_tool_response
            .as_ref()
            .is_some_and(|response| !response.tool_calls.is_empty())
        {
            break;
        }
    }

    if let Some(final_response) =
        pending_tool_response.filter(|response| response.tool_calls.is_empty())
    {
        final_content = final_response.content;
        final_model = final_response.model;
        final_provider_id = final_response.provider_id;
        final_tokens = final_response.tokens;
        final_latency_ms = final_response.latency_ms;
        final_reasoning = final_response
            .reasoning
            .or_else(|| non_empty(reasoning_seen.clone()));
    }

    let mut build_proposal = if matches!(session.mode, ChatMode::Build) {
        parse_build_proposal(&final_content)
    } else {
        None
    };

    // W16: proposal validation gate. The proposal the agent just emitted
    // gets a deterministic structural pass. If issues are found, the
    // validator hands them back to the agent inside a synthetic system
    // message and runs one non-streaming retry turn before showing the
    // result to the user.
    let mut residual_validation_issues: Vec<ValidationIssue> = Vec::new();
    let mut validation_retried = false;

    if matches!(session.mode, ChatMode::Build) {
        if let Some(initial_proposal) = build_proposal.as_ref() {
            let dashboard_for_validation = match session.dashboard_id.as_deref() {
                Some(dashboard_id) => state
                    .storage
                    .get_dashboard(dashboard_id)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };

            let initial_issues = crate::commands::validation::validate_build_proposal(
                initial_proposal,
                dashboard_for_validation.as_ref(),
                &session.messages,
            );

            if initial_issues.is_empty() {
                emit_validation_result_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    AgentPhaseStatus::Completed,
                    Vec::new(),
                    false,
                    synthetic_stream,
                );
            } else {
                tracing::warn!(
                    "proposal validation found {} issue(s); attempting retry",
                    initial_issues.len()
                );
                emit_validation_result_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    AgentPhaseStatus::Started,
                    initial_issues.clone(),
                    false,
                    synthetic_stream,
                );

                const MAX_VALIDATION_RETRIES: u8 = 1;
                let mut retries = 0_u8;
                let mut current_issues = initial_issues;

                while retries < MAX_VALIDATION_RETRIES && !current_issues.is_empty() {
                    retries += 1;
                    validation_retried = true;

                    let feedback =
                        crate::commands::validation::format_issues_for_agent(&current_issues);
                    let mut retry_messages = match grounded_messages(state, session).await {
                        Ok(messages) => messages,
                        Err(e) => {
                            tracing::warn!("validation retry: grounded_messages failed: {}", e);
                            break;
                        }
                    };
                    retry_messages.push(system_message(format!(
                        "[validation_failed] Your most recent proposal did not pass automated validation. Issues:\n\n{feedback}\nEmit a corrected BuildProposal JSON object directly as your next message. Address every issue above. Do not call any more tools unless absolutely necessary."
                    )));

                    let retry_response = match state
                        .ai_engine
                        .complete_chat_with_tools_json_object(
                            provider,
                            &retry_messages,
                            &tool_specs,
                        )
                        .await
                    {
                        Ok(response) => response,
                        Err(e) => {
                            tracing::warn!(
                                "validation retry call failed: {} — keeping pre-retry proposal",
                                e
                            );
                            break;
                        }
                    };

                    if retry_response.content.trim().is_empty() {
                        tracing::warn!(
                            "validation retry returned empty content (tool_calls={}); keeping pre-retry proposal",
                            retry_response.tool_calls.len()
                        );
                        break;
                    }

                    final_content = retry_response.content;
                    final_model = retry_response.model;
                    final_provider_id = retry_response.provider_id;
                    final_tokens = retry_response.tokens;
                    final_latency_ms += retry_response.latency_ms;
                    if let Some(retry_reasoning) = retry_response.reasoning {
                        final_reasoning = Some(retry_reasoning);
                    }

                    build_proposal = parse_build_proposal(&final_content);
                    current_issues = match build_proposal.as_ref() {
                        Some(updated) => crate::commands::validation::validate_build_proposal(
                            updated,
                            dashboard_for_validation.as_ref(),
                            &session.messages,
                        ),
                        None => current_issues,
                    };
                }

                let final_status = if current_issues.is_empty() {
                    AgentPhaseStatus::Completed
                } else {
                    AgentPhaseStatus::Failed
                };
                emit_validation_result_event(
                    app,
                    &session.id,
                    assistant_message_id,
                    sequence,
                    provider,
                    final_status,
                    current_issues.clone(),
                    validation_retried,
                    synthetic_stream,
                );
                residual_validation_issues = current_issues;
            }
        }
    }

    if let Some(proposal) = build_proposal.clone() {
        emit_chat_event(
            app,
            ChatEventEnvelope {
                kind: ChatEventKind::BuildProposalParsed,
                session_id: session.id.clone(),
                message_id: assistant_message_id.to_string(),
                sequence: next_sequence(sequence),
                agent_event: Some(AgentEvent::BuildProposal {
                    proposal: proposal.clone(),
                }),
                provider_id: Some(final_provider_id.clone()),
                model: Some(final_model.clone()),
                content_delta: None,
                reasoning_delta: None,
                reasoning: None,
                tool_call: None,
                tool_result: None,
                build_proposal: Some(proposal),
                final_message: None,
                error: None,
                synthetic: synthetic_stream,
                emitted_at: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    let _ = residual_validation_issues; // surfaced via the typed validation event

    let assistant_content = build_proposal
        .as_ref()
        .and_then(|proposal| proposal.summary.clone())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(final_content);
    let assistant_msg = ChatMessage {
        id: assistant_message_id.to_string(),
        role: MessageRole::Assistant,
        content: assistant_content.clone(),
        parts: assistant_parts(
            &assistant_content,
            final_reasoning.as_ref(),
            &persisted_tool_calls,
            &persisted_tool_results,
            build_proposal.as_ref(),
        ),
        mode: session.mode.clone(),
        tool_calls: if persisted_tool_calls.is_empty() {
            None
        } else {
            Some(persisted_tool_calls)
        },
        tool_results: if persisted_tool_results.is_empty() {
            None
        } else {
            Some(persisted_tool_results)
        },
        metadata: Some(MessageMetadata {
            model: Some(final_model),
            provider: Some(final_provider_id),
            tokens: final_tokens,
            latency_ms: Some(final_latency_ms),
            build_proposal,
            reasoning: final_reasoning,
        }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    session.messages.push(assistant_msg.clone());
    session.updated_at = chrono::Utc::now().timestamp_millis();

    state.storage.update_chat_session(session).await?;
    info!(
        "chat message completed: session={} message={} content_chars={} has_proposal={} has_reasoning={}",
        session.id,
        assistant_message_id,
        assistant_msg.content.chars().count(),
        assistant_msg
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.build_proposal.as_ref())
            .is_some(),
        assistant_msg
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.reasoning.as_ref())
            .is_some()
    );

    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::MessageCompleted,
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(AgentEvent::RunFinished),
            provider_id: assistant_msg
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.provider.clone()),
            model: assistant_msg
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: assistant_msg
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.reasoning.clone()),
            tool_call: None,
            tool_result: None,
            build_proposal: assistant_msg
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.build_proposal.clone()),
            final_message: Some(Box::new(assistant_msg.clone())),
            error: None,
            synthetic: synthetic_stream,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );

    let _ = content_seen;
    Ok(assistant_msg)
}

fn next_sequence(sequence: &mut u32) -> u32 {
    *sequence += 1;
    *sequence
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        return (*message).to_string();
    }
    if let Some(message) = panic.downcast_ref::<String>() {
        return message.clone();
    }
    "non-string panic payload".to_string()
}

/// W16: emit a `ChatEventKind::ProposalValidation` envelope carrying the
/// typed `AgentEvent::ProposalValidationResult` so the UI can render the
/// structured issue list (or a clean "validator passed" tile).
#[allow(clippy::too_many_arguments)]
fn emit_validation_result_event(
    app: &AppHandle,
    session_id: &str,
    message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    status: AgentPhaseStatus,
    issues: Vec<ValidationIssue>,
    retried: bool,
    synthetic: bool,
) {
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::ProposalValidation,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(AgentEvent::ProposalValidationResult {
                status,
                issues,
                retried,
            }),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: None,
            tool_result: None,
            build_proposal: None,
            final_message: None,
            error: None,
            synthetic,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_phase_event(
    app: &AppHandle,
    session_id: &str,
    message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    phase: AgentPhase,
    status: AgentPhaseStatus,
    detail: Option<String>,
    synthetic: bool,
) {
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::AgentPhase,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(AgentEvent::AgentPhase {
                phase,
                status,
                detail,
            }),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: None,
            tool_result: None,
            build_proposal: None,
            final_message: None,
            error: None,
            synthetic,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );
}

fn emit_chat_event(app: &AppHandle, event: ChatEventEnvelope) {
    if let Err(window_error) = app.emit_to("main", CHAT_EVENT_CHANNEL, event.clone()) {
        tracing::warn!("failed to emit chat event to main window: {}", window_error);
        if let Err(app_error) = app.emit(CHAT_EVENT_CHANNEL, event) {
            tracing::warn!("failed to emit app-wide chat event: {}", app_error);
        }
    }
}

fn emit_tool_call_event(
    app: &AppHandle,
    session: &ChatSession,
    assistant_message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    call: &crate::models::chat::ToolCall,
    status: ToolTraceStatus,
    synthetic_stream: bool,
) {
    let arguments_preview = preview_json(&call.arguments);
    let event_kind = match &status {
        ToolTraceStatus::Requested => ChatEventKind::ToolCallRequested,
        _ => ChatEventKind::ToolExecutionStarted,
    };
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: event_kind,
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(match &status {
                ToolTraceStatus::Requested => AgentEvent::ToolCallStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments_preview: arguments_preview.clone(),
                    policy_decision: ToolPolicyDecision::Accepted,
                },
                _ => AgentEvent::ToolCallEnd {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    status: status.clone(),
                },
            }),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: Some(ToolCallTrace {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments_preview,
                policy_decision: ToolPolicyDecision::Accepted,
                status,
            }),
            tool_result: None,
            build_proposal: None,
            final_message: None,
            error: None,
            synthetic: synthetic_stream,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );
}

fn emit_tool_result_event(
    app: &AppHandle,
    session: &ChatSession,
    assistant_message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    result: &ToolResult,
    synthetic_stream: bool,
) {
    let status = if result.error.is_some() {
        ToolTraceStatus::Error
    } else {
        ToolTraceStatus::Success
    };
    let result_preview = Some(preview_json(&result.result));
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::ToolResult,
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(AgentEvent::ToolResult {
                tool_call_id: result.tool_call_id.clone(),
                name: result.name.clone(),
                status: status.clone(),
                result_preview: result_preview.clone(),
                error: result.error.clone(),
            }),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: None,
            tool_result: Some(ToolResultTrace {
                tool_call_id: result.tool_call_id.clone(),
                name: result.name.clone(),
                status,
                result_preview,
                error: result.error.clone(),
            }),
            build_proposal: None,
            final_message: None,
            error: result.error.clone(),
            synthetic: synthetic_stream,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        },
    );
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// W16: deterministic serialisation for tool-call argument comparison.
/// `serde_json::to_string` order is sensitive to the input order of
/// objects when `preserve_order` is on; here we walk the value and sort
/// keys so `{"a":1,"b":2}` and `{"b":2,"a":1}` hash the same.
fn canonical_json_string(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Null => out.push_str("null"),
            serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            serde_json::Value::Number(n) => out.push_str(&n.to_string()),
            serde_json::Value::String(s) => out.push_str(
                &serde_json::to_string(s)
                    .unwrap_or_else(|_| format!("\"{}\"", s.replace('"', "'"))),
            ),
            serde_json::Value::Array(items) => {
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    walk(item, out);
                }
                out.push(']');
            }
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (idx, key) in keys.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(key).unwrap_or_else(|_| format!("\"{}\"", key)),
                    );
                    out.push(':');
                    walk(&map[*key], out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    walk(value, &mut out);
    out
}

/// W16: how many times the same `(tool_name, canonical_args)` key
/// appears in the recent window. The current candidate is NOT in the
/// deque yet — callers count >= 2 in the window to declare "this would
/// be the third call with identical args".
fn count_recent_repeats(
    history: &std::collections::VecDeque<(String, String)>,
    key: &(String, String),
) -> usize {
    history.iter().filter(|entry| *entry == key).count()
}

fn text_parts(content: &str) -> Vec<ChatMessagePart> {
    if content.trim().is_empty() {
        Vec::new()
    } else {
        vec![ChatMessagePart::Text {
            text: content.to_string(),
        }]
    }
}

fn assistant_parts(
    content: &str,
    reasoning: Option<&String>,
    tool_calls: &[crate::models::chat::ToolCall],
    tool_results: &[ToolResult],
    build_proposal: Option<&BuildProposal>,
) -> Vec<ChatMessagePart> {
    let mut parts = text_parts(content);
    if let Some(reasoning) = reasoning.filter(|value| !value.trim().is_empty()) {
        parts.push(ChatMessagePart::VisibleReasoning {
            text: reasoning.clone(),
        });
    }
    for call in tool_calls {
        let result_status = tool_results
            .iter()
            .find(|result| result.tool_call_id == call.id)
            .map(|result| {
                if result.error.is_some() {
                    ToolTraceStatus::Error
                } else {
                    ToolTraceStatus::Success
                }
            })
            .unwrap_or(ToolTraceStatus::Requested);
        parts.push(ChatMessagePart::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments_preview: preview_json(&call.arguments),
            policy_decision: ToolPolicyDecision::Accepted,
            status: result_status,
        });
    }
    parts.extend(tool_result_parts(tool_results));
    if let Some(proposal) = build_proposal {
        parts.push(ChatMessagePart::BuildProposal {
            proposal: proposal.clone(),
        });
    }
    parts
}

fn tool_result_parts(tool_results: &[ToolResult]) -> Vec<ChatMessagePart> {
    tool_results
        .iter()
        .map(|result| ChatMessagePart::ToolResult {
            tool_call_id: result.tool_call_id.clone(),
            name: result.name.clone(),
            status: if result.error.is_some() {
                ToolTraceStatus::Error
            } else {
                ToolTraceStatus::Success
            },
            result_preview: Some(preview_json(&result.result)),
            error: result.error.clone(),
        })
        .collect()
}

fn failed_event_kind(error: &anyhow::Error) -> ChatEventKind {
    if error.to_string().contains("chat_stream_cancelled") {
        ChatEventKind::MessageCancelled
    } else {
        ChatEventKind::MessageFailed
    }
}

fn failed_agent_event(error: &anyhow::Error) -> AgentEvent {
    let message = error.to_string();
    if message.contains("chat_stream_cancelled") {
        AgentEvent::AbortCancel {
            reason: "cancelled by user".to_string(),
        }
    } else {
        AgentEvent::RunError {
            message,
            recoverable: true,
        }
    }
}

fn preview_json(value: &serde_json::Value) -> serde_json::Value {
    mask_json(value, 0)
}

fn mask_json(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    const MAX_STRING: usize = 240;
    const MAX_ARRAY: usize = 12;
    const MAX_OBJECT: usize = 24;

    if depth >= 5 {
        return serde_json::json!("...");
    }

    match value {
        serde_json::Value::Object(map) => {
            let mut next = serde_json::Map::new();
            for (index, (key, item)) in map.iter().enumerate() {
                if index >= MAX_OBJECT {
                    next.insert("_truncated".to_string(), serde_json::json!(true));
                    break;
                }
                if is_secret_key(key) {
                    next.insert(key.clone(), serde_json::json!("***"));
                } else {
                    next.insert(key.clone(), mask_json(item, depth + 1));
                }
            }
            serde_json::Value::Object(next)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .take(MAX_ARRAY)
                .map(|item| mask_json(item, depth + 1))
                .chain((items.len() > MAX_ARRAY).then(|| serde_json::json!("...")))
                .collect(),
        ),
        serde_json::Value::String(text) => {
            if looks_like_secret(text) {
                serde_json::json!("***")
            } else if text.chars().count() > MAX_STRING {
                serde_json::json!(format!(
                    "{}...",
                    text.chars().take(MAX_STRING).collect::<String>()
                ))
            } else {
                serde_json::json!(text)
            }
        }
        _ => value.clone(),
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    [
        "authorization",
        "api_key",
        "apikey",
        "token",
        "secret",
        "password",
        "key",
    ]
    .iter()
    .any(|part| normalized.contains(part))
}

fn looks_like_secret(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("Bearer ")
        || trimmed.starts_with("sk-")
        || (trimmed.len() >= 32
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '=' | ':')))
}

/// Token budget for memories injected into the system prompt. Roughly
/// equates to ~1500 tokens of relevant prior-session knowledge.
const MEMORY_INJECTION_CHARS: usize = 6000;
const MEMORY_INJECTION_TOP_N: usize = 8;

async fn grounded_messages(
    state: &AppState,
    session: &ChatSession,
) -> anyhow::Result<Vec<ChatMessage>> {
    let mut messages = Vec::new();
    match session.mode {
        ChatMode::Build => {
            messages.push(system_message(build_chat_system_prompt()));
            if let Some(dashboard_id) = session.dashboard_id.as_deref() {
                if let Some(dashboard) = state.storage.get_dashboard(dashboard_id).await? {
                    let widgets_summary = dashboard
                        .layout
                        .iter()
                        .map(|widget| {
                            serde_json::json!({
                                "id": widget.id(),
                                "title": widget.title(),
                                "type": widget_type_label(widget),
                            })
                        })
                        .collect::<Vec<_>>();
                    let context = serde_json::json!({
                        "id": dashboard.id,
                        "name": dashboard.name,
                        "description": dashboard.description,
                        "widgets": widgets_summary,
                    });
                    messages.push(system_message(format!(
                        r#"You are editing an EXISTING dashboard. Apply DELTA semantics, NOT full-replace.

Rules
- The widgets in `widgets` array are the CHANGES, not the full dashboard. Any widget you do NOT mention stays unchanged.
- To ADD a widget, include it in `widgets` without `replace_widget_id` — it appends below the current bottom row.
- To REPLACE / EDIT an existing widget, include it in `widgets` WITH `replace_widget_id: "<existing id>"`. The new widget keeps the original x/y/w/h slot unless you set them.
- To REMOVE widgets, list their ids in `proposal.remove_widget_ids: ["id1", "id2"]`. Don't include their replacements in `widgets`.
- If the user asks for one tweak (e.g. "make the chart bar instead of line"), return ONLY that widget with `replace_widget_id`, NOT the whole dashboard.
- Re-emit the full set of widgets ONLY if the user asks to rebuild from scratch.
- Match the style/conventions of the existing widgets when adding new ones.
- If the user complains the previous widget was hardcoded, the replacement MUST have a real `datasource_plan` + `pipeline` that fetches the value live. Don't just substitute one literal for another.

Existing dashboard (read these widget ids before deciding what to replace/remove):
{}"#,
                        context
                    )));
                }
            } else {
                messages.push(system_message(
                    "There is no active dashboard. Your proposal will CREATE a new dashboard."
                        .to_string(),
                ));
            }
        }
        ChatMode::Context => {
            messages.push(system_message(context_chat_system_prompt()));
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
                        "Selected dashboard context (ground answers in this when relevant):\n{}",
                        context
                    )));
                }
            }
        }
    }

    if let Some(memory_message) = build_memory_injection(state, session).await {
        messages.push(system_message(memory_message));
    }

    messages.extend(session.messages.clone());
    Ok(messages)
}

/// Pull the user's most recent message as the retrieval query, then pack
/// matching memories into a compact system snippet. Returns `None` when
/// nothing useful matches so the prompt stays uncluttered.
async fn build_memory_injection(state: &AppState, session: &ChatSession) -> Option<String> {
    let query = session
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, MessageRole::User))
        .map(|message| message.content.clone())
        .unwrap_or_default();
    if query.trim().is_empty() {
        return None;
    }

    let mut scopes: Vec<Scope> = Vec::new();
    if let Some(dashboard_id) = session.dashboard_id.as_ref() {
        scopes.push(Scope::Dashboard(dashboard_id.clone()));
    }
    // Surface memories for every enabled MCP server so the agent inherits
    // tool-shape lessons even on first turn for that server.
    if let Ok(servers) = state.storage.list_mcp_servers().await {
        for server in servers.into_iter().filter(|s| s.is_enabled) {
            scopes.push(Scope::McpServer(server.id));
        }
    }
    scopes.push(Scope::Session(session.id.clone()));
    scopes.push(Scope::Global);

    let hits = match state
        .memory_engine
        .retrieve(&query, &scopes, MEMORY_INJECTION_TOP_N)
        .await
    {
        Ok(hits) if !hits.is_empty() => hits,
        _ => return None,
    };

    Some(render_memory_section(&hits))
}

fn render_memory_section(hits: &[MemoryHit]) -> String {
    let mut body = String::from(
        "## Known facts from prior sessions (relevance-ranked, may be stale)\n\
         Use these to skip rediscovering tool shapes or user preferences. \
         If a fact contradicts what you observe now, trust the observation \
         and ignore the stale memory.\n\n",
    );
    for hit in hits {
        let prefix = match &hit.record.scope {
            Scope::Dashboard(id) => format!("[dashboard:{}]", short_id(id)),
            Scope::McpServer(id) => format!("[mcp:{}]", id),
            Scope::Session(id) => format!("[session:{}]", short_id(id)),
            Scope::Global => "[global]".to_string(),
        };
        let line = format!("- {} {}\n", prefix, hit.record.content.trim());
        if body.len() + line.len() > MEMORY_INJECTION_CHARS {
            body.push_str("- (truncated — more memories available via `recall`)\n");
            break;
        }
        body.push_str(&line);
    }
    body
}

fn short_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        id.chars().take(8).collect()
    }
}

fn widget_type_label(widget: &crate::models::widget::Widget) -> &'static str {
    use crate::models::widget::Widget;
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

fn system_message(content: String) -> ChatMessage {
    ChatMessage {
        id: "runtime-system-context".to_string(),
        role: MessageRole::System,
        content: content.clone(),
        parts: text_parts(&content),
        mode: ChatMode::Context,
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

fn build_chat_system_prompt() -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    format!(
        r#"You are the Datrina Build Chat agent. Datrina is a local-first desktop tool — think of it as a Grafana-style dashboard builder with provider-driven proposals. Dashboards are made of widgets, each backed by a real datasource (HTTP request, stdio MCP tool, or provider prompt).

How to work
1. Read what the user wants to see.
2. If you need real data, call tools. Call multiple in parallel when independent. Tools available: `http_request` (policy-gated HTTPS to public endpoints) and `mcp_tool` (any configured stdio MCP tool; use exact server_id + tool_name from its description).
3. Inspect each result. Refine arguments if the data is wrong or too generic. Cap yourself at ~3 informative calls per question - the agent loop hard-stops at 40 iterations.
4. When you have enough, emit ONE dashboard proposal as STRICT JSON (no markdown fences, no commentary, no trailing prose) and STOP calling tools.
5. If the user is just asking a question or chatting (not requesting a build), answer plainly in text - do NOT emit JSON.

Open-ended requests (skill: MCP exploration)
When the user says something broad ("build something useful from this MCP", "do whatever makes sense"):
- Treat it as an exploration task, not a guessing game. Don't pre-pick a tool from the name alone.
- Look at the `mcp_tool` description: it lists each server's tool_name, description, and input_schema.
- Call 2-4 DIFFERENT tools (in parallel when independent) with minimal/empty arguments to see what data each one actually returns.
- Compare the results, pick the one(s) that produce concrete useful data, and design widgets around that.
- If a tool requires arguments you can't guess, either try defaults from its input_schema or move on to the next tool.
- Only after you have real data, propose the dashboard.

Anti-loop discipline (read this before every tool call)
- After every tool result, write one short sentence to yourself: "X returned <shape>; this <does / doesn't> answer the question". Then decide: design now, or ONE more call.
- NEVER call the same tool_name with the same arguments twice. If you already have its result, reuse it from earlier in this conversation.
- If 3 different tools all returned empty or generic data, STOP exploring and either (a) ask the user one short clarifying question, or (b) emit a proposal with a `text` widget that honestly says "no useful data found for X" - don't keep grinding.
- The hard stop is 40 iterations of tool resume. You should normally finish in 1-3; iterations beyond ~5 usually mean you're going in circles - take stock instead of grinding.

Talking with the user
- You may ask the user a SHORT clarifying question (one question, one line) when the request is genuinely ambiguous AND no amount of tool exploration would resolve it (e.g. "which project?" when there are thousands). Don't ask before doing any exploration; ask after one round of inspection if still unclear.
- The user may correct you mid-flow ("no, use the other tool", "filter by X"). Treat each new user message as fresh intent. Acknowledge the correction in one short sentence, then re-plan. Don't argue, don't re-justify the old plan.
- If a previous proposal turned out wrong, briefly acknowledge ("That was the wrong tool, retrying with X") and produce a new one. Don't re-emit the same JSON unchanged.

Output style
- Use Markdown when answering in text: headers, bullet lists, **bold**, `inline code`, fenced code blocks for JSON, tables when comparing rows.
- For JSON proposals, output STRICTLY the JSON object as the whole message body - no Markdown around it.

Pick widgets by USE CASE (most important rule)
Map the user's intent to one of these patterns, then pick the widget(s):

1. KPI row at the top - 3 to 6 `stat` widgets, w 3-4 h 2 each, on the same y=0 line. Use for "current value", "how many", "latest X". Add `thresholds` and `color_mode='value'` when the number means good/bad. Optionally `graph_mode='sparkline'` if you have a numeric history.

2. Service / build / check health overview - `status_grid`. Items are `[{{name, status, detail?}}]`. Status strings like ok/warn/error/down/unknown map to colors automatically; override with `config.status_colors`. Layout: `grid` (default), `row` (pills), `compact` (heatmap-like dots).

3. Top-N comparison (releases by activity, requests by endpoint, errors by service) - `bar_gauge` with horizontal bars, OR `chart` kind=bar, OR an enhanced `table` with `format='progress'` column.

4. Trend / time-series - `chart` kind=line (or area for cumulative). `x_axis` is the time column. Multiple Y series via `y_axis` array. For just-the-shape sparklines inside a stat, use the stat's `graph_mode='sparkline'`.

5. Breakdown / share of total - `chart` kind=pie if <=6 categories, else kind=bar.

6. Tabular details / scannable rows - `table`. Always set `config.columns` with explicit `key`, `header`, `format` per column. Use cell formats: `status` (colored pill), `progress` (bar 0..100), `badge`, `link` (with `link_template`), `sparkline`, `date`, `currency`, `percent`, `number`.

7. Log / event stream - `logs`. Runtime data `[{{ts, level, message, source?}}]`. Levels info/warn/error are colored automatically. Use `config.reverse: true` for chronological order (default is newest-first).

8. Density / matrix data (usage by hour-of-day, errors by service-x-region, distribution buckets) - `heatmap`. Cells `[{{x, y, value}}]` or a 2D array. `color_scheme` is viridis/magma/cool/warm/green_red.

9. Single bounded value with thresholds (CPU %, queue depth vs capacity) - `gauge` with `min`, `max`, `thresholds`. Don't use gauge for unbounded counts; use `stat` instead.

10. Explanations, runbook links, instructions - `text` with `config.format='markdown'`. Heading + bullets are rendered.

11. Screenshot, generated chart image, status badge URL - `image`.

Don't repeat the same widget for the same data; combine views (KPI row + table + chart) for a complete dashboard.

Proposal JSON shape:
{{
  "id": "<short stable id>",
  "title": "<proposal title>",
  "summary": "<one-sentence human summary>",
  "dashboard_name": "<optional, only when creating a new dashboard>",
  "dashboard_description": "<optional>",
  "widgets": [
    {{
      "widget_type": "stat" | "gauge" | "chart" | "table" | "text" | "image" | "logs" | "bar_gauge" | "status_grid" | "heatmap",
      "title": "<widget title>",
      "replace_widget_id": "<optional - existing widget id to REPLACE; omit to ADD a new one>",
      "data": <small preview sample matching the runtime shape; see below; OPTIONAL>,
      "datasource_plan": {{
        "kind": "builtin_tool" | "mcp_tool" | "provider_prompt",
        "tool_name": "<http_request OR exact MCP tool name>",
        "server_id": "<required for mcp_tool>",
        "arguments": {{ }},
        "prompt": "<required for provider_prompt>",
        "output_path": "<optional dotted path inside the result to pick as widget data>",
        "refresh_cron": "<optional 6-field cron with seconds, e.g. '0 */15 * * * *' for every 15 minutes; omit for manual-only>",
        "pipeline": [ /* optional ordered deterministic transform steps - see Pipeline section below */ ]
      }},
      "config": {{ ...see below... }},
      "x": <int 0..23>, "y": <int>, "w": <int 1..24>, "h": <int 1..20>
    }}
  ],
  "remove_widget_ids": ["<optional ids of existing widgets to REMOVE>"]
}}

Delta semantics
- When editing an existing dashboard, `widgets` is a delta. List ONLY the widgets you are adding or replacing. Untouched widgets stay as is.
- Set `replace_widget_id` to overwrite an existing widget at the same slot. Set `remove_widget_ids` to drop widgets entirely.
- When creating a fresh dashboard (no existing widgets), list all widgets in `widgets` and leave `remove_widget_ids` empty.

Per-widget config and runtime data shapes (the workflow output is shaped to match)

All sizes assume a **12-col** grid. Typical sizes per widget kind:

- `stat` (size w 3, h 2 — fit 4 per row)
  runtime: `{{ value: number|string, delta?: number, label?: string, sparkline?: number[] | {{t,v}}[] }}`
  config: `{{ unit?, prefix?, suffix?, decimals?, thresholds?: [{{value,color,label?}}], color_mode?: 'value'|'background'|'none', graph_mode?: 'sparkline'|'none' }}`

- `gauge` (size w 4, h 4)
  runtime: single number.
  config: `{{ min, max, unit?, thresholds?, show_value?: bool }}`. Threshold colors `#10b981` green, `#f59e0b` amber, `#ef4444` red.

- `chart` (size w 6-12, h 5-7)
  runtime: `{{ rows: [{{...}}] }}`.
  config: `{{ kind: 'line'|'bar'|'area'|'pie'|'scatter', x_axis: '<column>', y_axis: ['<col>', ...], colors?, stacked?, show_legend? }}`. Always set `x_axis`.

- `table` (size w 12, h 6-10)
  runtime: `{{ rows: [{{...}}] }}`.
  config: `{{ columns: [{{key, header, width?, format?, thresholds?, status_colors?, link_template?}}], page_size?, sortable?, filterable? }}`.
  Column formats: `text`, `number`, `date`, `currency`, `percent`, `status` (colored pill — pair with `status_colors`), `progress` (0..100 bar — pair with `thresholds`), `badge`, `link` (use `link_template` like `"https://repo/{{id}}"`), `sparkline` (cell value is an array of numbers).

- `text` (size w 6-12, h 2-6)
  runtime: string. config: `{{ format: 'markdown'|'plain'|'html', font_size?, color?, align? }}`. Prefer markdown.

- `image` (size w 4-8, h 4-8)
  runtime: string URL or `{{src, alt?}}`. config: `{{ fit?: 'cover'|'contain'|'fill', border_radius? }}`.

- `logs` (size w 12, h 6-8)
  runtime: `{{ entries: [{{ts?, level?, message, source?}}] }}` or just an array of entries; entries may also be plain strings.
  config: `{{ max_entries?, show_timestamp?, show_level?, wrap?, reverse? }}`. Newest-first by default.

- `bar_gauge` (size w 5-8, h 4-7)
  runtime: `{{ rows: [{{name, value, max?}}] }}` — top-N list of values.
  config: `{{ orientation?: 'horizontal'|'vertical', display_mode?: 'gradient'|'basic'|'retro', min?, max?, unit?, thresholds?, show_value? }}`.

- `status_grid` (size w 4-8, h 3-6)
  runtime: `{{ items: [{{name, status, detail?}}] }}`. Statuses ok/warn/error/down/unknown auto-color.
  config: `{{ columns?: int (default 4), layout?: 'grid'|'row'|'compact', show_label?, status_colors?: {{<status>: '#hex'}} }}`.

- `heatmap` (size w 8-12, h 5-10)
  runtime: either `{{ cells: [{{x, y, value}}] }}` or a 2D array `[[v00, v01, ...], [v10, ...]]`.
  config: `{{ color_scheme?: 'viridis'|'magma'|'cool'|'warm'|'green_red', x_label?, y_label?, unit?, show_legend?, log_scale? }}`.

Shared datasources (share data across widgets)
When 2+ widgets pull from the same MCP/HTTP call, declare a SHARED datasource at the proposal level and have each widget reference it. The shared workflow runs once per refresh and fans out to every consumer - one MCP call, multiple widgets, consistent data.

How to use:
1. In the proposal root, add `shared_datasources: [{{key, kind, tool_name, server_id?, arguments?, pipeline?, refresh_cron?, label?}}]`. The `pipeline` is the BASE pipeline applied once to the raw source output before fan-out.
2. In each consumer widget, set `datasource_plan` to `{{kind: "shared", source_key: "<the key>", pipeline: [<per-widget tail>], output_path?: "<optional pre-tail pick>"}}`. The widget's own pipeline runs AFTER the shared pipeline, scoped to its tail.
3. `refresh_cron` lives on the shared entry, not on consumers. One cron tick = all consumers refresh.

Example: 5 stat widgets all about "active releases":
```
shared_datasources: [{{
  key: "active_releases",
  kind: "mcp_tool",
  server_id: "yandex",
  tool_name: "get_recent_active_releases",
  pipeline: [{{kind: "pick", path: "data.releases"}}],
  refresh_cron: "0 */5 * * * *"
}}]
widgets: [
  {{title: "Active count", widget_type: "stat", datasource_plan: {{kind: "shared", source_key: "active_releases", pipeline: [{{kind: "length"}}]}}}},
  {{title: "Latest version", widget_type: "stat", datasource_plan: {{kind: "shared", source_key: "active_releases", pipeline: [{{kind: "sort", by: "created_at", order: "desc"}}, {{kind: "head"}}, {{kind: "pick", path: "version"}}]}}}},
  {{title: "Releases table", widget_type: "table", datasource_plan: {{kind: "shared", source_key: "active_releases", pipeline: [{{kind: "limit", count: 20}}]}}}}
]
```

When to use shared vs standalone:
- Use SHARED when 2+ widgets read from the SAME tool with the SAME arguments. Even if their per-widget pipelines diverge, the source is identical.
- Use STANDALONE when each widget's datasource is genuinely independent (different MCP tools, different HTTP endpoints, or same tool with very different arguments).
- Don't pre-aggregate in the shared pipeline - keep it close to the raw API shape so each consumer can navigate freely. Apply expensive base trims (e.g. `pick "data.items"`) in shared, leave specific filters/sorts/limits to consumers.

Self-testing widgets with `dry_run_widget`
- Before emitting the final proposal JSON, CALL `dry_run_widget` once per non-trivial widget. The tool builds the workflow, runs the datasource_plan + pipeline once with no persistence, and returns the actual widget runtime data or an error.
- ALWAYS dry_run for: stat/gauge (must end at a number), bar_gauge/status_grid (must have the right object shape), tables with explicit columns (verify keys match), and any widget whose pipeline uses `aggregate`, `map`, or `llm_postprocess`.
- You may skip dry_run for the simplest case: a `text` widget whose datasource is `provider_prompt` with literal markdown content, since there's no shape to verify.
- If dry_run returns `status: "error"`, read the error message, FIX the pipeline (most often: wrong path, wrong field name, wrong aggregate metric), and dry_run AGAIN. Don't just remove the failing widget.
- If dry_run returns `status: "ok"` but `widget_runtime` looks wrong (e.g. stat value is `null` or an object instead of a number), fix the pipeline and re-run. The dashboard the user sees will use exactly this runtime data, so make it correct now.
- Each dry_run is cheap (one MCP/HTTP call). Spending 2-3 dry_runs to land the right pipeline is much better than handing the user a broken widget.

Pipeline (deterministic data transforms)
The `datasource_plan.pipeline` is an ORDERED list of typed steps applied to the raw datasource output before it reaches the widget. PREFER these deterministic steps over LLM postprocessing whenever possible: they're cheap, reproducible, and don't burn provider tokens on every refresh.

Path conventions (used by `pick`, `filter.field`, `sort.by`, `aggregate.group_by`, `aggregate.metric.field`, `map.fields`)
- Dotted nested fields: `commit.author.name`, `data.releases.0.version`.
- Numeric segments / `[index]`: pick a specific array element.
- `[*]`: iterate every item of an array, applying the rest of the path to each.
- Two `[*]` chained (e.g. `[*].issues[*]`) auto-FLATTEN one level - you get a single flat list of issues, not an array of arrays.
- All other ops accept the same nested-path syntax for their field arguments.
- The MCP node already unwraps the `{{content:[{{text:"<json>"}}]}}` envelope before the pipeline runs, so start your `pick` from the parsed JSON root, NOT from `content.0.text`.

Available steps (each is a strict JSON object - no scripting):
- {{"kind": "pick", "path": "data.items"}} - navigate to a sub-value. Path supports dots, `[index]`, and `[*]` to iterate. Two chained `[*]` flatten one level.
- {{"kind": "filter", "field": "status", "op": "eq", "value": "active"}} - keep array items where field op value is truthy. Ops: eq, ne, gt, gte, lt, lte, contains, starts_with, ends_with, in, not_in, exists, not_exists, truthy, falsy. `field` may be a nested path.
- {{"kind": "sort", "by": "updated_at", "order": "desc"}} - sort array. order: asc | desc.
- {{"kind": "limit", "count": 10}} - take first N.
- {{"kind": "head"}} / {{"kind": "tail"}} - shortcut: first / last element of an array as a scalar.
- {{"kind": "length"}} - replace array with its length (integer). Use this for "how many X" stat widgets.
- {{"kind": "flatten"}} - flatten one level of array-of-arrays.
- {{"kind": "unique"}} or {{"kind": "unique", "by": "author"}} - deduplicate array items, optionally by a field.
- {{"kind": "map", "fields": ["id", "name", "status"], "rename": {{"updated_at": "ts"}}}} - keep only listed fields per item, optionally renaming. If `fields` is empty, all keys pass through but rename still applies.
- {{"kind": "aggregate", "group_by": "team", "metric": {{"kind": "count"}}, "output_key": "count"}} - reduce an array. Metrics: count, sum/avg/min/max/first/last (with `field`). Without `group_by`, returns a single object.
- {{"kind": "set", "field": "label", "value": "Active releases"}} - inject a literal value at top level (useful for stat label).
- {{"kind": "format", "template": "v{{version}} (build {{launch}})"}} - render a string template using `{{field}}` placeholders from the current object. On arrays applies per item.
- {{"kind": "coerce", "to": "number"}} / "integer" / "string" / "array" - force the type. Useful right before a stat/gauge widget.
- {{"kind": "llm_postprocess", "prompt": "...", "expect": "text" | "json"}} - LAST RESORT. Calls the active provider on every refresh; spends tokens. Use only when shape cannot be derived deterministically (free-form summarization, content rewriting). Output replaces the pipeline result.

Common pipeline recipes (copy these patterns)
- "Number of active releases" (stat):
  `pipeline: [{{kind:"pick",path:"data.releases"}}, {{kind:"filter",field:"status",op:"eq",value:"active"}}, {{kind:"length"}}, {{kind:"set",field:"label",value:"Active releases"}}]`
  Final shape: `{{ value: <count>, label: "Active releases" }}`. (`length` outputs a number directly; the `set` step wraps it.)
  Actually for stat with both value+label, pipe length → wrap: `[{{kind:"pick",...}}, {{kind:"filter",...}}, {{kind:"length"}}, {{kind:"set",field:"value",value:null}}, {{kind:"set",field:"label",value:"Active releases"}}]` then the widget runtime extracts via `output_path` if needed. Simpler: emit the bare number and put the label in widget config instead.
- "Latest version string" (stat):
  `pipeline: [{{kind:"pick",path:"data.releases"}}, {{kind:"sort",by:"created_at",order:"desc"}}, {{kind:"head"}}, {{kind:"pick",path:"version"}}]`
  Final shape: a string like "2026.05.44.0".
- "Top 5 contributors" (bar_gauge):
  `pipeline: [{{kind:"pick",path:"data.commits"}}, {{kind:"aggregate",group_by:"author",metric:{{kind:"count"}},output_key:"value"}}, {{kind:"sort",by:"value",order:"desc"}}, {{kind:"limit",count:5}}, {{kind:"map",rename:{{"author":"name"}}}}]`
  Final shape: `[{{name, value}}, ...]`.
- "Service health overview" (status_grid):
  `pipeline: [{{kind:"pick",path:"services"}}, {{kind:"map",fields:["name","status"]}}]`
  Final shape: `[{{name, status}}, ...]`.
- "Markdown summary of latest release" (text widget):
  `pipeline: [{{kind:"pick",path:"data.releases"}}, {{kind:"sort",by:"date",order:"desc"}}, {{kind:"head"}}, {{kind:"format",template:"**{{version}}** — {{summary}}\n\nLaunched {{date}}"}}]`
  Final shape: a markdown string. The text widget will render it.

Pipeline rules
- Build the pipeline to land on the EXACT runtime shape the widget expects (single number for stat/gauge, rows array for chart/table/bar_gauge, items for status_grid/logs, cells for heatmap, text string for text widget).
- `output_path` is just shorthand for a single `pick` step. If you have multiple steps, you can put the navigation as the first `pick` and drop `output_path`.
- For stat widgets where the source returns an array, common pipeline: pick -> filter -> aggregate count -> set label.
- For status_grid: pick array -> map to {{name, status}} -> done.
- For bar_gauge top-N: pick -> sort desc -> limit N -> map to {{name, value}}.
- Don't put `llm_postprocess` inside cron-refreshed widgets unless the user explicitly accepts the cost. Default to fully deterministic.

Layout grid
- The grid is **12 columns** wide. Y grows downward.
- **DO NOT set `x` or `y` on widgets.** The apply pipeline ALWAYS auto-packs new widgets row-first (left-to-right, top-to-bottom) on the 12-col grid using just `w` and `h`. Any `x`/`y` you supply is ignored. This is intentional: model-supplied positions consistently leave gaps.
- The widgets appear on the dashboard in the order you list them in `proposal.widgets`. Put related KPIs adjacent in the array and the auto-packer will keep them in the same row.
- Pick `w` so that several widgets together fill the row to 12:
    * 4 small stats per row -> w: 3 each (3+3+3+3 = 12).
    * 3 medium stats -> w: 4 each (4+4+4 = 12).
    * 2 stats + half chart -> w: 3,3,6.
    * 1 chart full width -> w: 12.
    * 1 chart + 1 bar_gauge -> w: 7,5 or 8,4.
    * Wide table -> w: 12.
- Pick `h` so the widget has room to render:
    * stat: 2.
    * gauge: 4.
    * chart: 5-7.
    * table: 6-10.
    * status_grid / bar_gauge: 4-5.
    * logs / heatmap: 6-8.
- Typical Grafana-style layout (each row sums to 12):
    * Row 1: 4 stats at w=3, h=2 each (KPI strip).
    * Row 2: chart w=8, status_grid w=4, h=5.
    * Row 3: full-width table w=12, h=8.
    * Optional: logs or heatmap full-width below.

Text widget content (READ THIS BEFORE WRITING A `text` WIDGET)
- `text` widget runtime data is a STRING that goes through the markdown renderer. NEVER put a JSON object/array as the widget data - it will render as raw `{{...}}` text and look like a bug.
- Write human-readable markdown: short headings, bullet lists, **bold**, `inline code`, tables when comparing items. Keep it concise (1-2 short sections per text widget).
- If your data source returns JSON but you want a `text` widget, USE THE PIPELINE: pick the relevant values, then add an `llm_postprocess` with `expect: "text"` and a prompt like "Write a 2-sentence summary using the field `name` and `version` from this JSON." The LLM will return a markdown string.
- If the data is already a string (markdown explanation, log message, summary text), `output_path` to that string is enough; no pipeline needed.
- Don't dump raw JSON into a text widget "for debugging" - use a `table` widget instead, or expose individual fields as `stat` widgets.

Rules
- Every widget MUST include a runnable `datasource_plan`. Literal `data` alone is just a preview - the runtime re-runs the plan to refresh.
- NO HARDCODED VALUES in the persisted state. Specific versions, names, ids, counts, timestamps - all of these must come from `datasource_plan` (tool call result + pipeline). The `data` field is a preview hint ONLY; the live widget reads from the plan output.
    * Wrong: a stat widget with `data: 13` and no plan, or with a plan whose `arguments` hardcode the answer.
    * Right: a stat widget with `datasource_plan` calling an MCP/HTTP tool and a pipeline ending at the number.
    * The ONLY exception is when the user explicitly asks for static / literal data ("write 'Hello' in a text widget", "show this specific number"). Then hardcoding is fine and should be acknowledged in `summary`.
- If a previous version of a widget hardcoded values and the user asks for a fix/replacement, the new widget MUST replace the hardcode with a real `datasource_plan` + `pipeline`. Don't substitute one hardcoded value with another - that is the same bug.
- For MCP-backed widgets, the datasource_plan's server_id and tool_name MUST match the values you actually called successfully.
- Use `output_path` (dotted path: `data.field.0.value`) to extract the exact slice the widget expects. Stat/Gauge need a number; Chart/Table need an array of objects.
- Multiple widgets can share the same datasource if it returns rich data; each widget gets its own `output_path`.
- Do NOT say "the dashboard was created/updated". The UI applies your proposal only after the user clicks Apply.
- Honesty: if a tool fails or the data is missing, say so in `summary` and produce the best partial proposal you can, or no proposal at all.

Current time: {now}."#
    )
}

fn context_chat_system_prompt() -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    format!(
        r#"You are the Datrina Context Chat assistant. Datrina is a local-first desktop tool for building and refreshing dashboards from real datasources.

Help the user with whatever they ask: data questions, debugging widget behavior, explaining the dashboard model, small scripts, etc. Be concise and accurate.

Tools available when calling them helps the answer:
- `http_request` - policy-gated HTTPS request to public endpoints.
- `mcp_tool` - any configured stdio MCP tool. Use the exact server_id and tool_name from the tool description.

Open-ended requests (skill: MCP exploration)
For broad questions about an MCP server, don't guess one tool: call 2-4 DIFFERENT tools from its tool list with minimal arguments to see what each returns, then base the answer on real observed data.

Anti-loop discipline
- After every tool result, briefly note what it returned and decide: answer now, or ONE more call.
- NEVER call the same tool_name with the same arguments twice.
- If 3 different tools came back empty/generic, stop calling tools - either ask the user one short clarifying question, or answer honestly that you couldn't get useful data.

Talking with the user
- You may ask one short clarifying question when the request is genuinely ambiguous and no amount of exploration would resolve it.
- The user may correct or redirect you at any time. Treat each new user message as fresh intent and re-plan.

Rules
- Don't fabricate data. If a tool fails or you can't answer, say so directly.
- Stop calling tools as soon as you have enough.
- Plain prose answer - no JSON proposal in this mode.
- Use Markdown freely (headers, lists, **bold**, `code`, code blocks, tables) - it's rendered.

Current time: {now}."#
    )
}

fn prompt_mcp_system_message(server: &MCPServer) -> String {
    format!(
        r#"The user provided a stdio MCP server '{name}' (server_id='{id}'). It is already enabled and reachable through `mcp_tool`.

If the user gave a specific tool/task, go straight to it. If the user was open-ended ("build something useful"), apply the MCP exploration skill:
1. From the `mcp_tool` description, find tools whose server_id is '{id}'.
2. Call 2-4 of them through `mcp_tool` (server_id='{id}') with minimal/empty arguments. Run independent inspect calls in parallel.
3. Pick the widget(s) from the observed result shape. Each widget's datasource_plan MUST set kind='mcp_tool', server_id='{id}', the same tool_name, and the exact arguments that produced useful data.
4. Stop after at most a few inspect calls, then emit the proposal JSON.

If the result of every tool looks empty or you genuinely need a piece of input from the user (e.g. a specific id from thousands), you may ask ONE short clarifying question instead of guessing."#,
        name = server.name,
        id = server.id,
    )
}

#[allow(clippy::too_many_arguments)]
async fn chat_tool_specs(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    require_mcp: bool,
    synthetic_stream: bool,
) -> anyhow::Result<Vec<AIToolSpec>> {
    let mut specs = vec![AIToolSpec {
        name: "http_request".to_string(),
        description: "Make a policy-gated HTTP request through Datrina's Rust ToolEngine. Localhost, private networks, and blocked schemes are denied.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
                "url": { "type": "string" },
                "body": {},
                "headers": { "type": "object" }
            },
            "required": ["method", "url"],
            "additionalProperties": false
        }),
    }];

    let mcp_tools = match tokio::time::timeout(
        tokio::time::Duration::from_secs(20),
        reconnect_enabled_mcp_servers(
            app,
            state,
            session_id,
            message_id,
            sequence,
            provider,
            synthetic_stream,
        ),
    )
    .await
    {
        Ok(Ok(tools)) => tools,
        Ok(Err(error)) if require_mcp => return Err(error),
        Ok(Err(_)) => Vec::new(),
        Err(_) if require_mcp => {
            return Err(anyhow::anyhow!(
                "timed out while connecting enabled MCP servers"
            ));
        }
        Err(_) => Vec::new(),
    };
    if !mcp_tools.is_empty() {
        let available = describe_mcp_tools(&mcp_tools);
        specs.push(AIToolSpec {
            name: "mcp_tool".to_string(),
            description: format!(
                "Call a connected or reconnectable stdio MCP tool through Datrina's Rust policy gateway. Use exact server_id and tool_name values. Available tools:\n{}",
                available
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "server_id": { "type": "string" },
                    "tool_name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["server_id", "tool_name"],
                "additionalProperties": false
            }),
        });
    }

    // W17: persistent memory recall. The agent can pull its own prior
    // facts mid-turn (tool shapes, user preferences, lessons) without
    // waiting for them to be auto-injected at session start.
    specs.push(AIToolSpec {
        name: "recall".to_string(),
        description: "Query Datrina's persistent agent memory for prior facts, preferences, lessons, or MCP tool shapes learned across previous sessions. Returns the top relevance-ranked hits. Use this when you suspect you've solved a similar question before, or before exploring an MCP tool to check if its result shape is already known.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Free-text query describing what you want to recall." },
                "top_n": { "type": "integer", "minimum": 1, "maximum": 20 }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    });

    // Dry-run tool - lets the agent validate a single widget proposal by
    // building its workflow, executing it once with no persistence, and
    // returning the widget runtime data (or the error). Use BEFORE emitting
    // the final proposal JSON for any widget where shape correctness
    // matters: stat/gauge numbers, table/chart row shapes, pipeline
    // aggregates, anything with `llm_postprocess`.
    specs.push(AIToolSpec {
        name: "dry_run_widget".to_string(),
        description: "Test a single widget proposal end-to-end without persisting anything: builds the workflow, runs the datasource_plan + pipeline once, and returns the actual widget runtime data or the error. Use this BEFORE committing widgets to the final dashboard proposal so you can verify the pipeline produces the right shape (a number for stat/gauge, an array of objects for chart/table, etc.). Cheap to call. For widgets with datasource_plan.kind='shared', ALSO pass the matching `shared_datasources` entry so the dry run can inline the source + base pipeline.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "proposal": {
                    "type": "object",
                    "description": "A single BuildWidgetProposal exactly as you would include in proposal.widgets[].",
                    "additionalProperties": true
                },
                "shared_datasources": {
                    "type": "array",
                    "description": "Optional list of SharedDatasource entries the widget might reference via source_key. Required when proposal.datasource_plan.kind='shared'.",
                    "items": { "type": "object", "additionalProperties": true }
                }
            },
            "required": ["proposal"],
            "additionalProperties": false
        }),
    });

    Ok(specs)
}

async fn chat_tool_specs_silent(
    state: &AppState,
    require_mcp: bool,
) -> anyhow::Result<Vec<AIToolSpec>> {
    let mut specs = vec![AIToolSpec {
        name: "http_request".to_string(),
        description: "Make a policy-gated HTTP request through Datrina's Rust ToolEngine. Localhost, private networks, and blocked schemes are denied.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
                "url": { "type": "string" },
                "body": {},
                "headers": { "type": "object" }
            },
            "required": ["method", "url"],
            "additionalProperties": false
        }),
    }];

    let mcp_tools = match tokio::time::timeout(
        tokio::time::Duration::from_secs(20),
        reconnect_enabled_mcp_servers_silent(state),
    )
    .await
    {
        Ok(Ok(tools)) => tools,
        Ok(Err(error)) if require_mcp => return Err(error),
        Ok(Err(_)) => Vec::new(),
        Err(_) if require_mcp => {
            return Err(anyhow::anyhow!(
                "timed out while connecting enabled MCP servers"
            ));
        }
        Err(_) => Vec::new(),
    };
    if !mcp_tools.is_empty() {
        let available = describe_mcp_tools(&mcp_tools);
        specs.push(AIToolSpec {
            name: "mcp_tool".to_string(),
            description: format!(
                "Call a connected or reconnectable stdio MCP tool through Datrina's Rust policy gateway. Use exact server_id and tool_name values. Available tools:\n{}",
                available
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "server_id": { "type": "string" },
                    "tool_name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["server_id", "tool_name"],
                "additionalProperties": false
            }),
        });
    }
    Ok(specs)
}

async fn reconnect_enabled_mcp_servers_silent(
    state: &AppState,
) -> anyhow::Result<Vec<crate::models::mcp::MCPTool>> {
    let servers = state.storage.list_mcp_servers().await?;
    let mut all_tools = Vec::new();
    for server in servers.into_iter().filter(|server| server.is_enabled) {
        if !state.mcp_manager.is_connected(&server.id).await {
            state.tool_engine.validate_mcp_server(&server)?;
            let tools = state.mcp_manager.connect(server).await?;
            all_tools.extend(tools);
        }
    }
    all_tools.extend(state.mcp_manager.list_tools().await);
    Ok(all_tools)
}

fn describe_mcp_tools(tools: &[crate::models::mcp::MCPTool]) -> String {
    tools
        .iter()
        .take(12)
        .map(|tool| {
            format!(
                "- server_id: {}; tool_name: {}; description: {}; input_schema: {}",
                tool.server_id,
                tool.name,
                tool.description,
                preview_json(&tool.input_schema)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn execute_dry_run_widget(
    state: &AppState,
    proposal_value: serde_json::Value,
    shared_value: Option<serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    let proposal: crate::models::dashboard::BuildWidgetProposal =
        serde_json::from_value(proposal_value)
            .map_err(|e| anyhow::anyhow!("dry_run_widget: invalid proposal shape: {}", e))?;
    let shared_datasources: Vec<crate::models::dashboard::SharedDatasource> = match shared_value {
        Some(value) => serde_json::from_value(value).map_err(|e| {
            anyhow::anyhow!("dry_run_widget: invalid shared_datasources shape: {}", e)
        })?,
        None => Vec::new(),
    };
    let proposal =
        crate::commands::dashboard::inline_shared_into_widget(proposal, shared_datasources)?;
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

    let (widget, workflow) = crate::commands::dashboard::proposal_widget_public(&proposal, 0, now)?;
    use crate::commands::dashboard::WidgetDatasource;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow::anyhow!("Widget has no datasource workflow"))?;

    // Reconnect MCP servers for the duration of this single run.
    let mcp_servers = state.storage.list_mcp_servers().await?;
    for server in mcp_servers.into_iter().filter(|server| server.is_enabled) {
        if !state.mcp_manager.is_connected(&server.id).await {
            state.tool_engine.validate_mcp_server(&server)?;
            let _ = state.mcp_manager.connect(server).await;
        }
    }

    let provider = {
        let providers = state.storage.list_providers().await?;
        let active_id = state
            .storage
            .get_config("active_provider_id")
            .await?
            .filter(|id| !id.trim().is_empty());
        active_id
            .as_deref()
            .and_then(|id| providers.iter().find(|p| p.id == id && p.is_enabled))
            .or_else(|| providers.iter().find(|p| p.is_enabled))
            .cloned()
    };
    let engine = crate::modules::workflow_engine::WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        provider,
    );
    let started = std::time::Instant::now();
    let execution = engine.execute(&workflow, None).await?;
    let duration_ms = started.elapsed().as_millis() as u64;
    let run = execution.run;
    let node_results = run.node_results.clone();

    if !matches!(run.status, crate::models::workflow::RunStatus::Success) {
        return Ok(serde_json::json!({
            "status": "error",
            "error": run.error.clone().unwrap_or_else(|| "workflow failed".into()),
            "raw_output": node_results,
            "duration_ms": duration_ms,
            "pipeline_steps": pipeline_steps,
            "has_llm_step": has_llm_step,
            "workflow_nodes": workflow.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        }));
    }

    let node_results =
        node_results.ok_or_else(|| anyhow::anyhow!("Workflow returned no node results"))?;
    let output =
        crate::commands::dashboard::extract_output_public(&node_results, &datasource.output_key)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Workflow output '{}' not found in node_results",
                    datasource.output_key
                )
            })?
            .clone();
    let widget_runtime = crate::commands::dashboard::widget_runtime_data_public(&widget, &output)?;

    Ok(serde_json::json!({
        "status": "ok",
        "widget_runtime": widget_runtime,
        "raw_output": output,
        "duration_ms": duration_ms,
        "pipeline_steps": pipeline_steps,
        "has_llm_step": has_llm_step,
        "workflow_nodes": workflow.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
    }))
}

async fn execute_chat_tool(
    state: &AppState,
    session: &ChatSession,
    call: &crate::models::chat::ToolCall,
) -> ToolResult {
    let outcome = match call.name.as_str() {
        "http_request" => {
            let method = call
                .arguments
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("GET");
            let url = call
                .arguments
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let body = call.arguments.get("body").cloned();
            let headers = call.arguments.get("headers").cloned();
            state
                .tool_engine
                .http_request(method, url, body, headers)
                .await
        }
        "mcp_tool" => {
            let server_id = call
                .arguments
                .get("server_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let tool_name = call
                .arguments
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = call.arguments.get("arguments").cloned();
            let mcp_result =
                execute_mcp_tool(state, &server_id, &tool_name, arguments.clone()).await;
            // W17: record what this tool returned so the next session
            // doesn't have to rediscover the shape from scratch.
            if let Ok(ref value) = mcp_result {
                let args_for_fp = arguments.unwrap_or(serde_json::Value::Null);
                let memory_engine = state.memory_engine.clone();
                let server_id_clone = server_id.clone();
                let tool_name_clone = tool_name.clone();
                let value_clone = value.clone();
                tokio::spawn(async move {
                    if let Err(e) = memory_engine
                        .observe_tool_shape(
                            &server_id_clone,
                            &tool_name_clone,
                            &args_for_fp,
                            &value_clone,
                        )
                        .await
                    {
                        tracing::warn!("memory: tool-shape observation failed: {}", e);
                    }
                });
            }
            mcp_result
        }
        "dry_run_widget" => {
            let proposal_value = call.arguments.get("proposal").cloned().unwrap_or_else(|| {
                serde_json::Value::Object(call.arguments.as_object().cloned().unwrap_or_default())
            });
            let shared_value = call.arguments.get("shared_datasources").cloned();
            execute_dry_run_widget(state, proposal_value, shared_value).await
        }
        "recall" => execute_recall_tool(state, session, &call.arguments).await,
        _ => Err(anyhow::anyhow!(
            "Tool '{}' is not exposed to chat tool calling",
            call.name
        )),
    };

    match outcome {
        Ok(result) => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result,
            error: None,
        },
        Err(error) => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: serde_json::json!({ "status": "error" }),
            error: Some(error.to_string()),
        },
    }
}

async fn execute_recall_tool(
    state: &AppState,
    session: &ChatSession,
    arguments: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let query = arguments
        .get("query")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if query.trim().is_empty() {
        return Err(anyhow::anyhow!("recall: 'query' is required"));
    }
    let top_n = arguments
        .get("top_n")
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(5)
        .clamp(1, 20);

    let mut scopes: Vec<Scope> = Vec::new();
    if let Some(dashboard_id) = session.dashboard_id.as_ref() {
        scopes.push(Scope::Dashboard(dashboard_id.clone()));
    }
    if let Ok(servers) = state.storage.list_mcp_servers().await {
        for server in servers.into_iter().filter(|s| s.is_enabled) {
            scopes.push(Scope::McpServer(server.id));
        }
    }
    scopes.push(Scope::Session(session.id.clone()));
    scopes.push(Scope::Global);

    let hits = state.memory_engine.retrieve(&query, &scopes, top_n).await?;
    Ok(serde_json::json!({
        "hits": hits.iter().map(|hit| {
            serde_json::json!({
                "scope": hit.record.scope,
                "kind": hit.record.kind,
                "content": hit.record.content,
                "score": hit.score,
                "created_at": hit.record.created_at,
            })
        }).collect::<Vec<_>>()
    }))
}

async fn execute_mcp_tool(
    state: &AppState,
    server_id: &str,
    tool_name: &str,
    arguments: Option<serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    if !state.mcp_manager.is_connected(server_id).await {
        let server = state
            .storage
            .get_mcp_server(server_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("MCP server not found"))?;
        if !server.is_enabled {
            return Err(anyhow::anyhow!("MCP server is disabled"));
        }
        state.tool_engine.validate_mcp_server(&server)?;
        state.mcp_manager.connect(server).await?;
    }
    let tool_name = resolve_mcp_tool_name(state, server_id, tool_name).await;
    state
        .tool_engine
        .validate_mcp_tool_call(server_id, &tool_name)?;
    state
        .mcp_manager
        .call_tool(server_id, &tool_name, arguments)
        .await
}

async fn resolve_mcp_tool_name(state: &AppState, server_id: &str, requested: &str) -> String {
    let requested = requested.trim();
    let tools = state.mcp_manager.list_tools().await;
    if tools
        .iter()
        .any(|tool| tool.server_id == server_id && tool.name == requested)
    {
        return requested.to_string();
    }

    tools
        .iter()
        .find(|tool| {
            tool.server_id == server_id
                && tool
                    .name
                    .rsplit('.')
                    .next()
                    .is_some_and(|suffix| suffix == requested)
        })
        .map(|tool| tool.name.clone())
        .unwrap_or_else(|| requested.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn reconnect_enabled_mcp_servers(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    synthetic_stream: bool,
) -> anyhow::Result<Vec<crate::models::mcp::MCPTool>> {
    let servers = state.storage.list_mcp_servers().await?;
    let mut all_tools = Vec::new();
    for server in servers.into_iter().filter(|server| server.is_enabled) {
        if state.mcp_manager.is_connected(&server.id).await {
            continue;
        }
        let server_id = server.id.clone();
        emit_phase_event(
            app,
            session_id,
            message_id,
            sequence,
            provider,
            AgentPhase::McpListTools {
                server_id: server_id.clone(),
            },
            AgentPhaseStatus::Started,
            None,
            synthetic_stream,
        );
        if let Err(error) = state.tool_engine.validate_mcp_server(&server) {
            emit_phase_event(
                app,
                session_id,
                message_id,
                sequence,
                provider,
                AgentPhase::McpListTools {
                    server_id: server_id.clone(),
                },
                AgentPhaseStatus::Failed,
                Some(error.to_string()),
                synthetic_stream,
            );
            return Err(error);
        }
        match state.mcp_manager.connect(server).await {
            Ok(tools) => {
                let detail = format!("{} tool(s) discovered", tools.len());
                emit_phase_event(
                    app,
                    session_id,
                    message_id,
                    sequence,
                    provider,
                    AgentPhase::McpListTools {
                        server_id: server_id.clone(),
                    },
                    AgentPhaseStatus::Completed,
                    Some(detail),
                    synthetic_stream,
                );
                all_tools.extend(tools);
            }
            Err(error) => {
                emit_phase_event(
                    app,
                    session_id,
                    message_id,
                    sequence,
                    provider,
                    AgentPhase::McpListTools {
                        server_id: server_id.clone(),
                    },
                    AgentPhaseStatus::Failed,
                    Some(error.to_string()),
                    synthetic_stream,
                );
                return Err(error);
            }
        }
    }
    all_tools.extend(state.mcp_manager.list_tools().await);
    Ok(all_tools)
}

fn parse_build_proposal(content: &str) -> Option<BuildProposal> {
    let direct = serde_json::from_str::<BuildProposal>(content).ok();
    if direct.is_some() {
        return direct;
    }

    let value = extract_json_object(content)
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())?;
    serde_json::from_value(value).ok()
}

fn extract_json_object(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&content[start..=end])
}

/// Heuristically extracts a stdio MCP server description from the user's
/// chat prompt. Two shapes are supported:
///
/// 1. **Labelled.** The prompt contains one or more of the lowercase
///    labels `command:`, `args:`, `name:`, `env:` at the start of a
///    trimmed line. `command:` and `args:` are required.
/// 2. **Free-form.** Any whitespace-separated token that looks like an
///    absolute path AND has "mcp" in its basename is taken as the
///    command, paired with the first `args:` line found anywhere in the
///    prompt. The `name:` / `env:` labels are optional add-ons.
///
/// The resulting `MCPServer.id` is derived from the binary basename so
/// re-pasting the same prompt produces the same server entry (idempotent
/// save). The `prompt-` prefix keeps prompt-registered servers in a
/// separate namespace from user-managed ones.
fn extract_prompt_mcp_server(content: &str) -> Option<MCPServer> {
    let normalized = content.replace(['—', '–'], "--");
    let mut command: Option<String> = None;
    let mut args: Option<Vec<String>> = None;
    let mut name: Option<String> = None;
    let mut env: Option<std::collections::HashMap<String, String>> = None;

    for line in normalized.lines() {
        let trimmed = line.trim();
        if let Some(rest) = strip_prompt_label(trimmed, "command") {
            if command.is_none() && !rest.is_empty() {
                command = Some(clean_path_token(rest));
            }
        } else if let Some(rest) = strip_prompt_label(trimmed, "args") {
            if args.is_none() {
                let parsed = split_prompt_args(rest);
                if !parsed.is_empty() {
                    args = Some(parsed);
                }
            }
        } else if let Some(rest) = strip_prompt_label(trimmed, "name") {
            if name.is_none() && !rest.is_empty() {
                name = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_prompt_label(trimmed, "env") {
            if env.is_none() {
                if let Some(parsed) = parse_prompt_env_block(rest) {
                    env = Some(parsed);
                }
            }
        }
    }

    if command.is_none() {
        command = normalized.split_whitespace().find_map(|part| {
            let cleaned = clean_path_token(part);
            if looks_like_mcp_binary_path(&cleaned) {
                Some(cleaned)
            } else {
                None
            }
        });
    }

    // Free-form prompts may put `args:` mid-line ("run /foo with args: a b").
    // Fall back to a substring scan if the strict label loop above didn't
    // find one. Matches the original (pre-generalisation) behaviour.
    if args.is_none() {
        args = normalized.lines().find_map(|line| {
            let trimmed = line.trim();
            let (_, args_text) = trimmed.split_once("args:")?;
            let parsed = split_prompt_args(args_text.trim());
            if parsed.is_empty() {
                None
            } else {
                Some(parsed)
            }
        });
    }

    let command = command?;
    let args = args.unwrap_or_default();

    // We require at least an args block OR a path that clearly looks like
    // an MCP binary, so a stray "/etc/hosts" mention does not register
    // anything.
    if args.is_empty() && !looks_like_mcp_binary_path(&command) {
        return None;
    }

    let display_name = name.unwrap_or_else(|| derive_prompt_server_display_name(&command));
    let server_id = format!("prompt-{}", derive_prompt_server_id_suffix(&command));

    Some(MCPServer {
        id: server_id,
        name: format!("Prompt: {}", display_name),
        transport: MCPTransport::Stdio,
        is_enabled: true,
        command: Some(command),
        args: if args.is_empty() { None } else { Some(args) },
        env,
        url: None,
    })
}

fn strip_prompt_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let lower = line.to_lowercase();
    let prefix = format!("{label}:");
    if lower.starts_with(&prefix) {
        Some(line[prefix.len()..].trim())
    } else {
        None
    }
}

fn clean_path_token(token: &str) -> String {
    token
        .trim()
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | ':' | ',' | ';' | '"' | '\'' | '`'))
        .to_string()
}

fn looks_like_mcp_binary_path(token: &str) -> bool {
    if !(token.starts_with('/') || token.starts_with("~/")) {
        return false;
    }
    let basename = token.rsplit('/').next().unwrap_or(token).to_lowercase();
    basename.contains("mcp")
        || basename.ends_with("-server")
        || basename.ends_with("_server")
        || basename.ends_with("-proxy")
}

fn derive_prompt_server_display_name(command: &str) -> String {
    let basename = command.rsplit('/').next().unwrap_or(command);
    let cleaned = basename
        .replace(|c: char| c == '-' || c == '_', " ")
        .trim()
        .to_string();
    if cleaned.is_empty() {
        "stdio MCP".to_string()
    } else {
        cleaned
    }
}

fn derive_prompt_server_id_suffix(command: &str) -> String {
    let basename = command.rsplit('/').next().unwrap_or(command);
    let mut id = String::with_capacity(basename.len());
    let mut last_dash = false;
    for ch in basename.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !id.is_empty() {
            id.push('-');
            last_dash = true;
        }
    }
    let trimmed = id.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "mcp".to_string()
    } else {
        trimmed
    }
}

fn parse_prompt_env_block(raw: &str) -> Option<std::collections::HashMap<String, String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try JSON object first.
    if trimmed.starts_with('{') {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) {
            let mut out = std::collections::HashMap::new();
            for (key, value) in map {
                let value_str = match value {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                out.insert(key, value_str);
            }
            if out.is_empty() {
                return None;
            }
            return Some(out);
        }
    }
    // Fallback: space-separated KEY=VALUE pairs, with quoted values respected.
    let mut out = std::collections::HashMap::new();
    for token in split_prompt_args(trimmed) {
        if let Some((key, value)) = token.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                out.insert(key.to_string(), value.to_string());
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn split_prompt_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in value.chars() {
        match (quote, ch) {
            (Some(active), c) if c == active => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[tauri::command]
pub async fn truncate_chat_messages(
    state: State<'_, AppState>,
    session_id: String,
    before_message_id: String,
) -> Result<ApiResult<ChatSession>, String> {
    let mut session = match state.storage.get_chat_session(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(ApiResult::err("Session not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    if let Some(index) = session
        .messages
        .iter()
        .position(|m| m.id == before_message_id)
    {
        session.messages.truncate(index);
        session.updated_at = chrono::Utc::now().timestamp_millis();
        if let Err(e) = state.storage.update_chat_session(&session).await {
            return Ok(ApiResult::err(e.to_string()));
        }
    }
    Ok(ApiResult::ok(session))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_labelled_command_and_args() {
        let prompt = r#"
            Use this stdio MCP server for the build:
            command: /Users/me/.bin/example-mcp-proxy
            args: --token abc --project foo
        "#;
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        assert_eq!(
            server.command.as_deref(),
            Some("/Users/me/.bin/example-mcp-proxy")
        );
        assert_eq!(
            server.args.as_deref(),
            Some(
                &[
                    "--token".to_string(),
                    "abc".to_string(),
                    "--project".to_string(),
                    "foo".to_string()
                ][..]
            )
        );
        assert!(server.id.starts_with("prompt-"));
        assert!(server.id.contains("example-mcp-proxy"));
        assert!(server.name.contains("example mcp proxy"));
    }

    #[test]
    fn extracts_free_form_path_with_mcp_basename() {
        let prompt = "please run /opt/tools/notion-mcp with args: list_pages workspace=demo";
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        assert_eq!(server.command.as_deref(), Some("/opt/tools/notion-mcp"));
        let args = server.args.as_ref().expect("args");
        assert_eq!(args[0], "list_pages");
        assert_eq!(args[1], "workspace=demo");
        assert_eq!(server.id, "prompt-notion-mcp");
    }

    #[test]
    fn extracts_path_with_server_suffix() {
        // Token without "mcp" still detected because basename ends with -server.
        let prompt = "use /usr/local/bin/weather-server\nargs: --tz utc";
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        assert_eq!(
            server.command.as_deref(),
            Some("/usr/local/bin/weather-server")
        );
        assert_eq!(server.id, "prompt-weather-server");
    }

    #[test]
    fn ignores_unrelated_paths() {
        let prompt = "please cat /etc/hosts and tell me what you see";
        assert!(extract_prompt_mcp_server(prompt).is_none());
    }

    #[test]
    fn idempotent_id_for_same_path() {
        let a =
            extract_prompt_mcp_server("command: /opt/example-mcp\nargs: --port 8080").expect("a");
        let b =
            extract_prompt_mcp_server("command: /opt/example-mcp\nargs: --port 9090").expect("b");
        assert_eq!(
            a.id, b.id,
            "same binary path must produce same prompt-server id"
        );
    }

    #[test]
    fn explicit_name_label_overrides_derived() {
        let prompt = r#"
            command: /opt/example-mcp
            args: --port 1
            name: My Custom MCP
        "#;
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        assert_eq!(server.name, "Prompt: My Custom MCP");
    }

    #[test]
    fn env_label_as_json_object() {
        let prompt = r#"
            command: /opt/example-mcp
            args: --port 1
            env: { "TOKEN": "abc", "REGION": "eu" }
        "#;
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        let env = server.env.expect("env");
        assert_eq!(env.get("TOKEN").map(String::as_str), Some("abc"));
        assert_eq!(env.get("REGION").map(String::as_str), Some("eu"));
    }

    #[test]
    fn env_label_as_kv_pairs() {
        let prompt = r#"
            command: /opt/example-mcp
            args: --port 1
            env: TOKEN=abc REGION=eu
        "#;
        let server = extract_prompt_mcp_server(prompt).expect("should detect");
        let env = server.env.expect("env");
        assert_eq!(env.get("TOKEN").map(String::as_str), Some("abc"));
        assert_eq!(env.get("REGION").map(String::as_str), Some("eu"));
    }

    #[test]
    fn requires_args_when_path_is_not_mcp_shaped() {
        // The path doesn't match the mcp heuristic AND there's no args
        // block — should not register.
        let prompt = "command: /opt/random-binary";
        assert!(extract_prompt_mcp_server(prompt).is_none());
    }

    #[test]
    fn canonical_json_string_sorts_keys() {
        let a = canonical_json_string(&serde_json::json!({"b": 2, "a": 1}));
        let b = canonical_json_string(&serde_json::json!({"a": 1, "b": 2}));
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn loop_detection_counts_match() {
        let mut deque: std::collections::VecDeque<(String, String)> =
            std::collections::VecDeque::new();
        let key = ("mcp_tool".to_string(), "{\"a\":1}".to_string());
        assert_eq!(count_recent_repeats(&deque, &key), 0);
        deque.push_back(key.clone());
        assert_eq!(count_recent_repeats(&deque, &key), 1);
        deque.push_back(key.clone());
        assert_eq!(count_recent_repeats(&deque, &key), 2);
        deque.push_back(("other".to_string(), "{}".to_string()));
        assert_eq!(count_recent_repeats(&deque, &key), 2);
    }
}
