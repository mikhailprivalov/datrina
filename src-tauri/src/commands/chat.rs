use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tracing::info;

use crate::models::chat::{
    ChatEventEnvelope, ChatEventKind, ChatMessage, ChatMode, ChatSession, CreateSessionRequest,
    MessageMetadata, MessageRole, SendMessageRequest, ToolCallTrace, ToolPolicyDecision,
    ToolResult, ToolResultTrace, ToolTraceStatus, CHAT_EVENT_CHANNEL,
};
use crate::models::dashboard::BuildProposal;
use crate::models::mcp::{MCPServer, MCPTransport};
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

    let mut provider_messages = match grounded_messages(&state, &session).await {
        Ok(messages) => messages,
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };
    if let Some(server) = prompt_mcp_server.as_ref() {
        provider_messages.push(system_message(format!(
            "The user supplied a stdio MCP server for this build request. It has been configured and enabled with server_id '{}'. Inspect and use its available tools through the mcp_tool function. For widgets backed by this MCP, return datasource_plan.kind='mcp_tool' and datasource_plan.server_id='{}'.",
            server.id, server.id
        )));
    }

    let tool_specs = match chat_tool_specs(&state, prompt_mcp_server.is_some()).await {
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

    let mut persisted_tool_calls = ai_response.tool_calls.clone();
    let mut persisted_tool_results = Vec::new();
    let mut final_content = ai_response.content.clone();
    let mut final_model = ai_response.model.clone();
    let mut final_provider_id = ai_response.provider_id.clone();
    let mut final_tokens = ai_response.tokens.clone();
    let mut final_latency_ms = ai_response.latency_ms;
    let mut final_reasoning = ai_response.reasoning.clone();

    if !ai_response.tool_calls.is_empty() {
        let assistant_tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: if ai_response.content.trim().is_empty() {
                "Tool call requested by provider.".to_string()
            } else {
                ai_response.content.clone()
            },
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
            persisted_tool_results.push(execute_chat_tool(&state, call).await);
        }

        let tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: serde_json::to_string(&persisted_tool_results).unwrap_or_default(),
            mode: session.mode.clone(),
            tool_calls: None,
            tool_results: Some(persisted_tool_results.clone()),
            metadata: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(tool_msg);

        let resumed_messages = match grounded_messages(&state, &session).await {
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

    let assistant_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::Assistant,
        content: build_proposal
            .as_ref()
            .and_then(|proposal| proposal.summary.clone())
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or(final_content),
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

    let result = send_message_stream_inner(
        &app,
        &state,
        &mut session,
        &provider,
        &req,
        &assistant_message_id,
        &abort_flag,
        &mut sequence,
        synthetic_stream,
    )
    .await;

    state.chat_abort_flags.remove(&session_id);

    match result {
        Ok(message) => Ok(ApiResult::ok(message)),
        Err(error) => {
            emit_chat_event(
                &app,
                ChatEventEnvelope {
                    kind: ChatEventKind::MessageFailed,
                    session_id: session_id.clone(),
                    message_id: assistant_message_id,
                    sequence: next_sequence(&mut sequence),
                    provider_id: Some(provider.id),
                    model: Some(provider.default_model),
                    content_delta: None,
                    reasoning_delta: None,
                    reasoning: None,
                    tool_call: None,
                    tool_result: None,
                    build_proposal: None,
                    final_message: None,
                    error: Some(error.to_string()),
                    synthetic: synthetic_stream,
                    emitted_at: chrono::Utc::now().timestamp_millis(),
                },
            );
            Ok(ApiResult::err(error.to_string()))
        }
    }
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
    state: &State<'_, AppState>,
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
        provider_messages.push(system_message(format!(
            "The user supplied a stdio MCP server for this build request. It has been configured and enabled with server_id '{}'. Inspect and use its available tools through the mcp_tool function. For widgets backed by this MCP, return datasource_plan.kind='mcp_tool' and datasource_plan.server_id='{}'.",
            server.id, server.id
        )));
    }

    let tool_specs = chat_tool_specs(state, prompt_mcp_server.is_some())
        .await
        .map_err(|e| anyhow::anyhow!("MCP tool discovery failed: {}", e))?;

    let emit_app = app.clone();
    let emit_session_id = session.id.clone();
    let emit_message_id = assistant_message_id.to_string();
    let provider_id = provider.id.clone();
    let model = provider.default_model.clone();
    let cancel_for_provider = abort_flag.clone();
    let mut content_seen = String::new();
    let mut reasoning_seen = String::new();

    let ai_response = state
        .ai_engine
        .complete_chat_with_tools_streaming(
            provider,
            &provider_messages,
            &tool_specs,
            |event| match event {
                AIStreamEvent::ContentDelta(delta) => {
                    content_seen.push_str(&delta);
                    emit_chat_event(
                        &emit_app,
                        ChatEventEnvelope {
                            kind: ChatEventKind::ContentDelta,
                            session_id: emit_session_id.clone(),
                            message_id: emit_message_id.clone(),
                            sequence: next_sequence(sequence),
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
                    reasoning_seen.push_str(&delta);
                    emit_chat_event(
                        &emit_app,
                        ChatEventEnvelope {
                            kind: ChatEventKind::ReasoningDelta,
                            session_id: emit_session_id.clone(),
                            message_id: emit_message_id.clone(),
                            sequence: next_sequence(sequence),
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
        .await
        .map_err(|e| anyhow::anyhow!("AI provider stream failed: {}", e))?;

    let mut persisted_tool_calls = ai_response.tool_calls.clone();
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

    if !ai_response.tool_calls.is_empty() {
        let assistant_tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: if ai_response.content.trim().is_empty() {
                "Tool call requested by provider.".to_string()
            } else {
                ai_response.content.clone()
            },
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
            let result = execute_chat_tool(state, call).await;
            emit_tool_result_event(
                app,
                session,
                assistant_message_id,
                sequence,
                provider,
                &result,
                synthetic_stream,
            );
            persisted_tool_results.push(result);
        }

        let tool_msg = ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: serde_json::to_string(&persisted_tool_results).unwrap_or_default(),
            mode: session.mode.clone(),
            tool_calls: None,
            tool_results: Some(persisted_tool_results.clone()),
            metadata: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        session.messages.push(tool_msg);

        let resumed_messages = grounded_messages(state, session).await?;
        let resume_cancel = abort_flag.clone();
        let resumed = state
            .ai_engine
            .complete_chat_with_tools_streaming(
                provider,
                &resumed_messages,
                &[],
                |event| match event {
                    AIStreamEvent::ContentDelta(delta) => {
                        emit_chat_event(
                            app,
                            ChatEventEnvelope {
                                kind: ChatEventKind::ContentDelta,
                                session_id: session.id.clone(),
                                message_id: assistant_message_id.to_string(),
                                sequence: next_sequence(sequence),
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
                final_content = response.content;
                final_model = response.model;
                final_provider_id = response.provider_id;
                final_tokens = response.tokens;
                final_latency_ms = response.latency_ms;
                final_reasoning = response
                    .reasoning
                    .or_else(|| non_empty(reasoning_seen.clone()));
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

    if let Some(proposal) = build_proposal.clone() {
        emit_chat_event(
            app,
            ChatEventEnvelope {
                kind: ChatEventKind::BuildProposalParsed,
                session_id: session.id.clone(),
                message_id: assistant_message_id.to_string(),
                sequence: next_sequence(sequence),
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

    let assistant_msg = ChatMessage {
        id: assistant_message_id.to_string(),
        role: MessageRole::Assistant,
        content: build_proposal
            .as_ref()
            .and_then(|proposal| proposal.summary.clone())
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or(final_content),
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

    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::MessageCompleted,
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
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

fn emit_chat_event(app: &AppHandle, event: ChatEventEnvelope) {
    if let Err(error) = app.emit(CHAT_EVENT_CHANNEL, event) {
        tracing::warn!("failed to emit chat event: {}", error);
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
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: match status {
                ToolTraceStatus::Requested => ChatEventKind::ToolCallRequested,
                _ => ChatEventKind::ToolExecutionStarted,
            },
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: Some(ToolCallTrace {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments_preview: preview_json(&call.arguments),
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
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::ToolResult,
            session_id: session.id.clone(),
            message_id: assistant_message_id.to_string(),
            sequence: next_sequence(sequence),
            provider_id: Some(provider.id.clone()),
            model: Some(provider.default_model.clone()),
            content_delta: None,
            reasoning_delta: None,
            reasoning: None,
            tool_call: None,
            tool_result: Some(ToolResultTrace {
                tool_call_id: result.tool_call_id.clone(),
                name: result.name.clone(),
                status: if result.error.is_some() {
                    ToolTraceStatus::Error
                } else {
                    ToolTraceStatus::Success
                },
                result_preview: Some(preview_json(&result.result)),
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
        messages.push(system_message(build_chat_system_prompt()));
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

fn build_chat_system_prompt() -> String {
    r#"You are in Datrina build chat. Return a provider-generated dashboard proposal as strict JSON, without markdown fences, when the user asks to create or change dashboard content. Never claim the dashboard was changed; the UI applies proposals only after explicit confirmation.
Required JSON shape:
{
  "id": "short-stable-id",
  "title": "Proposal title",
  "summary": "One sentence human summary",
  "dashboard_name": "Optional dashboard name",
  "dashboard_description": "Optional dashboard description",
  "widgets": [
    {
      "widget_type": "chart|table|text|gauge|image",
      "title": "Widget title",
      "data": "optional preview sample only; chart/table samples are arrays of objects, gauge samples are numbers, image samples include src",
      "datasource_plan": {
        "kind": "builtin_tool|mcp_tool|provider_prompt",
        "tool_name": "http_request or MCP tool name",
        "server_id": "required only for mcp_tool",
        "arguments": {},
        "prompt": "required only for provider_prompt",
        "output_path": "optional dotted path in the tool/provider result to use as widget data",
        "refresh_cron": "optional cron expression for scheduled refresh"
      },
      "config": {}
    }
  ]
}
Every widget must include datasource_plan. Use builtin_tool/http_request for reachable public HTTP data, mcp_tool for a configured stdio MCP server when available, or provider_prompt when the datasource should be produced by the active Rust-mediated provider. Do not return only literal static data as the datasource."#.to_string()
}

async fn chat_tool_specs(
    state: &State<'_, AppState>,
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
        reconnect_enabled_mcp_servers(state),
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
        let available = mcp_tools
            .iter()
            .map(|tool| format!("{}.{}", tool.server_id, tool.name))
            .collect::<Vec<_>>()
            .join(", ");
        specs.push(AIToolSpec {
            name: "mcp_tool".to_string(),
            description: format!(
                "Call a connected or reconnectable stdio MCP tool through Datrina's Rust policy gateway. Available tools: {}",
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

async fn execute_chat_tool(
    state: &State<'_, AppState>,
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
                .unwrap_or("");
            let tool_name = call
                .arguments
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let arguments = call.arguments.get("arguments").cloned();
            execute_mcp_tool(state, server_id, tool_name, arguments).await
        }
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

async fn execute_mcp_tool(
    state: &State<'_, AppState>,
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
    state
        .tool_engine
        .validate_mcp_tool_call(server_id, tool_name)?;
    state
        .mcp_manager
        .call_tool(server_id, tool_name, arguments)
        .await
}

async fn reconnect_enabled_mcp_servers(
    state: &State<'_, AppState>,
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

fn extract_prompt_mcp_server(content: &str) -> Option<MCPServer> {
    let normalized = content.replace(['—', '–'], "--");
    let command = normalized
        .split_whitespace()
        .find(|part| part.contains("yandex-mcp-store-proxy"))?
        .trim_matches(|ch: char| matches!(ch, ')' | '(' | ':' | ',' | ';'))
        .to_string();

    let args = normalized.lines().find_map(|line| {
        let trimmed = line.trim();
        let (_, args_text) = trimmed.split_once("args:")?;
        Some(split_prompt_args(args_text.trim()))
    })?;

    if args.is_empty() {
        return None;
    }

    Some(MCPServer {
        id: "prompt-yandex-mcp-store-proxy".to_string(),
        name: "Prompt Yandex MCP store proxy".to_string(),
        transport: MCPTransport::Stdio,
        is_enabled: true,
        command: Some(command),
        args: Some(args),
        env: None,
        url: None,
    })
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
pub async fn delete_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_chat_session(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}
