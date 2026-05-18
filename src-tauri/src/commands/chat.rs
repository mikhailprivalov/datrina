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
    PlanArtifact, PlanStep, PlanStepKind, PlanStepStatus, SendMessageRequest, TokenUsage,
    ToolCallTrace, ToolPolicyDecision, ToolResult, ToolResultTrace, ToolTraceStatus,
    CHAT_EVENT_CHANNEL,
};
use crate::models::dashboard::BuildProposal;
use crate::models::mcp::{MCPServer, MCPTransport};
use crate::models::memory::{MemoryHit, Scope};
use crate::models::pricing::{
    pricing_for, CostSource, ModelPricing, ModelPricingOverride, TurnCost, UsageReport,
};
use crate::models::validation::ValidationIssue;
use crate::models::ApiResult;
use crate::modules::ai::{AIStreamEvent, AIToolSpec};
use crate::{AppState, ReflectionPending};

/// W18: tag a streaming assistant turn as a reflection follow-up. When
/// set, the persisted assistant message gets a
/// `ChatMessagePart::ReflectionMeta` part so the UI can badge it as a
/// suggestion (one-click apply) instead of a fresh proposal.
#[derive(Clone, Debug, Default)]
pub struct ReflectionContext {
    pub widget_ids: Vec<String>,
}

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
        current_plan: None,
        plan_status: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_reasoning_tokens: 0,
        total_cost_usd: 0.0,
        cost_unknown_turns: 0,
        max_cost_usd: None,
        language_override: None,
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

    let user_mentions =
        resolve_widget_mentions(&req.widget_mentions, session.dashboard_id.as_deref());
    let user_source_mentions =
        dedupe_source_mentions(&req.source_mentions, session.dashboard_id.as_deref());
    let resolved_source_mentions = resolve_source_mentions(
        state.inner(),
        &user_source_mentions,
        session.dashboard_id.as_deref(),
    )
    .await;
    let user_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        parts: user_message_parts(&req.content, &user_mentions, &user_source_mentions),
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

    let provider = match crate::resolve_active_provider(state.storage.as_ref()).await {
        Ok(Ok(provider)) => provider,
        Ok(Err(setup_error)) => return Ok(ApiResult::err(setup_error.to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
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
    // W48: brief the agent on the user's @source mentions BEFORE the
    // budget check so the prompt cost is counted accurately.
    if matches!(session.mode, ChatMode::Build) {
        if let Some(prompt) = build_source_mentions_prompt(&resolved_source_mentions) {
            provider_messages.push(system_message(prompt));
        }
    }

    let pricing = pricing_for_provider(state.inner(), &provider).await;
    if let Err(error) = enforce_session_budget(&session, pricing, &provider_messages) {
        return Ok(ApiResult::err(error.to_string()));
    }

    let tool_specs = match chat_tool_specs_silent(
        state.inner(),
        prompt_mcp_server.is_some(),
        matches!(session.mode, ChatMode::Build),
    )
    .await
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
            metadata: {
                let (cost_usd, cost_source) =
                    metadata_cost_fields(ai_response.tokens.as_ref(), pricing);
                Some(MessageMetadata {
                    model: Some(ai_response.model.clone()),
                    provider: Some(ai_response.provider_id.clone()),
                    tokens: ai_response.tokens.clone(),
                    latency_ms: Some(ai_response.latency_ms),
                    build_proposal: None,
                    reasoning: ai_response.reasoning.clone(),
                    cost_usd,
                    cost_source,
                })
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        // Accumulate the tool-call-only assistant turn into session totals
        // so the running counter reflects every provider round trip, not
        // just the final reply.
        accumulate_session_usage(&mut session, ai_response.tokens.as_ref(), pricing);
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

    let mut build_proposal = if matches!(session.mode, ChatMode::Build) {
        parse_build_proposal(&final_content)
    } else {
        None
    };

    // W29: non-streaming path goes through the same validator gate as
    // the streaming send. A proposal with unresolved issues never makes
    // it onto the assistant message as a `BuildProposal` part — the
    // chat UI surfaces a typed diagnostic block instead, and the
    // backend `apply_build_proposal` command also refuses to apply.
    if matches!(session.mode, ChatMode::Build) {
        if let Some(proposal_ref) = build_proposal.as_ref() {
            let dashboard_for_validation = match session.dashboard_id.as_deref() {
                Some(dashboard_id) => state
                    .storage
                    .get_dashboard(dashboard_id)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let target_widget_ids = target_widget_ids_from(&user_mentions);
            let mentioned_sources = mentioned_sources_for_validation(&resolved_source_mentions);
            let mentioned_sources_slice = if mentioned_sources.is_empty() {
                None
            } else {
                Some(mentioned_sources.as_slice())
            };
            let issues = crate::commands::validation::validate_build_proposal_full(
                proposal_ref,
                dashboard_for_validation.as_ref(),
                &session.messages,
                if target_widget_ids.is_empty() {
                    None
                } else {
                    Some(target_widget_ids.as_slice())
                },
                mentioned_sources_slice,
            );
            if !issues.is_empty() {
                tracing::warn!(
                    "non-streaming build proposal suppressed: {} validation issue(s)",
                    issues.len()
                );
                build_proposal = None;
            }
        }
    }

    let assistant_content = build_proposal
        .as_ref()
        .and_then(|proposal| proposal.summary.clone())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(final_content);
    let turn_cost = accumulate_session_usage(&mut session, final_tokens.as_ref(), pricing);
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
            cost_usd: turn_cost.and_then(|t| t.amount_usd),
            cost_source: turn_cost.map(|t| t.source),
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

    let user_mentions =
        resolve_widget_mentions(&req.widget_mentions, session.dashboard_id.as_deref());
    let user_source_mentions =
        dedupe_source_mentions(&req.source_mentions, session.dashboard_id.as_deref());
    let user_msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        parts: user_message_parts(&req.content, &user_mentions, &user_source_mentions),
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

    let provider = match crate::resolve_active_provider(state.storage.as_ref()).await {
        Ok(Ok(provider)) => provider,
        Ok(Err(setup_error)) => return Ok(ApiResult::err(setup_error.to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
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
            cost_usd: None,
            cost_source: None,
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
            None,
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

#[allow(clippy::too_many_arguments)]
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
    reflection: Option<ReflectionContext>,
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

    // W38: derive the active target set from the request mentions so we
    // can scope both prompt construction and proposal validation to the
    // widgets the user named this turn. `Empty` = unscoped (legacy).
    let target_mentions =
        resolve_widget_mentions(&req.widget_mentions, session.dashboard_id.as_deref());
    let target_widget_ids = target_widget_ids_from(&target_mentions);
    // W48: same shape for source mentions — resolve once so the prompt
    // and the validator agree on which sources are real.
    let target_source_mentions =
        dedupe_source_mentions(&req.source_mentions, session.dashboard_id.as_deref());
    let resolved_source_mentions = resolve_source_mentions(
        state,
        &target_source_mentions,
        session.dashboard_id.as_deref(),
    )
    .await;
    let mentioned_sources = mentioned_sources_for_validation(&resolved_source_mentions);
    let mentioned_sources_slice: Option<&[crate::commands::validation::MentionedSource]> =
        if mentioned_sources.is_empty() {
            None
        } else {
            Some(mentioned_sources.as_slice())
        };

    let mut provider_messages = grounded_messages(state, session).await?;
    if let Some(server) = prompt_mcp_server.as_ref() {
        provider_messages.push(system_message(prompt_mcp_system_message(server)));
    }

    // W48: brief the agent on the user's @source mentions. Always
    // emitted in Build mode so the model gets the typed source identity
    // even when only one source was named.
    if matches!(session.mode, ChatMode::Build) {
        if let Some(prompt) = build_source_mentions_prompt(&resolved_source_mentions) {
            provider_messages.push(system_message(prompt));
        }
    }

    // W38: when the user mentioned specific widgets, give the agent a
    // typed bundle for each target plus the targeted-edit policy. The
    // validator enforces the rule; the prompt steers the model toward
    // success on the first try so we don't burn the single retry.
    if matches!(session.mode, ChatMode::Build) && !target_mentions.is_empty() {
        if let Some(prompt) = build_targeted_edit_prompt(state, session, &target_mentions).await {
            provider_messages.push(system_message(prompt));
        }
    }

    // W18: Build sessions get a planning preamble. If a plan already
    // exists, surface its current state so the agent annotates new tool
    // calls with `_plan_step`. Otherwise instruct it to call
    // `submit_plan` first.
    if matches!(session.mode, ChatMode::Build) {
        provider_messages.push(system_message(plan_system_message(
            session.current_plan.as_ref(),
            session.plan_status.as_ref(),
        )));
    }

    // W22: pre-flight budget gate. Fails fast before we open the network
    // stream so the user gets an honest "budget_exceeded" error instead
    // of a silently truncated reply.
    let pricing = pricing_for_provider(state, provider).await;
    enforce_session_budget(session, pricing, &provider_messages)?;

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
        matches!(session.mode, ChatMode::Build),
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
                strict_mode: crate::models::provider::StructuredOutputCapability::PlainText,
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
        // Honour the user's Stop click between tool rounds so we don't keep
        // looping after the abort flag was raised mid-tool-execution.
        if abort_flag.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("chat_stream_cancelled"));
        }
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
        let (turn_cost_usd, turn_cost_source) =
            metadata_cost_fields(response_with_tools.tokens.as_ref(), pricing);
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
                cost_usd: turn_cost_usd,
                cost_source: turn_cost_source,
            }),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        // W22: each tool-resume round trip burns tokens; fold them into
        // session totals so the footer reflects every provider call.
        accumulate_session_usage(session, response_with_tools.tokens.as_ref(), pricing);
        session.messages.push(assistant_tool_msg);

        // W18: Build sessions enforce a plan before any other tool fires.
        // If the agent's first batch contains no `submit_plan`, we
        // synthesize a `plan_required` result for every call, force a
        // resume, and skip executing the underlying tools.
        let plan_required_now = matches!(session.mode, ChatMode::Build)
            && session.current_plan.is_none()
            && !response_with_tools
                .tool_calls
                .iter()
                .any(|c| c.name == "submit_plan");
        if plan_required_now {
            emit_phase_event(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                AgentPhase::PlanEnforcement,
                AgentPhaseStatus::Started,
                Some("agent attempted a tool call before submitting a plan".to_string()),
                synthetic_stream,
            );
        }

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

            if plan_required_now {
                let synth = ToolResult {
                    tool_call_id: call.id.clone(),
                    name: call.name.clone(),
                    result: serde_json::json!({ "status": "plan_required" }),
                    error: Some(
                        "plan_required: call submit_plan first to outline your steps before any other tool"
                            .to_string(),
                    ),
                    compression: None,
                };
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
                emit_tool_result_event(
                    app,
                    session,
                    assistant_message_id,
                    sequence,
                    provider,
                    &synth,
                    synthetic_stream,
                );
                current_tool_results.push(synth.clone());
                persisted_tool_results.push(synth);
                continue;
            }

            // W18: submit_plan establishes the per-session plan. We
            // parse it inline (no external tool call), update the
            // session, and emit PlanUpdated so the UI renders the
            // checklist.
            if call.name == "submit_plan" {
                let outcome = parse_plan_artifact(&call.arguments);
                let result_tool = match outcome {
                    Ok(plan) => {
                        let initial_status = initial_plan_status(&plan);
                        session.current_plan = Some(plan.clone());
                        session.plan_status = Some(initial_status.clone());
                        emit_plan_updated(
                            app,
                            &session.id,
                            assistant_message_id,
                            sequence,
                            provider,
                            &plan,
                            &initial_status,
                            synthetic_stream,
                        );
                        emit_phase_event(
                            app,
                            &session.id,
                            assistant_message_id,
                            sequence,
                            provider,
                            AgentPhase::PlanEnforcement,
                            AgentPhaseStatus::Completed,
                            Some(format!("plan accepted ({} step(s))", plan.steps.len())),
                            synthetic_stream,
                        );
                        ToolResult {
                            tool_call_id: call.id.clone(),
                            name: call.name.clone(),
                            result: serde_json::json!({
                                "status": "ok",
                                "step_count": plan.steps.len(),
                            }),
                            error: None,
                            compression: None,
                        }
                    }
                    Err(error) => ToolResult {
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        result: serde_json::json!({ "status": "error" }),
                        error: Some(error.to_string()),
                        compression: None,
                    },
                };
                let final_status = if result_tool.error.is_some() {
                    ToolTraceStatus::Error
                } else {
                    ToolTraceStatus::Success
                };
                emit_tool_call_event(
                    app,
                    session,
                    assistant_message_id,
                    sequence,
                    provider,
                    call,
                    final_status,
                    synthetic_stream,
                );
                emit_tool_result_event(
                    app,
                    session,
                    assistant_message_id,
                    sequence,
                    provider,
                    &result_tool,
                    synthetic_stream,
                );
                current_tool_results.push(result_tool.clone());
                persisted_tool_results.push(result_tool);
                continue;
            }

            // W18: pop the optional `_plan_step` annotation before the
            // real tool runs so its schema stays clean. When a step is
            // tagged, advance status + re-emit PlanUpdated.
            let mut arguments_owned = call.arguments.clone();
            let stripped_step = pop_plan_step(&mut arguments_owned);
            let call_for_exec = crate::models::chat::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: arguments_owned,
            };
            if let (Some(step_id), Some(plan)) =
                (stripped_step.as_deref(), session.current_plan.clone())
            {
                let mut status_map = session
                    .plan_status
                    .clone()
                    .unwrap_or_else(|| initial_plan_status(&plan));
                if advance_plan_step(&mut status_map, step_id) {
                    session.plan_status = Some(status_map.clone());
                    emit_plan_updated(
                        app,
                        &session.id,
                        assistant_message_id,
                        sequence,
                        provider,
                        &plan,
                        &status_map,
                        synthetic_stream,
                    );
                }
            }

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
                    compression: None,
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
                let result = execute_chat_tool(state, session, &call_for_exec).await;
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

        if abort_flag.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("chat_stream_cancelled"));
        }
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

        // W22: post-flight budget gate. If the resume turn we just
        // recorded already pushed us over the cap, stop before opening
        // another resume turn instead of grinding more tokens.
        if let Some(max) = session.max_cost_usd.filter(|m| *m > 0.0) {
            if session.total_cost_usd >= max {
                tracing::warn!(
                    "chat stream stopped mid-resume: session {} hit budget cap ${:.4}",
                    session.id,
                    max
                );
                final_content = format!(
                    "Stopped: session budget ${:.4} reached after {tool_iteration} tool iteration(s). Raise the cap or start a new session to continue.",
                    max
                );
                break;
            }
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
        // W22: the terminal response (no tool calls) skipped the resume
        // loop's accumulator path; fold its usage into the session now.
        accumulate_session_usage(session, final_tokens.as_ref(), pricing);
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
    let mut validation_outcome: Option<(AgentPhaseStatus, Vec<ValidationIssue>, bool)> = None;

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

            let target_slice: Option<&[String]> = if target_widget_ids.is_empty() {
                None
            } else {
                Some(target_widget_ids.as_slice())
            };
            let initial_issues = crate::commands::validation::validate_build_proposal_full(
                initial_proposal,
                dashboard_for_validation.as_ref(),
                &session.messages,
                target_slice,
                mentioned_sources_slice,
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
                validation_outcome = Some((AgentPhaseStatus::Completed, Vec::new(), false));
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

                    tracing::info!(
                        provider = %retry_response.provider_id,
                        model = %retry_response.model,
                        strict_mode = ?retry_response.strict_mode,
                        "W33 validation retry: structured_output mode resolved"
                    );

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
                        Some(updated) => crate::commands::validation::validate_build_proposal_full(
                            updated,
                            dashboard_for_validation.as_ref(),
                            &session.messages,
                            target_slice,
                            mentioned_sources_slice,
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
                    final_status.clone(),
                    current_issues.clone(),
                    validation_retried,
                    synthetic_stream,
                );
                validation_outcome = Some((
                    final_status,
                    current_issues.clone(),
                    validation_retried,
                ));
                residual_validation_issues = current_issues;
            }
        }
    }

    // W29: gate `BuildProposalParsed` emission on the validator. If
    // issues survived the retry budget, the proposal is non-applyable
    // and must NOT travel as a `BuildProposal` part / message metadata
    // — the typed `ProposalValidation::Failed` envelope already carries
    // the structured issues and the UI renders that as a diagnostic.
    let proposal_blocked_by_validation = !residual_validation_issues.is_empty();
    if proposal_blocked_by_validation {
        if let Some(rejected) = build_proposal.as_ref() {
            tracing::warn!(
                "build proposal '{}' suppressed: validator left {} unresolved issue(s) after retry",
                rejected.title,
                residual_validation_issues.len()
            );
        }
        build_proposal = None;
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

    // W18: clean up the session plan now that the assistant turn is
    // about to end. Any step still `Running` flips to `Done` so the UI
    // stops spinning on it; a separate `PlanUpdated` event flushes the
    // final state to subscribers.
    if let Some(plan) = session.current_plan.clone() {
        let mut status_map = session
            .plan_status
            .clone()
            .unwrap_or_else(|| initial_plan_status(&plan));
        if finalize_plan_status(&mut status_map, false) {
            session.plan_status = Some(status_map.clone());
            emit_plan_updated(
                app,
                &session.id,
                assistant_message_id,
                sequence,
                provider,
                &plan,
                &status_map,
                synthetic_stream,
            );
        }
    }

    let assistant_content = build_proposal
        .as_ref()
        .and_then(|proposal| proposal.summary.clone())
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(final_content);
    let mut final_parts = assistant_parts(
        &assistant_content,
        final_reasoning.as_ref(),
        &persisted_tool_calls,
        &persisted_tool_results,
        build_proposal.as_ref(),
    );
    // W18: surface the plan + current status as a typed part on the
    // assistant message that owned this turn. UI renders it as a
    // checklist above the message body.
    if let (Some(plan), Some(status)) =
        (session.current_plan.as_ref(), session.plan_status.as_ref())
    {
        final_parts.insert(
            0,
            ChatMessagePart::Plan {
                plan: plan.clone(),
                status: status.clone(),
            },
        );
    }
    // W18: tag reflection turns so the UI can badge the resulting
    // message as a suggestion rather than a fresh proposal.
    if let Some(reflection) = reflection.as_ref() {
        final_parts.push(ChatMessagePart::ReflectionMeta {
            widget_ids: reflection.widget_ids.clone(),
        });
    }
    // Persist the validator outcome so reloading the session preserves
    // the diagnostic tile that explains *why* a proposal was (or wasn't)
    // applied. Without this the assistant turn ends up as raw JSON text
    // with no context after a page refresh.
    if let Some((status, issues, retried)) = validation_outcome {
        final_parts.push(ChatMessagePart::ProposalValidation {
            status,
            issues,
            retried,
            updated_at: chrono::Utc::now().timestamp_millis(),
        });
    }

    let assistant_msg = ChatMessage {
        id: assistant_message_id.to_string(),
        role: MessageRole::Assistant,
        content: assistant_content.clone(),
        parts: final_parts,
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
            tokens: final_tokens.clone(),
            latency_ms: Some(final_latency_ms),
            build_proposal,
            reasoning: final_reasoning,
            cost_usd: {
                let (cost_usd, _) = metadata_cost_fields(final_tokens.as_ref(), pricing);
                cost_usd
            },
            cost_source: {
                let (_, source) = metadata_cost_fields(final_tokens.as_ref(), pricing);
                source
            },
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

// ─── W18: plan handling ─────────────────────────────────────────────────────

fn submit_plan_tool_spec() -> AIToolSpec {
    AIToolSpec {
        name: "submit_plan".to_string(),
        description: "Submit your execution plan before running any other tool in this Build session. Required as the FIRST tool call. Each step has a stable id, a one-line title, a kind, optional depends_on, and a one-sentence rationale. After submit_plan, every subsequent tool call MUST include a top-level `_plan_step: \"<step_id>\"` argument so the UI can show progress.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "1-2 sentence elevator pitch of the whole plan." },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "Stable kebab-case id, e.g. 'explore_mcp'." },
                            "title": { "type": "string", "description": "User-facing one-line description." },
                            "kind": {
                                "type": "string",
                                "enum": ["explore", "fetch", "design", "test", "propose", "other"]
                            },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" }
                            },
                            "rationale": { "type": "string", "description": "One sentence on why this step." }
                        },
                        "required": ["id", "title", "kind"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["steps", "summary"],
            "additionalProperties": false
        }),
    }
}

/// Parse a `submit_plan` tool call's arguments into a [`PlanArtifact`].
fn parse_plan_artifact(args: &serde_json::Value) -> anyhow::Result<PlanArtifact> {
    #[derive(serde::Deserialize)]
    struct RawStep {
        id: String,
        title: String,
        kind: PlanStepKind,
        #[serde(default)]
        depends_on: Vec<String>,
        #[serde(default)]
        rationale: String,
    }
    #[derive(serde::Deserialize)]
    struct Raw {
        summary: String,
        steps: Vec<RawStep>,
    }
    let raw: Raw = serde_json::from_value(args.clone())
        .map_err(|e| anyhow::anyhow!("submit_plan: invalid arguments: {}", e))?;
    if raw.steps.is_empty() {
        return Err(anyhow::anyhow!(
            "submit_plan: at least one step is required"
        ));
    }
    Ok(PlanArtifact {
        summary: raw.summary,
        steps: raw
            .steps
            .into_iter()
            .map(|s| PlanStep {
                id: s.id,
                title: s.title,
                kind: s.kind,
                depends_on: s.depends_on,
                rationale: s.rationale,
            })
            .collect(),
        created_at: chrono::Utc::now().timestamp_millis(),
    })
}

fn initial_plan_status(plan: &PlanArtifact) -> std::collections::BTreeMap<String, PlanStepStatus> {
    plan.steps
        .iter()
        .map(|step| (step.id.clone(), PlanStepStatus::Pending))
        .collect()
}

/// Pop a `_plan_step` argument from the tool call's payload (if any).
/// Removing it before downstream execution keeps the underlying tools
/// schema-clean — they never see the plan annotation.
fn pop_plan_step(arguments: &mut serde_json::Value) -> Option<String> {
    let map = arguments.as_object_mut()?;
    let value = map.remove("_plan_step")?;
    value.as_str().map(|s| s.to_string())
}

/// Mark `step_id` as running and flush any prior `Running` entry to
/// `Done`. Returns `true` if the status map actually changed.
fn advance_plan_step(
    status: &mut std::collections::BTreeMap<String, PlanStepStatus>,
    step_id: &str,
) -> bool {
    let mut changed = false;
    for (id, value) in status.iter_mut() {
        if id == step_id {
            continue;
        }
        if matches!(value, PlanStepStatus::Running) {
            *value = PlanStepStatus::Done;
            changed = true;
        }
    }
    let entry = status
        .entry(step_id.to_string())
        .or_insert(PlanStepStatus::Pending);
    if !matches!(entry, PlanStepStatus::Running | PlanStepStatus::Done) {
        *entry = PlanStepStatus::Running;
        changed = true;
    } else if matches!(entry, PlanStepStatus::Pending) {
        *entry = PlanStepStatus::Running;
        changed = true;
    }
    changed
}

/// Finalize the plan when the assistant run ends. On `Done` finishes the
/// remaining `Running` step and leaves untouched `Pending` ones; on
/// failure flips the active step to `Failed` instead.
fn finalize_plan_status(
    status: &mut std::collections::BTreeMap<String, PlanStepStatus>,
    failed: bool,
) -> bool {
    let mut changed = false;
    for (_, value) in status.iter_mut() {
        if matches!(value, PlanStepStatus::Running) {
            *value = if failed {
                PlanStepStatus::Failed
            } else {
                PlanStepStatus::Done
            };
            changed = true;
        }
    }
    changed
}

#[allow(clippy::too_many_arguments)]
fn emit_plan_updated(
    app: &AppHandle,
    session_id: &str,
    message_id: &str,
    sequence: &mut u32,
    provider: &crate::models::provider::LLMProvider,
    plan: &PlanArtifact,
    status: &std::collections::BTreeMap<String, PlanStepStatus>,
    synthetic: bool,
) {
    emit_chat_event(
        app,
        ChatEventEnvelope {
            kind: ChatEventKind::PlanUpdated,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            sequence: next_sequence(sequence),
            agent_event: Some(AgentEvent::PlanUpdated {
                plan: plan.clone(),
                status: status.clone(),
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

fn plan_system_message(
    plan: Option<&PlanArtifact>,
    status: Option<&std::collections::BTreeMap<String, PlanStepStatus>>,
) -> String {
    match plan {
        None => "## Plan / Execute discipline (W18)\nThis Build session has NO plan yet. Your FIRST tool call MUST be `submit_plan(summary, steps)`. Each step needs id, title, kind, optional depends_on, rationale. After plan submission, every subsequent tool call MUST include a `_plan_step: \"<step_id>\"` argument so the UI tracks progress.".to_string(),
        Some(plan) => {
            let status_default = std::collections::BTreeMap::<String, PlanStepStatus>::new();
            let status = status.unwrap_or(&status_default);
            let mut body = String::from(
                "## Current plan (W18)\nA plan is already in flight for this Build session. Continue executing it; include `_plan_step: \"<step_id>\"` in EVERY subsequent tool call so the UI advances. If the user has redirected, you may finish steps as `done` and add new ones via a fresh `submit_plan` call only when the user clearly asked for a new direction.\n\nSummary: ",
            );
            body.push_str(&plan.summary);
            body.push_str("\n\nSteps:\n");
            for step in &plan.steps {
                let s = status.get(&step.id).copied().unwrap_or(PlanStepStatus::Pending);
                let marker = match s {
                    PlanStepStatus::Pending => "[ ]",
                    PlanStepStatus::Running => "[~]",
                    PlanStepStatus::Done => "[x]",
                    PlanStepStatus::Failed => "[!]",
                };
                body.push_str(&format!("- {} {} ({}): {}\n", marker, step.id, kind_label(step.kind), step.title));
            }
            body
        }
    }
}

/// W38: render a compact "targeted edit" system prompt for the mentioned
/// widgets. Pulls current widget config + datasource + tail pipeline +
/// freshness from storage so the agent has enough typed context to
/// produce a focused proposal without re-reading the dashboard or asking
/// the user. Returns `None` when nothing useful resolves (e.g. all
/// mentioned ids are stale).
async fn build_targeted_edit_prompt(
    state: &AppState,
    session: &ChatSession,
    mentions: &[crate::models::chat::WidgetMention],
) -> Option<String> {
    let dashboard_id = session.dashboard_id.as_deref()?;
    let dashboard = state
        .storage
        .get_dashboard(dashboard_id)
        .await
        .ok()
        .flatten()?;
    use crate::commands::dashboard::WidgetDatasource;
    let mut targets: Vec<serde_json::Value> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    for mention in mentions {
        match dashboard
            .layout
            .iter()
            .find(|w| w.id() == mention.widget_id)
        {
            Some(widget) => {
                let mut entry = serde_json::json!({
                    "widget_id": widget.id(),
                    "label": mention.label,
                    "title": widget.title(),
                    "kind": widget_type_label(widget),
                });
                if let Some(ds) = widget.datasource() {
                    entry["datasource"] = serde_json::json!({
                        "workflow_id": ds.workflow_id,
                        "output_key": ds.output_key,
                        "datasource_definition_id": ds.datasource_definition_id,
                        "binding_source": ds.binding_source,
                        "tail_pipeline_steps": ds.tail_pipeline.len(),
                        "capture_traces": ds.capture_traces,
                    });
                }
                targets.push(entry);
            }
            None => stale.push(mention.widget_id.clone()),
        }
    }
    if targets.is_empty() && stale.is_empty() {
        return None;
    }
    let targets_json = serde_json::to_string_pretty(&targets).unwrap_or_else(|_| "[]".to_string());
    let mut body = String::from(
        "## Targeted Build edit (W38)\nThe user mentioned specific widgets in their message. \
        Treat these as the EXCLUSIVE edit target set for this turn unless the user explicitly asks \
        for broader cleanup (\"also delete the old chart\", \"add a new widget too\").\n\n\
        Rules:\n\
        - To CHANGE / REPLACE a mentioned widget, emit it inside `widgets` with `replace_widget_id: \"<id>\"`.\n\
        - To REMOVE mentioned widgets, list their ids in `proposal.remove_widget_ids`.\n\
        - To EXPLAIN or DEBUG a mentioned widget without changing it, answer in prose; do NOT emit a proposal.\n\
        - Do NOT touch (replace, remove, or duplicate) widgets that are not in this target set. \
          The validator blocks any proposal that mutates unmentioned widgets.\n\
        - Adding net-new widgets is allowed only when the user explicitly asks for additions, \
          or when the requested change literally cannot be expressed as an edit to a mentioned widget.\n\n\
        Targets:\n",
    );
    body.push_str(&targets_json);
    if !stale.is_empty() {
        body.push_str("\n\nStale mention ids (no matching widget on the dashboard — ignore):\n");
        for id in stale {
            body.push_str(&format!("- {}\n", id));
        }
    }
    Some(body)
}

fn kind_label(kind: PlanStepKind) -> &'static str {
    match kind {
        PlanStepKind::Explore => "explore",
        PlanStepKind::Fetch => "fetch",
        PlanStepKind::Design => "design",
        PlanStepKind::Test => "test",
        PlanStepKind::Propose => "propose",
        PlanStepKind::Other => "other",
    }
}

// ─── W18: post-apply reflection turn ────────────────────────────────────────

/// Spawn an asynchronous reflection turn for one widget. Called from
/// `refresh_widget` when a freshly applied widget produces its first
/// successful runtime payload.
pub fn enqueue_reflection_turn(
    app: AppHandle,
    state: AppState,
    pending: ReflectionPending,
    data: serde_json::Value,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_reflection_turn(&app, &state, &pending, &data).await {
            tracing::warn!(
                "post-apply reflection failed for widget {}: {}",
                pending.widget_id,
                error
            );
        }
    });
}

async fn run_reflection_turn(
    app: &AppHandle,
    state: &AppState,
    pending: &ReflectionPending,
    data: &serde_json::Value,
) -> anyhow::Result<()> {
    let session = match state.storage.get_chat_session(&pending.session_id).await? {
        Some(s) => s,
        None => return Ok(()),
    };
    if !matches!(session.mode, ChatMode::Build) {
        // Reflection only makes sense in Build mode — the agent has
        // proposal-shaped tooling there.
        return Ok(());
    }

    let provider = match crate::resolve_active_provider(state.storage.as_ref()).await? {
        Ok(p) => p,
        Err(setup_error) => {
            tracing::info!("reflection skipped: {}", setup_error.message());
            return Ok(());
        }
    };

    let preview = preview_json(data);
    let preview_text = serde_json::to_string(&preview).unwrap_or_default();
    let action = if pending.replaced {
        "replaced"
    } else {
        "added"
    };
    // W32: include compact replay diagnostics from the most recent
    // captured trace, when capture is on and a trace exists. The
    // summary names the first empty/failed step so the agent can
    // anchor its critique on a concrete step index instead of
    // re-guessing the pipeline shape from `preview_text`.
    let trace_summary = match state.storage.list_widget_traces(&pending.widget_id).await {
        Ok(rows) => rows.first().and_then(|(_captured_at, trace_json)| {
            serde_json::from_str::<crate::models::pipeline::PipelineTrace>(trace_json)
                .ok()
                .map(|trace| crate::commands::debug::trace_summary_for_reflection(&trace))
        }),
        Err(_) => None,
    };
    let trace_block = trace_summary
        .map(|s| format!("\n- recent pipeline trace:\n{}", s))
        .unwrap_or_default();
    // W41: paste the typed provenance summary so the agent knows whether
    // the widget is deterministic-only, provider-backed, or post-processed
    // by an LLM step, plus the concrete tool/server/model that produced
    // the value. Built from the same path the inspector renders, so the
    // critique reads the same data the user is staring at.
    let provenance_block = match crate::commands::provenance::build_widget_provenance(
        state.storage.as_ref(),
        Some(&provider),
        None,
        &pending.dashboard_id,
        &pending.widget_id,
    )
    .await
    {
        Ok(prov) => {
            let summary = crate::commands::provenance::provenance_summary_for_reflection(&prov);
            if summary.is_empty() {
                String::new()
            } else {
                format!("\n- widget provenance:\n{}", summary)
            }
        }
        Err(error) => {
            tracing::warn!(
                "reflection provenance unavailable for widget {}: {}",
                pending.widget_id,
                error
            );
            String::new()
        }
    };
    let content = format!(
        "[reflection] Widget you just {action} just rendered live data after applying the proposal.\n- widget_id: {}\n- title: \"{}\"\n- kind: {}\n- runtime preview: {}{}{}\n\nCritique your own output. If the value looks broken (null / empty / zero / wrong shape / off-domain), emit a fix-up BuildProposal as a DELTA with `replace_widget_id: \"{}\"` and an updated pipeline. If the value looks correct for the user's intent, reply with one short line acknowledging it — DO NOT emit JSON.",
        pending.widget_id,
        pending.widget_title,
        pending.widget_kind,
        preview_text,
        provenance_block,
        trace_block,
        pending.widget_id
    );
    spawn_chat_streaming_turn(
        app.clone(),
        state.clone(),
        session,
        provider,
        SendMessageRequest {
            content,
            widget_mentions: Vec::new(),
            source_mentions: Vec::new(),
        },
        Some(ReflectionContext {
            widget_ids: vec![pending.widget_id.clone()],
        }),
    )
    .await
}

/// W21: spin up a new chat session and run an autonomous turn off the
/// back of a firing alert. Returns the session id so the caller can
/// persist it on the `alert_events` row for budget tracking + the
/// "View" deep link in the follow-up notification.
pub async fn spawn_autonomous_alert_turn(
    app: AppHandle,
    state: AppState,
    mode: ChatMode,
    dashboard_id: Option<String>,
    widget_id: Option<String>,
    title: String,
    prompt: String,
    max_cost_usd: Option<f64>,
) -> anyhow::Result<String> {
    let provider = crate::resolve_active_provider(state.storage.as_ref())
        .await?
        .map_err(|e| anyhow::anyhow!("no enabled LLM provider to run autonomous alert: {}", e))?;

    let now = chrono::Utc::now().timestamp_millis();
    let session = ChatSession {
        id: uuid::Uuid::new_v4().to_string(),
        mode,
        dashboard_id,
        widget_id,
        title,
        messages: vec![],
        current_plan: None,
        plan_status: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_reasoning_tokens: 0,
        total_cost_usd: 0.0,
        cost_unknown_turns: 0,
        max_cost_usd,
        language_override: None,
        created_at: now,
        updated_at: now,
    };
    state.storage.create_chat_session(&session).await?;
    let session_id = session.id.clone();

    spawn_chat_streaming_turn(
        app,
        state,
        session,
        provider,
        SendMessageRequest {
            content: prompt,
            widget_mentions: Vec::new(),
            source_mentions: Vec::new(),
        },
        None,
    )
    .await?;
    Ok(session_id)
}

/// Shared entry point that mirrors `send_message_stream` for callers
/// that already hold the session and provider (W18 reflection,
/// W21 alerts).
async fn spawn_chat_streaming_turn(
    app: AppHandle,
    state: AppState,
    mut session: ChatSession,
    provider: crate::models::provider::LLMProvider,
    req: SendMessageRequest,
    reflection: Option<ReflectionContext>,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
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
    session.updated_at = now;
    state.storage.update_chat_session(&session).await?;

    let assistant_message_id = uuid::Uuid::new_v4().to_string();
    let synthetic_stream = !matches!(
        provider.kind,
        crate::models::provider::ProviderKind::Openrouter
            | crate::models::provider::ProviderKind::Custom
    );
    let abort_flag = Arc::new(AtomicBool::new(false));
    let session_id = session.id.clone();
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

    let app_for_task = app.clone();
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();
    let provider_for_task = provider.clone();
    let assistant_message_id_for_task = assistant_message_id.clone();
    let reflection_for_task = reflection.clone();
    tauri::async_runtime::spawn(async move {
        let mut local_sequence = sequence;
        let outcome = AssertUnwindSafe(send_message_stream_inner(
            &app_for_task,
            &state_for_task,
            &mut session,
            &provider_for_task,
            &req,
            &assistant_message_id_for_task,
            &abort_flag,
            &mut local_sequence,
            synthetic_stream,
            reflection_for_task,
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
                    "reflection stream panicked: session={} message={} reason={}",
                    session_id_for_task,
                    assistant_message_id_for_task,
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
                    message_id: assistant_message_id_for_task,
                    sequence: next_sequence(&mut local_sequence),
                    agent_event: Some(agent_event),
                    provider_id: Some(provider_for_task.id),
                    model: Some(provider_for_task.default_model),
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
    Ok(())
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
                compression: result.compression.clone(),
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
                compression: result.compression.clone(),
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

// ─── W22: token + cost accounting ───────────────────────────────────────────

/// Convert a wire `TokenUsage` into the richer `UsageReport` used by the
/// pricing layer.
fn usage_report_from_tokens(tokens: &TokenUsage) -> UsageReport {
    UsageReport::new(tokens.prompt, tokens.completion, tokens.reasoning)
}

/// Resolve the pricing entry for this provider+model, consulting the
/// user-editable overrides file first. Errors are downgraded to `None`
/// (means "no pricing data, can't compute cost") so a broken overrides
/// file never blocks a chat turn.
pub async fn pricing_for_provider(
    state: &AppState,
    provider: &crate::models::provider::LLMProvider,
) -> Option<ModelPricing> {
    let overrides = load_pricing_overrides(state).await.unwrap_or_default();
    pricing_for(provider.kind, &provider.default_model, &overrides)
}

/// Load and parse `pricing_overrides.json`. Missing file or invalid
/// JSON returns an empty list — never an error — so the chat path keeps
/// flowing.
pub async fn load_pricing_overrides(state: &AppState) -> anyhow::Result<Vec<ModelPricingOverride>> {
    let path = state.storage.pricing_overrides_path();
    match tokio::fs::read_to_string(&path).await {
        Ok(raw) => {
            let parsed: crate::models::pricing::PricingOverridesFile =
                serde_json::from_str(&raw).unwrap_or_default();
            Ok(parsed.overrides)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => {
            tracing::warn!(
                "pricing_overrides.json read failed at {}: {}",
                path.display(),
                error
            );
            Ok(Vec::new())
        }
    }
}

/// Pre-flight estimate: how many extra USD will this provider request
/// add to the session, given the messages we're about to send? Uses
/// input pricing on a chars/4 token approximation when the provider
/// doesn't pre-report usage, plus a small completion buffer so the gate
/// trips before the request actually overruns.
fn estimate_request_cost(
    pricing: ModelPricing,
    provider_messages: &[ChatMessage],
    completion_token_buffer: u32,
) -> f64 {
    let approx_input_tokens: u32 = provider_messages
        .iter()
        .map(|message| (message.content.len() / 4).max(1) as u32)
        .sum();
    let estimate_usage = UsageReport::new(approx_input_tokens, completion_token_buffer, None);
    pricing.cost_for(&estimate_usage)
}

/// Returns `Err(budget_exceeded)` if running this request would push the
/// session past its `max_cost_usd` cap. `None` for either argument means
/// "skip the check" — no cap configured or no pricing data available.
pub fn enforce_session_budget(
    session: &ChatSession,
    pricing: Option<ModelPricing>,
    provider_messages: &[ChatMessage],
) -> anyhow::Result<()> {
    let Some(max) = session.max_cost_usd.filter(|m| *m > 0.0) else {
        return Ok(());
    };
    if session.total_cost_usd >= max {
        return Err(anyhow::anyhow!(
            "budget_exceeded: session limit ${:.4} already reached (spent ${:.4})",
            max,
            session.total_cost_usd
        ));
    }
    let Some(pricing) = pricing else {
        return Ok(());
    };
    // Reserve ~512 completion tokens so a single follow-up reply can't
    // silently overshoot the cap.
    let projected = session.total_cost_usd + estimate_request_cost(pricing, provider_messages, 512);
    if projected > max {
        return Err(anyhow::anyhow!(
            "budget_exceeded: next request would push session past ${:.4} (spent ${:.4}, projected +${:.4})",
            max,
            session.total_cost_usd,
            projected - session.total_cost_usd
        ));
    }
    Ok(())
}

/// W49: price a single turn's usage. Provider-reported total cost wins
/// over the local pricing table; pricing-table cost is the next
/// fallback; if neither is available the turn surfaces as
/// `UnknownPricing` so the UI can render `unknown cost` instead of
/// silently treating it as a free call.
fn price_turn(tokens: &TokenUsage, pricing: Option<ModelPricing>) -> TurnCost {
    if let Some(cost) = tokens
        .provider_cost_usd
        .filter(|c| c.is_finite() && *c >= 0.0)
    {
        return TurnCost::provider_total(cost);
    }
    if let Some(pricing) = pricing {
        return TurnCost::pricing_table(pricing.cost_for(&usage_report_from_tokens(tokens)));
    }
    TurnCost::unknown()
}

/// Apply a single assistant turn's usage to the session's running
/// totals. Returns the computed `TurnCost` for the turn (so callers can
/// stamp `cost_usd` + `cost_source` onto `MessageMetadata`). Tokens are
/// accumulated unconditionally — the unknown-pricing path still
/// increments the running token counters, and bumps
/// `cost_unknown_turns` so the UI can label the session total as a
/// lower bound rather than the literal truth. `None` is returned only
/// when no `tokens` block was parsed at all (rare; mid-stream failure).
fn accumulate_session_usage(
    session: &mut ChatSession,
    tokens: Option<&TokenUsage>,
    pricing: Option<ModelPricing>,
) -> Option<TurnCost> {
    let tokens = tokens?;
    session.total_input_tokens = session
        .total_input_tokens
        .saturating_add(tokens.prompt as u64);
    session.total_output_tokens = session
        .total_output_tokens
        .saturating_add(tokens.completion as u64);
    if let Some(reasoning) = tokens.reasoning {
        session.total_reasoning_tokens = session
            .total_reasoning_tokens
            .saturating_add(reasoning as u64);
    }
    let turn = price_turn(tokens, pricing);
    match turn.amount_usd {
        Some(amount) if amount.is_finite() && amount >= 0.0 => {
            session.total_cost_usd += amount;
        }
        _ => {
            session.cost_unknown_turns = session.cost_unknown_turns.saturating_add(1);
        }
    }
    Some(turn)
}

/// W49: helper for the metadata stamp on tool-call rounds where we
/// re-price the same `tokens` block we just folded into the session
/// totals. Always reflects what `accumulate_session_usage` would have
/// recorded for the same input.
fn metadata_cost_fields(
    tokens: Option<&TokenUsage>,
    pricing: Option<ModelPricing>,
) -> (Option<f64>, Option<CostSource>) {
    match tokens.map(|t| price_turn(t, pricing)) {
        Some(turn) => (turn.amount_usd, Some(turn.source)),
        None => (None, None),
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

/// W48: resolver output for a user-mentioned source. Carries the
/// stable identity (definition id when available, else workflow id),
/// the kind for prompt clarity, a freshness/health snapshot, and a
/// pruned sample preview suitable for embedding in the system prompt
/// without bloating the provider request.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSourceMention {
    pub label: String,
    pub kind: crate::models::chat::SourceMentionKind,
    pub datasource_definition_id: Option<String>,
    pub workflow_id: Option<String>,
    pub widget_id: Option<String>,
    pub dashboard_id: Option<String>,
    pub input_alias: Option<String>,
    pub description: Option<String>,
    pub source_kind_label: Option<String>,
    pub tool_name: Option<String>,
    pub server_id: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_run_at: Option<i64>,
    pub last_duration_ms: Option<u32>,
    pub consumer_count: Option<u32>,
    pub sample_preview: Option<serde_json::Value>,
    pub arguments_preview: Option<serde_json::Value>,
}

/// W48: derive the validator's compact view from the resolved mention
/// list. Mentions that did not resolve to any identity are skipped so
/// the validator does not punish the agent for ghost references.
pub(crate) fn mentioned_sources_for_validation(
    resolved: &[ResolvedSourceMention],
) -> Vec<crate::commands::validation::MentionedSource> {
    resolved
        .iter()
        .filter(|r| r.datasource_definition_id.is_some() || r.workflow_id.is_some())
        .map(|r| crate::commands::validation::MentionedSource {
            label: r.label.clone(),
            datasource_definition_id: r.datasource_definition_id.clone(),
            workflow_id: r.workflow_id.clone(),
        })
        .collect()
}

/// W48: dedupe + sanity-trim incoming source mentions, stamp the active
/// dashboard id when the frontend omitted it for `kind == Widget`. We
/// never trust the wire to identify what is or isn't a valid source —
/// `resolve_source_mentions` does the real lookup; this just normalises
/// the incoming bundle so duplicates and blanks fall out early.
fn dedupe_source_mentions(
    mentions: &[crate::models::chat::SourceMention],
    active_dashboard_id: Option<&str>,
) -> Vec<crate::models::chat::SourceMention> {
    use std::collections::HashSet;
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out = Vec::new();
    for mention in mentions {
        let kind = mention.kind;
        let key_id = mention
            .datasource_definition_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| mention.workflow_id.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| mention.widget_id.clone().filter(|s| !s.trim().is_empty()));
        let Some(key_id) = key_id else { continue };
        let kind_key = format!("{:?}", kind);
        if !seen.insert((kind_key, key_id.clone())) {
            continue;
        }
        let mut next = mention.clone();
        if matches!(kind, crate::models::chat::SourceMentionKind::Widget)
            && next.dashboard_id.is_none()
        {
            next.dashboard_id = active_dashboard_id.map(|s| s.to_string());
        }
        out.push(next);
    }
    out
}

/// W48: walk every mention and produce a [`ResolvedSourceMention`].
/// Unknown identifiers still yield a row — with the label intact — so
/// the UI chip can render but the validator filter discards them.
pub(crate) async fn resolve_source_mentions(
    state: &AppState,
    mentions: &[crate::models::chat::SourceMention],
    active_dashboard_id: Option<&str>,
) -> Vec<ResolvedSourceMention> {
    let mut resolved = Vec::new();
    for mention in mentions {
        match mention.kind {
            crate::models::chat::SourceMentionKind::Datasource => {
                let mut row = ResolvedSourceMention::from_mention(mention);
                if let Some(def_id) = mention.datasource_definition_id.as_deref() {
                    if let Ok(Some(def)) = state.storage.get_datasource_definition(def_id).await {
                        row.fill_from_definition(&def);
                    }
                }
                resolved.push(row);
            }
            crate::models::chat::SourceMentionKind::Workflow => {
                let mut row = ResolvedSourceMention::from_mention(mention);
                if let Some(wf_id) = mention.workflow_id.as_deref() {
                    if let Ok(Some(def)) = state.storage.get_datasource_by_workflow_id(wf_id).await
                    {
                        row.fill_from_definition(&def);
                    } else if let Ok(Some(wf)) = state.storage.get_workflow(wf_id).await {
                        row.source_kind_label = Some("workflow".to_string());
                        row.workflow_id = Some(wf.id);
                        if row.label.trim().is_empty() {
                            row.label = wf.name;
                        }
                    }
                }
                resolved.push(row);
            }
            crate::models::chat::SourceMentionKind::Widget => {
                let mut row = ResolvedSourceMention::from_mention(mention);
                let dashboard_id = mention.dashboard_id.as_deref().or(active_dashboard_id);
                if let (Some(dashboard_id), Some(widget_id)) =
                    (dashboard_id, mention.widget_id.as_deref())
                {
                    if let Ok(Some(dashboard)) = state.storage.get_dashboard(dashboard_id).await {
                        if let Some(widget) = dashboard.layout.iter().find(|w| w.id() == widget_id)
                        {
                            if let Some(config) =
                                crate::commands::datasource::widget_datasource(widget)
                            {
                                row.workflow_id = Some(config.workflow_id.clone());
                                row.datasource_definition_id =
                                    config.datasource_definition_id.clone();
                                row.source_kind_label = Some("widget".to_string());
                                if let Some(def_id) = config.datasource_definition_id.as_deref() {
                                    if let Ok(Some(def)) =
                                        state.storage.get_datasource_definition(def_id).await
                                    {
                                        row.fill_from_definition(&def);
                                    }
                                } else if let Ok(Some(def)) = state
                                    .storage
                                    .get_datasource_by_workflow_id(&config.workflow_id)
                                    .await
                                {
                                    row.fill_from_definition(&def);
                                }
                            }
                            row.widget_id = Some(widget.id().to_string());
                            row.dashboard_id = Some(dashboard.id.clone());
                            if row.label.trim().is_empty() {
                                row.label = widget.title().to_string();
                            }
                        }
                    }
                }
                resolved.push(row);
            }
        }
    }
    resolved
}

impl ResolvedSourceMention {
    fn from_mention(mention: &crate::models::chat::SourceMention) -> Self {
        Self {
            label: mention.label.clone(),
            kind: mention.kind,
            datasource_definition_id: mention.datasource_definition_id.clone(),
            workflow_id: mention.workflow_id.clone(),
            widget_id: mention.widget_id.clone(),
            dashboard_id: mention.dashboard_id.clone(),
            input_alias: mention.input_alias.clone(),
            description: None,
            source_kind_label: None,
            tool_name: None,
            server_id: None,
            last_status: None,
            last_error: None,
            last_run_at: None,
            last_duration_ms: None,
            consumer_count: None,
            sample_preview: None,
            arguments_preview: None,
        }
    }

    fn fill_from_definition(&mut self, def: &crate::models::datasource::DatasourceDefinition) {
        use crate::models::dashboard::BuildDatasourcePlanKind;
        self.datasource_definition_id = Some(def.id.clone());
        self.workflow_id = Some(def.workflow_id.clone());
        self.description = def.description.clone();
        self.source_kind_label = Some(
            match def.kind {
                BuildDatasourcePlanKind::BuiltinTool => "builtin_tool",
                BuildDatasourcePlanKind::McpTool => "mcp_tool",
                BuildDatasourcePlanKind::ProviderPrompt => "provider_prompt",
                BuildDatasourcePlanKind::Shared => "shared",
                BuildDatasourcePlanKind::Compose => "compose",
            }
            .to_string(),
        );
        self.tool_name = def.tool_name.clone();
        self.server_id = def.server_id.clone();
        if let Some(args) = def.arguments.as_ref() {
            self.arguments_preview = Some(preview_json(args));
        }
        if let Some(health) = def.health.as_ref() {
            self.last_status = Some(
                match health.last_status {
                    crate::models::datasource::DatasourceHealthStatus::Ok => "ok",
                    crate::models::datasource::DatasourceHealthStatus::Error => "error",
                }
                .to_string(),
            );
            self.last_error = health.last_error.clone();
            self.last_run_at = Some(health.last_run_at);
            self.last_duration_ms = Some(health.last_duration_ms);
            self.consumer_count = Some(health.consumer_count);
            if let Some(preview) = health.sample_preview.as_ref() {
                self.sample_preview = Some(preview_json(preview));
            }
        }
        if self.label.trim().is_empty() {
            self.label = def.name.clone();
        }
    }

    fn to_prompt_json(&self) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "label".into(),
            serde_json::Value::String(self.label.clone()),
        );
        entry.insert(
            "mention_kind".into(),
            serde_json::Value::String(format!("{:?}", self.kind).to_lowercase()),
        );
        if let Some(id) = &self.datasource_definition_id {
            entry.insert(
                "datasource_definition_id".into(),
                serde_json::Value::String(id.clone()),
            );
        }
        if let Some(id) = &self.workflow_id {
            entry.insert("workflow_id".into(), serde_json::Value::String(id.clone()));
        }
        if let Some(id) = &self.widget_id {
            entry.insert("widget_id".into(), serde_json::Value::String(id.clone()));
        }
        if let Some(alias) = &self.input_alias {
            entry.insert(
                "input_alias".into(),
                serde_json::Value::String(alias.clone()),
            );
        }
        if let Some(s) = &self.source_kind_label {
            entry.insert("source_kind".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(s) = &self.tool_name {
            entry.insert("tool_name".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(s) = &self.server_id {
            entry.insert("server_id".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(s) = &self.description {
            entry.insert("description".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(s) = &self.last_status {
            entry.insert("last_status".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(s) = &self.last_error {
            entry.insert("last_error".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(n) = self.last_duration_ms {
            entry.insert("last_duration_ms".into(), serde_json::json!(n));
        }
        if let Some(n) = self.consumer_count {
            entry.insert("consumer_count".into(), serde_json::json!(n));
        }
        if let Some(preview) = &self.sample_preview {
            entry.insert("sample_preview".into(), preview.clone());
        }
        if let Some(args) = &self.arguments_preview {
            entry.insert("arguments_preview".into(), args.clone());
        }
        serde_json::Value::Object(entry)
    }
}

/// W48: assemble the system message that briefs the agent on the
/// sources the user named. Returns `None` for empty / fully unresolved
/// mention lists so we don't bloat the provider context with a noop
/// block.
pub(crate) fn build_source_mentions_prompt(resolved: &[ResolvedSourceMention]) -> Option<String> {
    if resolved.is_empty() {
        return None;
    }
    let usable: Vec<&ResolvedSourceMention> = resolved
        .iter()
        .filter(|r| r.datasource_definition_id.is_some() || r.workflow_id.is_some())
        .collect();
    let unresolved: Vec<&ResolvedSourceMention> = resolved
        .iter()
        .filter(|r| r.datasource_definition_id.is_none() && r.workflow_id.is_none())
        .collect();
    if usable.is_empty() && unresolved.is_empty() {
        return None;
    }
    let mut prompt = String::from(
        "## Source mentions (W48)\nThe user explicitly named the following EXISTING source(s) this turn. \
        Each entry below is a real, runnable source — DO NOT invent a new tool call to reproduce it. \
        Reuse the listed identifiers in your `datasource_plan`:\n\n\
        Rules:\n\
        - SINGLE source mention → emit one widget whose `datasource_plan.arguments.datasource_definition_id` is the listed id (or `workflow_id` for legacy entries).\n\
        - MULTIPLE source mentions → emit a widget with `kind: \"compose\"`. Put every mentioned source under `inputs`, keyed by the provided `input_alias` (or a clear snake_case slug). Each inner plan must reference the existing `datasource_definition_id` / `workflow_id` so we don't fan out duplicate tool calls.\n\
        - The outer compose `pipeline` should produce the final widget value (text widgets: a markdown narrative combining the named inputs; tables/charts: rows/series joined on a shared key).\n\
        - Text widgets must summarise every mentioned source in prose; the validator will reject a proposal that drops a mentioned source on the floor.\n\
        - Do NOT paste the raw `sample_preview` into the widget — it is for your reference only.\n\n",
    );
    if !usable.is_empty() {
        let usable_json: Vec<serde_json::Value> =
            usable.iter().map(|r| r.to_prompt_json()).collect();
        prompt.push_str("Sources:\n");
        prompt.push_str(
            &serde_json::to_string_pretty(&usable_json).unwrap_or_else(|_| "[]".to_string()),
        );
        prompt.push('\n');
    }
    if !unresolved.is_empty() {
        prompt.push_str(
            "\nThese mentions did not resolve to a saved datasource — treat them as stale and ignore:\n",
        );
        for entry in unresolved {
            prompt.push_str(&format!("- {}\n", entry.label));
        }
    }
    Some(prompt)
}

/// W38: drop empty/blank mentions, dedupe by `widget_id`, and stamp the
/// active dashboard id when the caller forgot. The frontend should already
/// scope mentions to the active dashboard, but we never trust the wire.
fn resolve_widget_mentions(
    mentions: &[crate::models::chat::WidgetMention],
    active_dashboard_id: Option<&str>,
) -> Vec<crate::models::chat::WidgetMention> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for mention in mentions {
        let trimmed_id = mention.widget_id.trim();
        if trimmed_id.is_empty() {
            continue;
        }
        if !seen.insert(trimmed_id.to_string()) {
            continue;
        }
        let mut next = mention.clone();
        next.widget_id = trimmed_id.to_string();
        if next.dashboard_id.is_none() {
            next.dashboard_id = active_dashboard_id.map(|s| s.to_string());
        }
        out.push(next);
    }
    out
}

/// Build the user message's parts, attaching a typed mention bundle when
/// the user named one or more existing widgets this turn. W48 extends
/// this with a parallel `SourceMentions` part for `@source` chips.
fn user_message_parts(
    content: &str,
    mentions: &[crate::models::chat::WidgetMention],
    source_mentions: &[crate::models::chat::SourceMention],
) -> Vec<ChatMessagePart> {
    let mut parts = text_parts(content);
    if !mentions.is_empty() {
        parts.push(ChatMessagePart::WidgetMentions {
            mentions: mentions.to_vec(),
        });
    }
    if !source_mentions.is_empty() {
        parts.push(ChatMessagePart::SourceMentions {
            mentions: source_mentions.to_vec(),
        });
    }
    parts
}

/// Extract a slice of widget ids from a mention list, suitable for the
/// validator's `target_widget_ids` argument.
fn target_widget_ids_from(mentions: &[crate::models::chat::WidgetMention]) -> Vec<String> {
    mentions.iter().map(|m| m.widget_id.clone()).collect()
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
            compression: result.compression.clone(),
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

/// W30: Build a compact catalog of saved DatasourceDefinitions for the
/// Build agent. The aim is reuse: when the agent emits a
/// `shared_datasources` entry that mirrors a saved definition's
/// (kind, server_id, tool_name) signature, the apply path can bind the
/// resulting widgets to the saved definition instead of duplicating it.
/// Returns `None` when nothing is saved so the prompt stays uncluttered.
async fn build_datasource_catalog_prompt(state: &AppState) -> anyhow::Result<Option<String>> {
    let definitions = state.storage.list_datasource_definitions().await?;
    if definitions.is_empty() {
        return Ok(None);
    }
    let entries: Vec<serde_json::Value> = definitions
        .iter()
        .take(20)
        .map(|def| {
            let kind = serde_json::to_value(&def.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let mut entry = serde_json::json!({
                "id": def.id,
                "name": def.name,
                "kind": kind,
                "pipeline_steps": def.pipeline.len(),
                "consumer_count": def.health.as_ref().map(|h| h.consumer_count).unwrap_or(0),
            });
            if let Some(server_id) = &def.server_id {
                entry["server_id"] = serde_json::Value::String(server_id.clone());
            }
            if let Some(tool_name) = &def.tool_name {
                entry["tool_name"] = serde_json::Value::String(tool_name.clone());
            }
            if let Some(description) = &def.description {
                entry["description"] = serde_json::Value::String(description.clone());
            }
            if let Some(health) = &def.health {
                entry["last_status"] =
                    serde_json::to_value(health.last_status).unwrap_or(serde_json::Value::Null);
                if let Some(sample) = &health.sample_preview {
                    let truncated = serde_json::to_string(sample)
                        .map(|s| {
                            if s.len() > 240 {
                                format!("{}…", &s[..240])
                            } else {
                                s
                            }
                        })
                        .unwrap_or_default();
                    entry["sample_preview"] = serde_json::Value::String(truncated);
                }
            }
            entry
        })
        .collect();
    let payload = serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string());
    Ok(Some(format!(
        r#"Saved datasources (W30 Workbench catalog) — prefer reusing one of these before inventing a new shared_datasource:
{}

Reuse guidance:
- If your proposal needs a source whose `(kind, server_id, tool_name)` matches an entry above, emit a `shared_datasources` entry with the same fields and reference it from your widget via `datasource_plan: {{ kind: "shared", source_key: "<your key>" }}`.
- Don't paraphrase a saved datasource into a near-duplicate; it ships as a separate workflow and breaks the Workbench's consumer view.
- Saved pipelines are deterministic; if the saved datasource already produces the shape you need, leave the widget pipeline empty."#,
        payload
    )))
}

async fn grounded_messages(
    state: &AppState,
    session: &ChatSession,
) -> anyhow::Result<Vec<ChatMessage>> {
    let mut messages = Vec::new();

    // W47: prepend the resolved language directive so it lands at the
    // top of the system stack — providers respect early instructions
    // more reliably than ones buried under context blocks. Resolve
    // failures are swallowed: a missing language is "auto", not an
    // outage. The session lookup uses storage rather than the in-memory
    // `session` so per-session overrides written via
    // `set_session_language_policy` are picked up before the next turn.
    if let Ok(resolved) = crate::commands::language::resolve_effective_language(
        state.storage.as_ref(),
        session.dashboard_id.as_deref(),
        Some(session.id.as_str()),
    )
    .await
    {
        if let Some(directive) = resolved.system_directive() {
            messages.push(system_message(directive));
        }
    }

    match session.mode {
        ChatMode::Build => {
            let mcp_tools = state.mcp_manager.list_tools().await;
            messages.push(system_message(build_chat_system_prompt(&mcp_tools)));
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
            // W30: surface the saved Datasource catalog so the Build agent
            // can reuse existing sources instead of inventing duplicate
            // shared_datasources entries.
            if let Some(catalog) = build_datasource_catalog_prompt(state).await? {
                messages.push(system_message(catalog));
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
    // W49: run every provider-facing message list through the
    // context-economy compactor. Large tool results become compact
    // status+shape summaries, oldest non-system turns get dropped with
    // an explicit `[context_truncated]` marker when we'd otherwise
    // ship hundreds of thousands of tokens to the provider. The local
    // session.messages is unchanged.
    let budget = crate::modules::context_budget::ContextBudget::default();
    let compacted = crate::modules::context_budget::compact_for_provider(messages, budget);
    if compacted.was_truncated() {
        tracing::info!(
            "chat context compacted: session={} original_chars={} final_chars={} tool_summaries={} dropped_turns={}",
            session.id,
            compacted.original_chars,
            compacted.final_chars,
            compacted.tool_summaries_applied,
            compacted.dropped_messages
        );
    }
    // W49: hard ceiling fail-closed. Even after dropping oldest turns
    // and pruning tool results, if the recent-N tail + system stack
    // would push the request 60% past the soft budget we refuse to
    // open the provider stream — better a typed `context_overflow`
    // error than a silent provider 4xx after the round-trip cost is
    // already burned.
    let hard_ceiling = budget.max_total_chars.saturating_mul(8) / 5;
    if compacted.final_chars > hard_ceiling {
        return Err(anyhow::anyhow!(
            "context_overflow: even after compaction the next provider request would be {} chars (~{} tokens), past the hard ceiling of {} chars (~{} tokens). Start a new session or drop large recent attachments.",
            compacted.final_chars,
            crate::modules::context_budget::estimate_tokens(compacted.final_chars),
            hard_ceiling,
            crate::modules::context_budget::estimate_tokens(hard_ceiling)
        ));
    }
    Ok(compacted.messages)
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
        Widget::Gallery { .. } => "gallery",
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

/// Pick a representative (server_id, tool_name) to seed the system prompt
/// examples. If at least one MCP server is connected, the agent sees a real
/// tool it can actually call; otherwise it sees abstract placeholders so it
/// asks the user to configure one rather than hallucinating names.
fn mcp_prompt_example(tools: &[crate::models::mcp::MCPTool]) -> (String, String) {
    let mut sorted: Vec<&crate::models::mcp::MCPTool> = tools.iter().collect();
    sorted.sort_by(|a, b| a.server_id.cmp(&b.server_id).then(a.name.cmp(&b.name)));
    match sorted.first() {
        Some(tool) => (tool.server_id.clone(), tool.name.clone()),
        None => (
            "<your_server_id>".to_string(),
            "<tool_from_tools_list>".to_string(),
        ),
    }
}

fn build_chat_system_prompt(tools: &[crate::models::mcp::MCPTool]) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let (example_server, example_tool) = mcp_prompt_example(tools);
    let body = format!(
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

11. Screenshot, generated chart image, status badge URL - `image`. The runtime image is clickable for fullscreen.

12. Collection of images from a datasource (RSS enclosures, image-search results, media MCP tool, GitHub release assets) - `gallery`. Runtime data is an array of items with `src` (URL or path) and optional `title`, `caption`, `alt`, `source`, `link`. The pipeline MUST produce the items - never hardcode a `data` array of image URLs (the validator rejects it). Use `kind: gallery` and shape the pipeline so each item has at least `{{src}}`. Useful for image lookup ("show me cat photos"), media feeds, release screenshots.

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
      "widget_type": "stat" | "gauge" | "chart" | "table" | "text" | "image" | "logs" | "bar_gauge" | "status_grid" | "heatmap" | "gallery",
      "title": "<widget title>",
      "replace_widget_id": "<optional - existing widget id to REPLACE; omit to ADD a new one>",
      "data": <small preview sample matching the runtime shape; see below; OPTIONAL>,
      "datasource_plan": {{
        "kind": "builtin_tool" | "mcp_tool" | "provider_prompt" | "shared" | "compose",
        "tool_name": "<http_request OR exact MCP tool name>",
        "server_id": "<required for mcp_tool>",
        "arguments": {{ }},
        "prompt": "<required for provider_prompt>",
        "output_path": "<optional dotted path inside the result to pick as widget data>",
        "refresh_cron": "<optional 6-field cron with seconds, e.g. '0 */15 * * * *' for every 15 minutes; omit for manual-only>",
        "pipeline": [ /* optional ordered deterministic transform steps - see Pipeline section below */ ],
        "source_key": "<required for kind='shared'; the matching shared_datasources key>",
        "inputs": {{ /* required for kind='compose': {{ <name>: <inner BuildDatasourcePlan>, ... }} - see Compose section */ }}
      }},
      "config": {{ ...see below... }},
      "size_preset": "kpi" | "half_width" | "wide_chart" | "full_width" | "table" | "text_panel" | "gallery",
      "layout_pattern": "kpi_row" | "trend_chart_row" | "operations_table" | "datasource_overview" | "media_board" | "text_panel"
      /* DO NOT set "x" or "y" on new widgets. Raw "w"/"h" only when no size_preset fits. */
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

- `gallery` (size w 6-12, h 5-8)
  runtime: `{{ items: [{{src, title?, caption?, alt?, source?, link?}}] }}` produced by the pipeline. `src` may be a remote URL (image-search result, RSS enclosure, GitHub asset). Items are clickable to open the lightbox with keyboard navigation.
  config: `{{ layout?: 'grid'|'row'|'masonry', thumbnail_aspect?: 'square'|'landscape'|'portrait'|'original', max_visible_items?: int, show_caption?: bool, show_source?: bool, fullscreen_enabled?: bool, fit?: 'cover'|'contain'|'fill', border_radius?: int }}`.
  Pipeline pattern (image search → gallery): `[{{kind:"pick", path:"hits[*]"}}, {{kind:"map", fields:["url","title","source"], rename:{{"url":"src"}}}}, {{kind:"limit", count:24}}]`. Never bake a literal `data` array of image URLs - the validator rejects it; the pipeline must produce the items.

Dashboard parameters (Grafana-style template variables)
When the user's request implies switching between several values (project names, environments, time ranges, services, regions), declare a `parameters` entry on the proposal root and reference it as `$name` or `${{name}}` inside widget `datasource_plan.arguments` / pipeline step configs instead of hardcoding the value.

How to use:
1. Add `parameters: [{{id, name, label, kind, default?, ...}}]` to the proposal root. `name` is what the widget refs as `$name`. `kind` is one of: `static_list` (fixed dropdown), `text_input`, `time_range`, `interval`, `constant`, `mcp_query` (live MCP tool call returns options), `http_query` (live HTTP request returns options), `datasource_query` (saved DatasourceDefinition produces options). Query-backed kinds run a pipeline tail that must shape output into `[{{label, value}}]` (or a plain array of scalars, which gets auto-doubled). Use `depends_on: ["other_param"]` to make a query parameter cascade — `$other_param` tokens inside `arguments`/`url`/`body` substitute at resolve time so "env → service" / "project → release" selectors work.
2. Reference the parameter inside widget arguments: `"arguments": {{"project": "$project"}}`. Whole-string tokens (`"$project"`) preserve type; mixed strings (`"/api/$project/list"`) interpolate as text.
3. Call `dry_run_widget` with `parameters` AND `parameter_values` so the dry-run substitutes a concrete value.

Example: build dashboard for any of several projects
```
parameters: [{{
  id: "project",
  name: "project",
  label: "Project",
  kind: "static_list",
  options: [{{label: "Alpha", value: "alpha"}}, {{label: "Beta", value: "beta"}}],
  default: "alpha"
}}]
widgets: [{{
  title: "Release count",
  widget_type: "stat",
  datasource_plan: {{
    kind: "mcp_tool",
    server_id: "<<MCP_EXAMPLE_SERVER>>",
    tool_name: "<<MCP_EXAMPLE_TOOL>>",
    arguments: {{"project": "$project"}},
    pipeline: [{{kind: "length"}}]
  }}
}}]
```

When NOT to use:
- Single-value dashboards where the value will never change (a literal is fine).
- Values derived from MCP tool output (use a pipeline step instead).
- The widget label / title — parameters drive datasource arguments, not display strings.

Shared datasources (share data across widgets)
When 2+ widgets pull from the same MCP/HTTP call, declare a SHARED datasource at the proposal level and have each widget reference it. The shared workflow runs once per refresh and fans out to every consumer - one MCP call, multiple widgets, consistent data.

How to use:
1. In the proposal root, add `shared_datasources: [{{key, kind, tool_name, server_id?, arguments?, pipeline?, refresh_cron?, label?}}]`. The `pipeline` is the BASE pipeline applied once to the raw source output before fan-out.
2. In each consumer widget, set `datasource_plan` to `{{kind: "shared", source_key: "<the key>", pipeline: [<per-widget tail>], output_path?: "<optional pre-tail pick>"}}`. The widget's own pipeline runs AFTER the shared pipeline, scoped to its tail.
3. `refresh_cron` lives on the shared entry, not on consumers. One cron tick = all consumers refresh.

Example: 3 widgets reading from the same source:
```
shared_datasources: [{{
  key: "shared_items",
  kind: "mcp_tool",
  server_id: "<<MCP_EXAMPLE_SERVER>>",
  tool_name: "<<MCP_EXAMPLE_TOOL>>",
  pipeline: [{{kind: "pick", path: "data.items"}}],
  refresh_cron: "0 */5 * * * *"
}}]
widgets: [
  {{title: "Item count", widget_type: "stat", datasource_plan: {{kind: "shared", source_key: "shared_items", pipeline: [{{kind: "length"}}]}}}},
  {{title: "Latest", widget_type: "stat", datasource_plan: {{kind: "shared", source_key: "shared_items", pipeline: [{{kind: "sort", by: "created_at", order: "desc"}}, {{kind: "head"}}, {{kind: "pick", path: "name"}}]}}}},
  {{title: "Items table", widget_type: "table", datasource_plan: {{kind: "shared", source_key: "shared_items", pipeline: [{{kind: "limit", count: 20}}]}}}}
]
```

When to use shared vs standalone:
- Use SHARED when 2+ widgets read from the SAME tool with the SAME arguments. Even if their per-widget pipelines diverge, the source is identical.
- Use STANDALONE when each widget's datasource is genuinely independent (different MCP tools, different HTTP endpoints, or same tool with very different arguments).
- Don't pre-aggregate in the shared pipeline - keep it close to the raw API shape so each consumer can navigate freely. Apply expensive base trims (e.g. `pick "data.items"`) in shared, leave specific filters/sorts/limits to consumers.

Compose datasources (one widget reads from N sources)
When the user asks for a SINGLE widget that combines data from two or more distinct sources (e.g. "show weather AND air quality together", "summarize commits plus incidents", "table of cities with temperature, AQI, and population"), use `kind: "compose"` on that widget. The widget runs each inner input independently, then the merged object `{{ name1: <output1>, name2: <output2>, ... }}` is fed to the widget's own `pipeline` and `output_path`.

How to use:
1. Set the widget's `datasource_plan` to `{{kind: "compose", inputs: {{ name1: <plan>, name2: <plan>, ... }}, pipeline?: [...], output_path?: "..." }}`.
2. Each inner `<plan>` is a regular `BuildDatasourcePlan` — `mcp_tool`, `builtin_tool`, `provider_prompt`, or `shared` (referencing `proposal.shared_datasources`). Inner inputs CANNOT themselves be `compose` (one level only).
3. The widget's outer `pipeline` operates on the merged object. Use `pick "name1"` / `pick "name2"` to navigate, `format "{{...}}"` or `llm_postprocess` for human-readable summaries, etc. Stat/Gauge widgets still need a final number; Table widgets still need an array of objects.
4. Refresh cron lives on the OUTER compose plan, not on inner inputs.
5. If inputs read sources also used by OTHER widgets, declare those sources as `shared_datasources` and have each compose input use `{{kind:"shared", source_key:"<key>"}}` so the fetch isn't duplicated.

Example: a single widget that combines two independent sources into one summary
```
shared_datasources: [
  {{key: "primary_metric", kind: "builtin_tool", tool_name: "http_request",
    arguments: {{method: "GET", url: "https://api.example.com/primary"}}}},
  {{key: "secondary_metric", kind: "builtin_tool", tool_name: "http_request",
    arguments: {{method: "GET", url: "https://api.example.com/secondary"}}}}
]
widgets: [
  // ... per-source widgets reading shared keys directly ...
  {{
    title: "Combined summary",
    widget_type: "text",
    datasource_plan: {{
      kind: "compose",
      inputs: {{
        primary: {{kind: "shared", source_key: "primary_metric", pipeline: [{{kind: "pick", path: "current.value"}}]}},
        secondary: {{kind: "shared", source_key: "secondary_metric", pipeline: [{{kind: "pick", path: "current.value"}}]}}
      }},
      pipeline: [
        {{kind: "format", template: "Primary **{{primary}}**, secondary **{{secondary}}**."}}
      ]
    }}
  }}
]
```

Example: a table joining rows from two sources by a shared key (e.g. id, name, region). Outer pipeline picks each input out of the merged object and emits the row array via `llm_postprocess` (or a deterministic `set`+`format` recipe) so each row carries fields from both sources.

When to use compose vs standalone widgets:
- Use COMPOSE when ONE widget must show values derived from 2+ sources together (a combined summary, a stat showing X/Y, a table joined by key).
- Use SEPARATE widgets when the values are independent and the user is fine seeing them side-by-side. Don't reach for compose just to "tidy up" the dashboard.
- The compose dry_run runs every inner fetch, so it costs as many HTTP/MCP calls as it has inputs. Keep inputs minimal.

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
- {{"kind": "mcp_call", "server_id": "<<MCP_EXAMPLE_SERVER>>", "tool_name": "<<MCP_EXAMPLE_TOOL>>", "arguments": {{"id": "$_.id"}}}} - call any MCP tool mid-pipeline. The tool result REPLACES the current pipeline value. Use `"$_"` to pass the current value as-is, or `"$_.field.path"` to pluck a field (type-preserving in whole-string tokens; `"prefix-$_.id"` interpolates as text). Useful for list-then-fetch-each: first step lists ids, then `mcp_call` enriches the chosen one. Costs one extra MCP call per refresh, so prefer plain `pick`/`filter` when local transforms suffice.

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

Layout grid (W45)
- The grid is **12 columns** wide. Y grows downward. Auto-pack packs new widgets row-first using just `w`/`h`; order in `proposal.widgets` defines the row.
- **DO NOT set `x` or `y` on widgets.** The validator now FAILS the proposal with `proposed_explicit_coordinates` if you do.
- Prefer a **named `size_preset`** over raw `w`/`h`. Setting both fails the validator with `conflicting_layout_fields`. Pick ONE per new widget:
    * `kpi` — small KPI card (stat: 3x2, gauge: 3x3, bar_gauge: 4x3). 4 per row.
    * `half_width` — half-row panel (~6 cols wide, 4-6 high). Use for chart-next-to-status, side-by-side comparisons.
    * `wide_chart` — chart or bar_gauge spanning 8 cols. Pair with a 4-col `kpi`/`half_width` neighbor to fill the row.
    * `full_width` — full 12-col panel, modest height (chart/heatmap/bar_gauge/gallery).
    * `table` — wide tabular block (table: 12x8, logs: 12x7). Use for scannable rows.
    * `text_panel` — markdown panel (6x4).
    * `gallery` — image gallery / image card (8x6).
- Also pick a `layout_pattern` to signal intent (does not change packing, but helps future tooling):
    * `kpi_row` — 3-6 stat widgets across the top.
    * `trend_chart_row` — wide chart leading a row.
    * `operations_table` — full-width table row.
    * `datasource_overview` — status_grid + supporting stats.
    * `media_board` — gallery + image cards.
    * `text_panel` — markdown panel with supporting metrics.
- Raw `w`/`h` is still accepted when no preset fits, but `size_preset` is the preferred default.
- Typical layouts (each row sums to 12 via presets):
    * Executive: row 1 = 4x `kpi` stat; row 2 = `wide_chart` (8) + `kpi` gauge x2 (3+3 not 4 — use `half_width` chart + 2x kpi gauges instead); row 3 = `table`.
    * Operations: row 1 = `kpi` strip; row 2 = `wide_chart` + `half_width` status_grid; row 3 = `table` logs.
    * Media: row 1 = `kpi`; row 2 = `full_width` gallery; row 3 = `text_panel` summary.
- Replacement widgets (`replace_widget_id` set): the layout gate is skipped; the new widget inherits the old slot's position. You may still set `size_preset` to resize the replacement.

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
    );
    body.replace("<<MCP_EXAMPLE_SERVER>>", &example_server)
        .replace("<<MCP_EXAMPLE_TOOL>>", &example_tool)
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
    build_mode: bool,
) -> anyhow::Result<Vec<AIToolSpec>> {
    let mut specs: Vec<AIToolSpec> = Vec::new();
    if build_mode {
        specs.push(submit_plan_tool_spec());
    }
    specs.push(AIToolSpec {
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
    });

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

    // W51: bounded raw-artifact slice recovery. The compressor wraps
    // bulky tool results into a compact summary plus a `raw_artifact_id`
    // pointer; this tool lets the agent ask for a small JSON-pointer
    // slice, row window, or byte slice of the raw payload when the
    // compact summary intentionally hid the detail it needs.
    specs.push(inspect_artifact_tool_spec());

    // W37: enabled external sources (HN search, Wikipedia, CoinGecko,
    // public GitHub repo metadata, etc.) appear as first-class typed
    // tools so the LLM does not have to guess endpoints with raw
    // http_request. Disabled / blocked / credential-missing sources are
    // filtered out before this point and never reach the LLM.
    match crate::commands::external_source::list_runnable_external_sources(state).await {
        Ok(runnable) => {
            for (source, descriptor) in runnable {
                specs.push(AIToolSpec {
                    name: descriptor.tool_name,
                    description: format!(
                        "{}{}",
                        descriptor.description,
                        source
                            .attribution
                            .as_deref()
                            .map(|a| format!(" (Attribution: {})", a))
                            .unwrap_or_default()
                    ),
                    parameters: descriptor.parameters_schema,
                });
            }
        }
        Err(error) => {
            tracing::warn!("W37: skipping external source tool specs: {}", error);
        }
    }

    // Dry-run tool - lets the agent validate a single widget proposal by
    // building its workflow, executing it once with no persistence, and
    // returning the widget runtime data (or the error). Use BEFORE emitting
    // the final proposal JSON for any widget where shape correctness
    // matters: stat/gauge numbers, table/chart row shapes, pipeline
    // aggregates, anything with `llm_postprocess`.
    specs.push(AIToolSpec {
        name: "dry_run_widget".to_string(),
        description: "Test a single widget proposal end-to-end without persisting anything: builds the workflow, runs the datasource_plan + pipeline once, and returns the actual widget runtime data or the error. Use this BEFORE committing widgets to the final dashboard proposal so you can verify the pipeline produces the right shape (a number for stat/gauge, an array of objects for chart/table, etc.). Cheap to call.\n\nCALL SHAPE — wrap the widget under the `proposal` key, not at the top level:\n```\n{\n  \"proposal\": { \"widget_type\": \"stat\", \"title\": \"Tokyo · Temperature\", \"datasource_plan\": {...}, \"config\": {...}, \"size_preset\": \"kpi\" },\n  \"shared_datasources\": [ { \"key\": \"forecast\", \"kind\": \"builtin_tool\", \"tool_name\": \"http_request\", \"arguments\": {...}, \"pipeline\": [...] } ]\n}\n```\n\nVALIDATION BINDING — the final-proposal validator binds dry-run evidence to widget titles. `proposal.title` MUST equal the exact title of the widget you will emit (punctuation, casing, whitespace; the matcher is also punctuation-insensitive). If you plan to ship many near-identical widgets that share the SAME pipeline shape (e.g. one stat per city), do ONE dry-run for the representative and pass `titles_covered` listing every final title it stands in for — the validator treats each entry as if it had its own successful dry-run.\n\nFor widgets with datasource_plan.kind='shared', ALSO pass the matching `shared_datasources` entry so the dry-run can inline the source + base pipeline. If the widget references `$param` tokens, pass `parameters` (declarations) and `parameter_values` (concrete values) so substitution happens at dry-run time.".to_string(),
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
                },
                "titles_covered": {
                    "type": "array",
                    "description": "Optional list of additional widget titles this dry-run stands in for. Use when several final widgets share the SAME pipeline shape (e.g. one stat per city). Each title behaves as if it had its own successful dry-run for validator purposes.",
                    "items": { "type": "string" }
                },
                "parameters": {
                    "type": "array",
                    "description": "W25: dashboard parameter declarations referenced by the widget. Required when the widget references `$name` tokens so the dry-run can substitute them.",
                    "items": { "type": "object", "additionalProperties": true }
                },
                "parameter_values": {
                    "type": "object",
                    "description": "W25: concrete values keyed by parameter name. Used to substitute `$name` tokens before the dry-run executes.",
                    "additionalProperties": true
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
    build_mode: bool,
) -> anyhow::Result<Vec<AIToolSpec>> {
    let mut specs: Vec<AIToolSpec> = Vec::new();
    if build_mode {
        specs.push(submit_plan_tool_spec());
    }
    specs.push(AIToolSpec {
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
    });

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
    // W51: mirror the streaming-path inspect_artifact registration so
    // silent retries can recover bounded raw detail too.
    specs.push(inspect_artifact_tool_spec());
    // W37: same external source injection as the streaming path so
    // silent retries (validation-failure retries, build re-prompts) see
    // an identical tool surface.
    if let Ok(runnable) =
        crate::commands::external_source::list_runnable_external_sources(state).await
    {
        for (source, descriptor) in runnable {
            specs.push(AIToolSpec {
                name: descriptor.tool_name,
                description: format!(
                    "{}{}",
                    descriptor.description,
                    source
                        .attribution
                        .as_deref()
                        .map(|a| format!(" (Attribution: {})", a))
                        .unwrap_or_default()
                ),
                parameters: descriptor.parameters_schema,
            });
        }
    }
    Ok(specs)
}

/// W51: shared `inspect_artifact` tool spec so both the streaming and
/// silent tool catalogs use identical schemas.
fn inspect_artifact_tool_spec() -> AIToolSpec {
    AIToolSpec {
        name: "inspect_artifact".to_string(),
        description: "Request a bounded slice of a previous tool result's raw payload. Datrina compresses bulky tool results (HTTP, MCP, datasource, pipeline) and stores the redacted raw locally; the compact summary you saw includes a `raw_artifact_id` and `truncation` markers that name exactly which slice was hidden. Use this tool when the compact summary omitted a value you need. Returns the slice plus its byte/row metadata; never echoes secrets.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "artifact_id": { "type": "string", "description": "The `raw_artifact_id` value from a previous tool result's `_compression` block." },
                "path": { "type": "string", "description": "Optional JSON pointer (`/foo/bar` or `$.foo.bar`) into the raw payload. Default: whole payload." },
                "row_start": { "type": "integer", "minimum": 0, "description": "If the slice is an array, start index. Default 0." },
                "row_limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "If the slice is an array, max rows to return. Default 20, hard cap 200." },
                "byte_limit": { "type": "integer", "minimum": 256, "maximum": 32000, "description": "Hard cap on the returned slice serialisation. Default 8000, hard cap 32000." }
            },
            "required": ["artifact_id"],
            "additionalProperties": false
        }),
    }
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
    parameters_value: Option<serde_json::Value>,
    parameter_values_value: Option<serde_json::Value>,
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

    // W25: optional parameter substitution. The agent may pass declared
    // parameters + concrete values so a dry-run with `$project` resolves
    // to a real value at proposal time.
    let parameters: Vec<crate::models::dashboard::DashboardParameter> = match parameters_value {
        Some(value) => serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("dry_run_widget: invalid parameters shape: {}", e))?,
        None => Vec::new(),
    };
    let parameter_values: std::collections::BTreeMap<
        String,
        crate::models::dashboard::ParameterValue,
    > = match parameter_values_value {
        Some(value) => serde_json::from_value(value).map_err(|e| {
            anyhow::anyhow!("dry_run_widget: invalid parameter_values shape: {}", e)
        })?,
        None => std::collections::BTreeMap::new(),
    };
    let resolved_params = if parameters.is_empty() && parameter_values.is_empty() {
        None
    } else if parameters.is_empty() {
        Some(crate::modules::parameter_engine::ResolvedParameters::from_map(parameter_values))
    } else {
        crate::modules::parameter_engine::ResolvedParameters::resolve(
            &parameters,
            &parameter_values,
        )
        .ok()
    };
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

    let (widget, mut workflow) =
        crate::commands::dashboard::proposal_widget_public(&proposal, 0, now)?;
    use crate::commands::dashboard::WidgetDatasource;
    let datasource = widget
        .datasource()
        .ok_or_else(|| anyhow::anyhow!("Widget has no datasource workflow"))?;

    if let Some(params) = &resolved_params {
        crate::modules::parameter_engine::substitute_workflow(
            &mut workflow,
            params,
            crate::modules::parameter_engine::SubstituteOptions::default(),
        );
    }

    // Reconnect MCP servers for the duration of this single run.
    let mcp_servers = state.storage.list_mcp_servers().await?;
    for server in mcp_servers.into_iter().filter(|server| server.is_enabled) {
        if !state.mcp_manager.is_connected(&server.id).await {
            state.tool_engine.validate_mcp_server(&server)?;
            let _ = state.mcp_manager.connect(server).await;
        }
    }

    // W29: dry-run does not strictly require a provider — pipelines
    // without an `llm_postprocess` step run fine. If the operator hasn't
    // picked an active provider yet, fall through without one; the
    // workflow engine reports a clear error if an LLM step is reached
    // without a provider.
    let provider = match crate::resolve_active_provider(state.storage.as_ref()).await? {
        Ok(provider) => Some(provider),
        Err(_setup_error) => None,
    };
    // W47: chat-driven dry runs inherit the app default language;
    // dashboard/session scope isn't surfaced from here.
    let language_directive =
        crate::commands::language::resolve_effective_language(state.storage.as_ref(), None, None)
            .await
            .ok()
            .and_then(|resolved| resolved.system_directive());
    let engine = crate::modules::workflow_engine::WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        provider,
    )
    .with_language(language_directive);
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
            // Be forgiving about how the model wraps the proposal. Models
            // alternate between {proposal: {...}}, {widget: {...}}, and
            // flattened-fields shapes; all carry the same payload.
            let proposal_value = call
                .arguments
                .get("proposal")
                .or_else(|| call.arguments.get("widget"))
                .cloned()
                .unwrap_or_else(|| {
                    serde_json::Value::Object(
                        call.arguments.as_object().cloned().unwrap_or_default(),
                    )
                });
            let shared_value = call.arguments.get("shared_datasources").cloned();
            let parameters_value = call.arguments.get("parameters").cloned();
            let parameter_values_value = call.arguments.get("parameter_values").cloned();
            execute_dry_run_widget(
                state,
                proposal_value,
                shared_value,
                parameters_value,
                parameter_values_value,
            )
            .await
        }
        "recall" => execute_recall_tool(state, session, &call.arguments).await,
        // W51: bounded raw artifact detail recovery. Lives inside the
        // chat tool dispatcher so it inherits MAX_TOOL_ITERATIONS,
        // loop-detection, cost budget, and the same policy boundary as
        // every other tool. Never registered as a public Tauri command.
        "inspect_artifact" => execute_inspect_artifact_tool(state, session, &call.arguments).await,
        other => {
            if let Some(source_id) =
                crate::commands::external_source::parse_external_source_tool_name(other)
            {
                crate::commands::external_source::run_external_source_tool(
                    state,
                    source_id,
                    &call.arguments,
                )
                .await
            } else {
                Err(anyhow::anyhow!(
                    "Tool '{}' is not exposed to chat tool calling",
                    call.name
                ))
            }
        }
    };

    match outcome {
        Ok(result) => maybe_compress_tool_result(state, session, call, result).await,
        Err(error) => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: serde_json::json!({ "status": "error" }),
            error: Some(error.to_string()),
            compression: None,
        },
    }
}

/// W51: route every successful tool result through
/// [`crate::modules::context_compressor`] before the provider sees it.
/// The compact value replaces `ToolResult.result` so both the persisted
/// session and the next provider turn carry the same compact shape.
/// The redacted raw payload is retained locally in
/// [`crate::modules::storage::Storage::raw_artifacts`] so chat trace,
/// Pipeline Debug, and the model (via `inspect_artifact`) can request
/// bounded detail later. The `inspect_artifact` tool itself is exempt
/// from compression so the agent receives the literal slice it asked
/// for.
async fn maybe_compress_tool_result(
    state: &AppState,
    session: &ChatSession,
    call: &crate::models::chat::ToolCall,
    raw: serde_json::Value,
) -> ToolResult {
    use crate::modules::context_compressor::{compress, CompressionProfile};

    if call.name == "inspect_artifact" {
        return ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: raw,
            error: None,
            compression: None,
        };
    }

    let raw_size = serde_json::to_string(&raw).map(|s| s.len()).unwrap_or(0);
    let profile = CompressionProfile::for_tool(&call.name);
    let mut artifact = compress(profile, &raw);
    // Only persist a raw artifact when we actually saved meaningful
    // bytes — small results stay in `ToolResult.result` verbatim.
    let should_retain = artifact.raw_bytes >= 2_000
        && artifact.raw_bytes.saturating_sub(artifact.compact_bytes) > 512;
    if should_retain {
        let payload_json = serde_json::to_string(&raw).unwrap_or_default();
        let checksum = format!("{:x}", md5_sum(&payload_json));
        match state
            .storage
            .store_raw_artifact(
                "chat_session",
                &session.id,
                profile.as_str(),
                artifact.raw_bytes,
                artifact.compact_bytes,
                &checksum,
                1,
                "ephemeral",
                &payload_json,
            )
            .await
        {
            Ok(id) => {
                artifact = artifact.with_raw_artifact_ref(id.clone());
                // Best-effort cap: keep the most recent 50 ephemeral
                // artifacts per session so the table stays bounded.
                if let Err(e) = state
                    .storage
                    .prune_raw_artifacts("chat_session", &session.id, 50)
                    .await
                {
                    tracing::warn!("W51: prune_raw_artifacts failed: {}", e);
                }
                let _ = id;
            }
            Err(e) => {
                tracing::warn!("W51: store_raw_artifact failed: {}", e);
            }
        }
    }

    let truncation_paths: Vec<String> = artifact
        .truncation
        .iter()
        .map(|marker| marker.path.clone())
        .collect();
    let compression = crate::models::chat::ToolResultCompression {
        profile: artifact.profile.as_str().to_string(),
        raw_bytes: artifact.raw_bytes,
        compact_bytes: artifact.compact_bytes,
        estimated_tokens_saved: artifact.estimated_tokens_saved,
        raw_artifact_id: artifact.raw_artifact_ref.clone(),
        truncation_paths,
    };
    // Decide what `ToolResult.result` should carry: when the payload
    // was small enough to skip retention, leave the raw value so the
    // local session stays high-fidelity. Otherwise persist the compact
    // value (with a `_artifact_id` pointer when one exists) so both
    // the session and the provider see the same compressed shape.
    let result_value = if should_retain {
        let _ = raw_size;
        artifact.compact.clone()
    } else {
        raw
    };
    ToolResult {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        result: result_value,
        error: None,
        compression: Some(compression),
    }
}

/// Lightweight MD5 over the artifact bytes for the `raw_artifacts`
/// checksum column. Not security-sensitive — used only to detect
/// duplicate persists and verify integrity in debug surfaces.
fn md5_sum(input: &str) -> u128 {
    // Tiny FNV-1a fallback so we don't pull in another crate just for
    // a debug checksum. Collision risk is acceptable for the dedup
    // hint use case.
    let mut hash: u128 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// W51: bounded raw-artifact slice the agent can request via the
/// `inspect_artifact` tool. Returns the raw JSON pointer slice (or
/// line range / row window) capped by `byte_limit`. Fails closed when
/// the artifact id is unknown, expired, or unsafe to expose.
async fn execute_inspect_artifact_tool(
    state: &AppState,
    session: &ChatSession,
    arguments: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let artifact_id = arguments
        .get("artifact_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("inspect_artifact: 'artifact_id' is required"))?;
    let pointer = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let row_start = arguments
        .get("row_start")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let row_limit = arguments
        .get("row_limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(20)
        .min(200) as usize;
    let byte_limit = arguments
        .get("byte_limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(8_000)
        .min(32_000) as usize;

    let record = state
        .storage
        .get_raw_artifact(&artifact_id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "inspect_artifact: artifact '{}' not found (already pruned or never written)",
                artifact_id
            )
        })?;

    if record.owner_kind != "chat_session" || record.owner_id != session.id {
        return Err(anyhow::anyhow!(
            "inspect_artifact: artifact '{}' does not belong to this session",
            artifact_id
        ));
    }

    let raw: serde_json::Value = serde_json::from_str(&record.payload_json)
        .map_err(|e| anyhow::anyhow!("inspect_artifact: raw payload is not valid JSON: {}", e))?;

    let scoped = if let Some(ptr) = pointer.as_deref() {
        let json_pointer = if ptr.starts_with('/') {
            ptr.to_string()
        } else {
            format!(
                "/{}",
                ptr.trim_start_matches('$')
                    .trim_start_matches('.')
                    .replace('.', "/")
            )
        };
        raw.pointer(&json_pointer).cloned().ok_or_else(|| {
            anyhow::anyhow!("inspect_artifact: path '{}' not found in artifact", ptr)
        })?
    } else {
        raw
    };

    let scoped = match scoped {
        serde_json::Value::Array(items) => {
            let end = (row_start + row_limit).min(items.len());
            let slice: Vec<serde_json::Value> = items
                .iter()
                .skip(row_start)
                .take(row_limit)
                .cloned()
                .collect();
            serde_json::json!({
                "kind": "array_slice",
                "row_start": row_start,
                "row_end": end,
                "total_rows": items.len(),
                "rows": slice,
            })
        }
        other => other,
    };

    let encoded = serde_json::to_string(&scoped).unwrap_or_default();
    let bounded = if encoded.len() > byte_limit {
        serde_json::json!({
            "kind": "byte_truncated",
            "byte_limit": byte_limit,
            "char_count": encoded.chars().count(),
            "hint": "slice exceeded byte_limit; re-call inspect_artifact with a narrower path or row_window",
        })
    } else {
        scoped
    };

    Ok(serde_json::json!({
        "artifact_id": record.id,
        "profile": record.profile,
        "raw_size": record.raw_size,
        "compact_size": record.compact_size,
        "slice": bounded,
    }))
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

    fn empty_session_with_cap(cap: Option<f64>, spent: f64) -> ChatSession {
        ChatSession {
            id: "s".to_string(),
            mode: ChatMode::Build,
            dashboard_id: None,
            widget_id: None,
            title: "t".to_string(),
            messages: vec![],
            current_plan: None,
            plan_status: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_reasoning_tokens: 0,
            total_cost_usd: spent,
            cost_unknown_turns: 0,
            max_cost_usd: cap,
            language_override: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn budget_check_passes_without_cap() {
        let session = empty_session_with_cap(None, 0.0);
        assert!(enforce_session_budget(&session, None, &[]).is_ok());
    }

    #[test]
    fn budget_check_blocks_when_already_over_cap() {
        let session = empty_session_with_cap(Some(0.01), 0.05);
        let err = enforce_session_budget(&session, None, &[]).unwrap_err();
        assert!(err.to_string().contains("budget_exceeded"));
    }

    #[test]
    fn budget_check_blocks_when_projection_exceeds_cap() {
        // 200k input tokens at $10/1M = $2.00 input cost alone.
        let session = empty_session_with_cap(Some(0.50), 0.0);
        let pricing = ModelPricing {
            input_usd_per_1m: 10.0,
            output_usd_per_1m: 30.0,
            reasoning_usd_per_1m: None,
        };
        let messages: Vec<ChatMessage> = (0..200)
            .map(|i| ChatMessage {
                id: format!("m-{i}"),
                role: MessageRole::User,
                content: "x".repeat(4_000),
                parts: vec![],
                mode: ChatMode::Build,
                tool_calls: None,
                tool_results: None,
                metadata: None,
                timestamp: 0,
            })
            .collect();
        let err = enforce_session_budget(&session, Some(pricing), &messages).unwrap_err();
        assert!(err.to_string().contains("budget_exceeded"));
    }

    #[test]
    fn accumulate_session_usage_updates_totals_and_cost() {
        let mut session = empty_session_with_cap(Some(1.00), 0.0);
        let pricing = ModelPricing {
            input_usd_per_1m: 1.0,
            output_usd_per_1m: 3.0,
            reasoning_usd_per_1m: None,
        };
        let tokens = TokenUsage {
            prompt: 1_000_000,
            completion: 1_000_000,
            reasoning: None,
            provider_cost_usd: None,
        };
        let turn = accumulate_session_usage(&mut session, Some(&tokens), Some(pricing));
        assert_eq!(session.total_input_tokens, 1_000_000);
        assert_eq!(session.total_output_tokens, 1_000_000);
        assert!((session.total_cost_usd - 4.0).abs() < 1e-9);
        let turn = turn.expect("turn cost emitted");
        assert!((turn.amount_usd.unwrap() - 4.0).abs() < 1e-9);
        assert_eq!(
            turn.source,
            crate::models::pricing::CostSource::PricingTable
        );
        assert_eq!(session.cost_unknown_turns, 0);
    }

    #[test]
    fn accumulate_session_prefers_provider_total_cost() {
        // W49: when the provider already billed the turn (OpenRouter
        // `usage.cost`), accounting uses that figure verbatim rather
        // than re-deriving it from tokens × pricing-table.
        let mut session = empty_session_with_cap(None, 0.0);
        let pricing = ModelPricing {
            input_usd_per_1m: 1.0,
            output_usd_per_1m: 3.0,
            reasoning_usd_per_1m: None,
        };
        let tokens = TokenUsage {
            prompt: 1_000_000,
            completion: 1_000_000,
            reasoning: None,
            provider_cost_usd: Some(0.42),
        };
        let turn = accumulate_session_usage(&mut session, Some(&tokens), Some(pricing))
            .expect("turn cost emitted");
        assert_eq!(
            turn.source,
            crate::models::pricing::CostSource::ProviderTotal
        );
        assert!((turn.amount_usd.unwrap() - 0.42).abs() < 1e-9);
        assert!((session.total_cost_usd - 0.42).abs() < 1e-9);
    }

    #[test]
    fn accumulate_session_unknown_pricing_marks_turn_not_zero() {
        // W49: tokens with no pricing entry must surface as
        // `unknown_pricing`, never as a silent $0 contribution.
        let mut session = empty_session_with_cap(None, 0.0);
        let tokens = TokenUsage {
            prompt: 1_000,
            completion: 500,
            reasoning: None,
            provider_cost_usd: None,
        };
        let turn = accumulate_session_usage(&mut session, Some(&tokens), None)
            .expect("turn emitted even without pricing");
        assert!(turn.amount_usd.is_none());
        assert_eq!(
            turn.source,
            crate::models::pricing::CostSource::UnknownPricing
        );
        assert_eq!(session.total_input_tokens, 1_000);
        assert_eq!(session.total_output_tokens, 500);
        assert_eq!(session.total_cost_usd, 0.0);
        assert_eq!(session.cost_unknown_turns, 1);
    }

    #[test]
    fn widget_mentions_roundtrip_via_send_request() {
        // W38: SendMessageRequest must accept widget_mentions on the
        // wire shape the frontend sends.
        let wire = serde_json::json!({
            "content": "fix this",
            "widget_mentions": [
                {"widget_id": "w_alpha", "label": "Alpha", "widget_kind": "stat"},
                {"widget_id": "w_alpha", "label": "Alpha duplicate"},
            ]
        });
        let req: crate::models::chat::SendMessageRequest =
            serde_json::from_value(wire).expect("parses");
        assert_eq!(req.widget_mentions.len(), 2);
        let resolved = resolve_widget_mentions(&req.widget_mentions, Some("dash-1"));
        // Duplicates dropped, dashboard id stamped onto each entry.
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].widget_id, "w_alpha");
        assert_eq!(resolved[0].dashboard_id.as_deref(), Some("dash-1"));
        // target_widget_ids_from yields the user's scope verbatim.
        let ids = target_widget_ids_from(&resolved);
        assert_eq!(ids, vec!["w_alpha".to_string()]);
    }

    #[test]
    fn widget_mentions_dedupe_preserves_distinct_ids_for_duplicate_titles() {
        // Two widgets with the same title but distinct ids — both must
        // survive resolution since `widget_id` is the dedupe key.
        let mentions = vec![
            crate::models::chat::WidgetMention {
                widget_id: "w_a".into(),
                dashboard_id: None,
                label: "Active users".into(),
                widget_kind: Some("stat".into()),
            },
            crate::models::chat::WidgetMention {
                widget_id: "w_b".into(),
                dashboard_id: None,
                label: "Active users".into(),
                widget_kind: Some("stat".into()),
            },
        ];
        let resolved = resolve_widget_mentions(&mentions, None);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].widget_id, "w_a");
        assert_eq!(resolved[1].widget_id, "w_b");
    }

    #[test]
    fn send_request_defaults_when_mentions_field_omitted() {
        // Legacy frontends that don't yet send widget_mentions must
        // still parse (default == empty list).
        let wire = serde_json::json!({"content": "hi"});
        let req: crate::models::chat::SendMessageRequest =
            serde_json::from_value(wire).expect("parses without widget_mentions");
        assert!(req.widget_mentions.is_empty());
    }

    #[test]
    fn accumulate_without_pricing_still_tracks_tokens() {
        let mut session = empty_session_with_cap(None, 0.0);
        let tokens = TokenUsage {
            prompt: 100,
            completion: 50,
            reasoning: Some(20),
            provider_cost_usd: None,
        };
        let turn =
            accumulate_session_usage(&mut session, Some(&tokens), None).expect("turn emitted");
        assert_eq!(session.total_input_tokens, 100);
        assert_eq!(session.total_output_tokens, 50);
        assert_eq!(session.total_reasoning_tokens, 20);
        assert_eq!(session.total_cost_usd, 0.0);
        assert_eq!(
            turn.source,
            crate::models::pricing::CostSource::UnknownPricing
        );
        assert!(turn.amount_usd.is_none());
        assert_eq!(session.cost_unknown_turns, 1);
    }
}
