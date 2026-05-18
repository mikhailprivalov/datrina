use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{header, Client};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::models::chat::{ChatMessage, MessageRole, ToolCall};
use crate::models::dashboard::Dashboard;
use crate::models::provider::{
    provider_capabilities, supports_structured_output, EffectiveWidgetModel, LLMProvider,
    ProviderKind, ProviderRuntimeStatus, ProviderTestResult, StructuredOutputCapability,
    WidgetCapability, WidgetModelError, WidgetModelSource,
};
use crate::models::widget::Widget;

#[derive(Clone)]
pub struct AIEngine {
    client: Client,
}

pub struct AIResponse {
    pub content: String,
    pub provider_id: String,
    pub model: String,
    pub tokens: Option<crate::models::chat::TokenUsage>,
    pub latency_ms: u64,
    pub tool_calls: Vec<ToolCall>,
    pub reasoning: Option<String>,
    /// W33: resolved structured-output mode actually applied to this
    /// request. Defaults to `PlainText` because most chat turns don't
    /// request a strict body; the proposal-emission retry path is the
    /// notable exception. Surfaced so acceptance reports can distinguish
    /// strict-mode evidence from a soft fallback.
    pub strict_mode: StructuredOutputCapability,
}

pub enum AIStreamEvent {
    ContentDelta(String),
    ReasoningDelta(String),
}

#[derive(Debug, Clone)]
pub struct AIToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// W33: narrow provider abstraction the agent eval suite uses to drive
/// the chat loop without instantiating product `LocalMock`. Production
/// callers still hold `Arc<AIEngine>` directly — the trait exists so a
/// `RecordedProvider` in tests can replace the real network calls.
///
/// Streaming intentionally stays off the trait surface: the chat loop
/// uses generic closures for SSE event delivery and pushing that
/// through dyn dispatch would force an invasive refactor across the
/// whole 4.8k-line `chat.rs`. The non-streaming `complete` method is
/// the lowest-common-denominator entry point exercised by the
/// full-loop replay harness, and it is sufficient to cover the
/// validator/tool/cost gates that fail in real outages.
#[async_trait]
pub trait AIProvider: Send + Sync {
    async fn complete(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        structured_output: StructuredOutputCapability,
    ) -> Result<AIResponse>;
}

#[async_trait]
impl AIProvider for AIEngine {
    async fn complete(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        structured_output: StructuredOutputCapability,
    ) -> Result<AIResponse> {
        self.complete_with_structured_output(provider, messages, tools, structured_output)
            .await
    }
}

impl Default for AIEngine {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(600))
                .pool_idle_timeout(Duration::from_secs(15))
                .no_gzip()
                .no_brotli()
                .no_zstd()
                .no_deflate()
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }
}

impl AIEngine {
    pub async fn complete_chat(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
    ) -> Result<AIResponse> {
        self.complete_chat_with_tools(provider, messages, &[]).await
    }

    pub async fn complete_chat_with_tools(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
    ) -> Result<AIResponse> {
        self.complete_chat_with_tools_inner(provider, messages, tools, None)
            .await
    }

    /// W16: variant that asks OpenAI-compatible providers for a strict
    /// JSON object response via `response_format: {"type":"json_object"}`.
    /// Used on the proposal validation retry pass where we need a clean
    /// `BuildProposal` JSON, not free-form text wrapping a JSON blob.
    /// W33: routes through the capability map so unknown / non-strict
    /// providers fall back to plain text *visibly* (the resolved
    /// `AIResponse::strict_mode` flag tells the caller whether strict
    /// was actually applied), instead of shipping a body the provider
    /// silently ignores.
    pub async fn complete_chat_with_tools_json_object(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
    ) -> Result<AIResponse> {
        self.complete_with_structured_output(
            provider,
            messages,
            tools,
            StructuredOutputCapability::JsonObject,
        )
        .await
    }

    /// W33: shared inner that resolves the *requested* structured-output
    /// mode against the provider/model capability map and downgrades to
    /// `PlainText` when the model is not on the strict allowlist. The
    /// returned `AIResponse::strict_mode` reports what was actually
    /// applied, not what was requested.
    pub async fn complete_with_structured_output(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        requested: StructuredOutputCapability,
    ) -> Result<AIResponse> {
        let resolved = match requested {
            StructuredOutputCapability::PlainText => StructuredOutputCapability::PlainText,
            StructuredOutputCapability::JsonObject => {
                supports_structured_output(provider.kind, &provider.default_model)
            }
        };
        let response_format = match resolved {
            StructuredOutputCapability::JsonObject => Some(json!({"type": "json_object"})),
            StructuredOutputCapability::PlainText => None,
        };
        if requested == StructuredOutputCapability::JsonObject && !resolved.is_strict() {
            tracing::info!(
                provider_kind = ?provider.kind,
                model = %provider.default_model,
                "structured_output_fallback: requested json_object, provider not on strict allowlist — falling back to plain text"
            );
        }
        let mut response = self
            .complete_chat_with_tools_inner(provider, messages, tools, response_format)
            .await?;
        response.strict_mode = resolved;
        Ok(response)
    }

    async fn complete_chat_with_tools_inner(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        response_format: Option<serde_json::Value>,
    ) -> Result<AIResponse> {
        validate_provider(provider)?;

        let started = Instant::now();
        let completion = match provider.kind {
            ProviderKind::Ollama => self.complete_ollama(provider, messages).await?,
            ProviderKind::Openrouter | ProviderKind::Custom => {
                self.complete_openai_compatible(provider, messages, tools, response_format)
                    .await?
            }
        };

        Ok(AIResponse {
            content: completion.content,
            provider_id: provider.id.clone(),
            model: provider.default_model.clone(),
            tokens: completion.tokens,
            latency_ms: started.elapsed().as_millis() as u64,
            tool_calls: completion.tool_calls,
            reasoning: completion.reasoning,
            strict_mode: StructuredOutputCapability::PlainText,
        })
    }

    pub async fn complete_chat_with_tools_streaming<F, C>(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        mut on_event: F,
        is_cancelled: C,
    ) -> Result<AIResponse>
    where
        F: FnMut(AIStreamEvent),
        C: Fn() -> bool,
    {
        validate_provider(provider)?;

        let started = Instant::now();
        let completion = match provider.kind {
            ProviderKind::Openrouter | ProviderKind::Custom => {
                self.complete_openai_compatible_streaming(
                    provider,
                    messages,
                    tools,
                    &mut on_event,
                    is_cancelled,
                )
                .await?
            }
            ProviderKind::Ollama => {
                let completion = self.complete_ollama(provider, messages).await?;
                if !completion.content.is_empty() {
                    on_event(AIStreamEvent::ContentDelta(completion.content.clone()));
                }
                completion
            }
        };

        Ok(AIResponse {
            content: completion.content,
            provider_id: provider.id.clone(),
            model: provider.default_model.clone(),
            tokens: completion.tokens,
            latency_ms: started.elapsed().as_millis() as u64,
            tool_calls: completion.tool_calls,
            reasoning: completion.reasoning,
            strict_mode: StructuredOutputCapability::PlainText,
        })
    }

    pub async fn test_provider(&self, provider: &LLMProvider) -> ProviderTestResult {
        let checked_at = chrono::Utc::now().timestamp_millis();
        let invalid = validate_provider(provider)
            .err()
            .map(|error| ProviderTestResult {
                status: ProviderRuntimeStatus::InvalidConfig,
                provider_id: provider.id.clone(),
                provider: provider.name.clone(),
                model: provider.default_model.clone(),
                error: Some(error.to_string()),
                checked_at,
            });

        if let Some(result) = invalid {
            return result;
        }

        let result = match provider.kind {
            ProviderKind::Ollama => self.test_ollama(provider).await,
            ProviderKind::Openrouter | ProviderKind::Custom => {
                self.test_openai_compatible(provider).await
            }
        };

        match result {
            Ok(()) => ProviderTestResult {
                status: ProviderRuntimeStatus::Ok,
                provider_id: provider.id.clone(),
                provider: provider.name.clone(),
                model: provider.default_model.clone(),
                error: None,
                checked_at,
            },
            Err(error) => ProviderTestResult {
                status: ProviderRuntimeStatus::Unavailable,
                provider_id: provider.id.clone(),
                provider: provider.name.clone(),
                model: provider.default_model.clone(),
                error: Some(error.to_string()),
                checked_at,
            },
        }
    }

    async fn complete_openai_compatible(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        response_format: Option<serde_json::Value>,
    ) -> Result<AICompletion> {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut payload = json!({
            "model": provider.default_model,
            "stream": false,
            "messages": to_openai_messages(messages),
        });
        apply_openrouter_options(provider, &mut payload);

        if let Some(format) = response_format {
            payload["response_format"] = format;
        }

        if !tools.is_empty() {
            payload["tools"] = json!(tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>());
            payload["tool_choice"] = json!("auto");
        }

        let mut request = self
            .client
            .post(endpoint)
            .header(header::ACCEPT_ENCODING, "identity")
            .json(&payload);

        if matches!(provider.kind, ProviderKind::Openrouter) {
            request = request
                .header("HTTP-Referer", "https://github.com/datrina/datrina")
                .header("X-Title", "Datrina");
        }

        if let Some(api_key) = provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| anyhow!("provider_network_error: {}", e))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| anyhow!("provider_body_error status={}: {}", status, e))?;
        if !status.is_success() {
            return Err(anyhow!(
                "provider_http_error status={}: {}",
                status,
                truncate(&body)
            ));
        }

        let parsed: OpenAIChatResponse =
            serde_json::from_str(&body).map_err(|e| anyhow!("provider_parse_error: {}", e))?;
        let message = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider_empty_response: missing assistant message"))?
            .message;
        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .filter_map(|call| {
                let args = serde_json::from_str(&call.function.arguments).ok()?;
                Some(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments: args,
                })
            })
            .collect::<Vec<_>>();
        let content = message.content.unwrap_or_default();
        if content.trim().is_empty() && tool_calls.is_empty() {
            return Err(anyhow!(
                "provider_empty_response: missing assistant content and tool calls"
            ));
        }
        Ok(AICompletion {
            content,
            tokens: parsed.usage.map(token_usage_from_openai),
            tool_calls,
            reasoning: message
                .reasoning
                .or(message.reasoning_content)
                .or_else(|| reasoning_details_text(message.reasoning_details.as_deref())),
        })
    }

    async fn complete_openai_compatible_streaming<F, C>(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
        tools: &[AIToolSpec],
        on_event: &mut F,
        is_cancelled: C,
    ) -> Result<AICompletion>
    where
        F: FnMut(AIStreamEvent),
        C: Fn() -> bool,
    {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut payload = json!({
            "model": provider.default_model,
            "stream": true,
            "stream_options": { "include_usage": true },
            "messages": to_openai_messages(messages),
        });
        apply_openrouter_options(provider, &mut payload);

        if !tools.is_empty() {
            payload["tools"] = json!(tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>());
            payload["tool_choice"] = json!("auto");
        }

        let mut request = self
            .client
            .post(endpoint)
            .header(header::ACCEPT_ENCODING, "identity")
            .json(&payload);

        if matches!(provider.kind, ProviderKind::Openrouter) {
            request = request
                .header("HTTP-Referer", "https://github.com/datrina/datrina")
                .header("X-Title", "Datrina");
        }

        if let Some(api_key) = provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| anyhow!("provider_network_error: {}", e))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| anyhow!("provider_body_error status={}: {}", status, e))?;
            return Err(anyhow!(
                "provider_http_error status={}: {}",
                status,
                truncate(&body)
            ));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_builders: BTreeMap<u32, ToolCallBuilder> = BTreeMap::new();
        let mut tokens = None;
        let mut first_chunk_received = false;
        const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(60);

        loop {
            if is_cancelled() {
                return Err(anyhow!("chat_stream_cancelled"));
            }
            let next_chunk = if first_chunk_received {
                stream.next().await
            } else {
                match tokio::time::timeout(FIRST_BYTE_TIMEOUT, stream.next()).await {
                    Ok(value) => value,
                    Err(_) => {
                        return Err(anyhow!(
                            "provider_first_byte_timeout: provider sent no data within {}s",
                            FIRST_BYTE_TIMEOUT.as_secs()
                        ));
                    }
                }
            };
            let Some(chunk) = next_chunk else {
                break;
            };
            let chunk = chunk.map_err(|e| anyhow!("provider_stream_error: {}", e))?;
            first_chunk_received = true;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some((event_end, separator_len)) = find_sse_event_end(&buffer) {
                let raw_event = buffer[..event_end].to_string();
                buffer = buffer[event_end + separator_len..].to_string();
                for data in sse_data_lines(&raw_event) {
                    if data == "[DONE]" {
                        continue;
                    }
                    process_openai_stream_data(
                        &data,
                        &mut tokens,
                        &mut content,
                        &mut reasoning,
                        &mut tool_builders,
                        on_event,
                    )?;
                }
            }
        }

        if !buffer.trim().is_empty() {
            for data in sse_data_lines(&buffer) {
                if data == "[DONE]" {
                    continue;
                }
                process_openai_stream_data(
                    &data,
                    &mut tokens,
                    &mut content,
                    &mut reasoning,
                    &mut tool_builders,
                    on_event,
                )?;
            }
        }

        if is_cancelled() {
            return Err(anyhow!("chat_stream_cancelled"));
        }

        let tool_calls = tool_builders
            .into_iter()
            .filter_map(|(index, builder)| builder.build(index))
            .collect::<Vec<_>>();

        if content.trim().is_empty() && tool_calls.is_empty() {
            return Err(anyhow!(
                "provider_empty_response: missing assistant content and tool calls"
            ));
        }

        Ok(AICompletion {
            content,
            tokens,
            tool_calls,
            reasoning: if reasoning.trim().is_empty() {
                None
            } else {
                Some(reasoning)
            },
        })
    }

    async fn complete_ollama(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
    ) -> Result<AICompletion> {
        let endpoint = join_url(&provider.base_url, "/api/chat")?;
        let response = self
            .client
            .post(endpoint)
            .header(header::ACCEPT_ENCODING, "identity")
            .json(&json!({
                "model": provider.default_model,
                "stream": false,
                "messages": to_ollama_messages(messages),
            }))
            .send()
            .await
            .map_err(|e| anyhow!("provider_network_error: {}", e))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| anyhow!("provider_body_error status={}: {}", status, e))?;
        if !status.is_success() {
            return Err(anyhow!(
                "provider_http_error status={}: {}",
                status,
                truncate(&body)
            ));
        }

        let parsed: OllamaChatResponse =
            serde_json::from_str(&body).map_err(|e| anyhow!("provider_parse_error: {}", e))?;
        if parsed.message.content.trim().is_empty() {
            return Err(anyhow!(
                "provider_empty_response: missing assistant content"
            ));
        }
        Ok(AICompletion {
            content: parsed.message.content,
            tokens: None,
            tool_calls: vec![],
            reasoning: None,
        })
    }

    async fn test_openai_compatible(&self, provider: &LLMProvider) -> Result<()> {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut request = self
            .client
            .post(endpoint)
            .header(header::ACCEPT_ENCODING, "identity")
            .json(&json!({
                "model": provider.default_model,
                "stream": false,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
            }));

        if matches!(provider.kind, ProviderKind::Openrouter) {
            request = request
                .header("HTTP-Referer", "https://github.com/datrina/datrina")
                .header("X-Title", "Datrina");
        }

        if let Some(api_key) = provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| anyhow!("provider_network_error: {}", e))?;
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| anyhow!("provider_body_error status={}: {}", status, e))?;
            Err(anyhow!(
                "provider_http_error status={}: {}",
                status,
                truncate(&body)
            ))
        }
    }

    async fn test_ollama(&self, provider: &LLMProvider) -> Result<()> {
        let endpoint = join_url(&provider.base_url, "/api/tags")?;
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|e| anyhow!("provider_network_error: {}", e))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("provider_http_error status={}", response.status()))
        }
    }
}

struct AICompletion {
    content: String,
    tokens: Option<crate::models::chat::TokenUsage>,
    tool_calls: Vec<ToolCall>,
    reasoning: Option<String>,
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl ToolCallBuilder {
    fn build(self, index: u32) -> Option<ToolCall> {
        if self.name.trim().is_empty() {
            return None;
        }
        let arguments = serde_json::from_str(&self.arguments).unwrap_or_else(|_| json!({}));
        Some(ToolCall {
            id: self.id.unwrap_or_else(|| format!("tool-call-{index}")),
            name: self.name,
            arguments,
        })
    }
}

pub fn validate_provider(provider: &LLMProvider) -> Result<()> {
    if provider.is_unsupported {
        return Err(anyhow!(
            "provider has an unsupported legacy kind — replace with OpenRouter, Ollama, or a custom OpenAI-compatible provider"
        ));
    }
    if !provider.is_enabled {
        return Err(anyhow!("provider is disabled"));
    }
    if provider.name.trim().is_empty() {
        return Err(anyhow!("provider name is required"));
    }
    if provider.default_model.trim().is_empty() {
        return Err(anyhow!("default_model is required"));
    }

    match provider.kind {
        ProviderKind::Ollama => {
            validate_base_url(&provider.base_url)?;
            Ok(())
        }
        ProviderKind::Openrouter => {
            validate_base_url(&provider.base_url)?;
            require_api_key(provider)
        }
        ProviderKind::Custom => {
            validate_base_url(&provider.base_url)?;
            Ok(())
        }
    }
}

/// W43: resolve which model a single widget run should use.
///
/// Order of precedence:
/// 1. widget `model_override` (if set);
/// 2. dashboard `model_policy` (if set);
/// 3. caller-supplied `app_active_provider` fallback (None → no LLM).
///
/// `providers` is the full list of stored providers; we re-look up by id
/// rather than passing them in so callers don't have to pre-fetch the
/// matching row. The api key for the *fallback* provider is taken from
/// the live row exactly as the chat path does, so credentials stay
/// Rust-owned and the widget JSON never carries one.
///
/// Returns:
/// - `Ok(Some(model))` — widget should run with this LLM selection.
/// - `Ok(None)` — no policy at any level; caller's existing behaviour
///   (deterministic-only / app-fallback) stands.
/// - `Err(WidgetModelError)` — typed remediation, surfaced unchanged
///   through the refresh command result.
pub fn resolve_effective_widget_model(
    widget: &Widget,
    dashboard: &Dashboard,
    providers: &[LLMProvider],
    app_active_provider: Option<&LLMProvider>,
) -> std::result::Result<Option<EffectiveWidgetModel>, WidgetModelError> {
    use crate::models::widget::DatasourceConfig;

    fn widget_override(w: &Widget) -> Option<&crate::models::widget::WidgetModelOverride> {
        widget_datasource_config(w).and_then(|ds: &DatasourceConfig| ds.model_override.as_ref())
    }

    if let Some(override_policy) = widget_override(widget) {
        return resolve_policy(
            &override_policy.provider_id,
            &override_policy.model,
            &override_policy.required_caps,
            providers,
            WidgetModelSource::WidgetOverride,
        )
        .map(Some);
    }

    if let Some(default_policy) = dashboard.model_policy.as_ref() {
        return resolve_policy(
            &default_policy.provider_id,
            &default_policy.model,
            &default_policy.required_caps,
            providers,
            WidgetModelSource::DashboardDefault,
        )
        .map(Some);
    }

    Ok(app_active_provider.map(|provider| EffectiveWidgetModel {
        provider: provider.clone(),
        model: provider.default_model.clone(),
        source: WidgetModelSource::AppActiveProvider,
        required_caps: Vec::new(),
    }))
}

fn resolve_policy(
    provider_id: &str,
    model: &str,
    required_caps: &[WidgetCapability],
    providers: &[LLMProvider],
    source: WidgetModelSource,
) -> std::result::Result<EffectiveWidgetModel, WidgetModelError> {
    let provider = providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| WidgetModelError::ProviderMissing {
            provider_id: provider_id.to_string(),
            source,
        })?;
    if provider.is_unsupported || !provider.is_enabled {
        return Err(WidgetModelError::ProviderDisabled {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            source,
        });
    }
    if let Err(error) = validate_provider(&provider) {
        return Err(WidgetModelError::ProviderInvalidConfig {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            source,
            reason: error.to_string(),
        });
    }
    let resolved_model = if model.trim().is_empty() {
        provider.default_model.clone()
    } else {
        model.to_string()
    };
    let available = provider_capabilities(provider.kind, &resolved_model);
    let missing: Vec<WidgetCapability> = required_caps
        .iter()
        .copied()
        .filter(|cap| !available.contains(cap))
        .collect();
    if !missing.is_empty() {
        return Err(WidgetModelError::CapabilityUnsupported {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            model: resolved_model,
            source,
            missing,
        });
    }
    Ok(EffectiveWidgetModel {
        provider,
        model: resolved_model,
        source,
        required_caps: required_caps.to_vec(),
    })
}

/// W43: pull the [`crate::models::widget::DatasourceConfig`] off any
/// widget shape. Mirrors the existing `widget.datasource()` style helper
/// in `commands::datasource`, kept here so the resolution helper stays
/// self-contained.
fn widget_datasource_config(widget: &Widget) -> Option<&crate::models::widget::DatasourceConfig> {
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

/// W43: apply the [`EffectiveWidgetModel`] to a clone of the provider
/// so the workflow engine/pipeline sees the override model id without
/// mutating the stored provider row. Other fields (api key, base url)
/// flow through unchanged.
pub fn provider_with_model(model: &EffectiveWidgetModel) -> LLMProvider {
    let mut provider = model.provider.clone();
    provider.default_model = model.model.clone();
    provider
}

fn require_api_key(provider: &LLMProvider) -> Result<()> {
    match provider
        .api_key
        .as_ref()
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
    {
        Some(_) => Ok(()),
        None => Err(anyhow!("api_key is required for this provider kind")),
    }
}

fn validate_base_url(base_url: &str) -> Result<()> {
    let url = reqwest::Url::parse(base_url.trim())?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(anyhow!("unsupported URL scheme: {}", scheme)),
    }
}

fn openai_chat_endpoint(base_url: &str) -> Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    validate_base_url(trimmed)?;
    if trimmed.ends_with("/chat/completions") {
        Ok(trimmed.to_string())
    } else if trimmed.ends_with("/v1") {
        Ok(format!("{}/chat/completions", trimmed))
    } else {
        Ok(format!("{}/v1/chat/completions", trimmed))
    }
}

fn join_url(base_url: &str, path: &str) -> Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    validate_base_url(trimmed)?;
    Ok(format!("{}{}", trimmed, path))
}

fn apply_openrouter_options(provider: &LLMProvider, payload: &mut serde_json::Value) {
    if matches!(provider.kind, ProviderKind::Openrouter) {
        payload["reasoning"] = json!({
            "enabled": true,
            "exclude": false,
        });
        // W49: ask OpenRouter to include the upstream billing cost in the
        // `usage` block so cost accounting can prefer the provider's own
        // figure over the local pricing-table estimate.
        payload["usage"] = json!({ "include": true });
    }
}

fn to_openai_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    let mut result = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::User => result.push(json!({"role": "user", "content": message.content})),
            MessageRole::Assistant => {
                if let Some(tool_calls) = &message.tool_calls {
                    let mut value = json!({"role": "assistant", "content": null});
                    if let Some(reasoning) = message
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.reasoning.as_ref())
                        .filter(|reasoning| !reasoning.trim().is_empty())
                    {
                        value["reasoning"] = json!(reasoning);
                    }
                    value["tool_calls"] = json!(tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                    result.push(value);
                } else {
                    let mut value = json!({"role": "assistant", "content": message.content});
                    if let Some(reasoning) = message
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.reasoning.as_ref())
                        .filter(|reasoning| !reasoning.trim().is_empty())
                    {
                        value["reasoning"] = json!(reasoning);
                    }
                    result.push(value);
                }
            }
            MessageRole::System => {
                result.push(json!({"role": "system", "content": message.content}))
            }
            MessageRole::Tool => {
                if let Some(tool_results) = &message.tool_results {
                    for tool_result in tool_results {
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_result.tool_call_id,
                            "name": tool_result.name,
                            "content": serde_json::to_string(&tool_result_for_provider(tool_result)).unwrap_or_else(|_| message.content.clone()),
                        }));
                    }
                }
            }
        }
    }
    result
}

fn to_ollama_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    to_openai_messages(messages)
}

/// W51: provider-visible rendering of a `ToolResult`. When the
/// callsite already attached `compression` metadata, the `result`
/// payload is the compressor's compact value — we wrap it with a
/// small `_compression` envelope so the model sees the raw artifact id
/// (and can call `inspect_artifact` for bounded detail). Legacy
/// callsites without compression fall through the same
/// `context_compressor` so the provider never receives an unbounded
/// blob through this function.
fn tool_result_for_provider(tool_result: &crate::models::chat::ToolResult) -> serde_json::Value {
    use crate::modules::context_compressor::{compress, CompressionProfile};

    let result_value = if let Some(meta) = tool_result.compression.as_ref() {
        let mut envelope = serde_json::Map::new();
        envelope.insert(
            "_compression".to_string(),
            json!({
                "profile": meta.profile,
                "raw_bytes": meta.raw_bytes,
                "compact_bytes": meta.compact_bytes,
                "estimated_tokens_saved": meta.estimated_tokens_saved,
                "raw_artifact_id": meta.raw_artifact_id,
                "truncation_paths": meta.truncation_paths,
            }),
        );
        envelope.insert("compact".to_string(), tool_result.result.clone());
        if meta.raw_artifact_id.is_some() {
            envelope.insert(
                "_hint".to_string(),
                json!("call inspect_artifact(artifact_id, path?) to request bounded raw detail."),
            );
        }
        serde_json::Value::Object(envelope)
    } else {
        // Legacy/non-compressed callsite: run the same compressor so
        // we never ship a raw multi-KB blob through this function.
        let profile = CompressionProfile::for_tool(&tool_result.name);
        compress(profile, &tool_result.result).provider_payload()
    };
    json!({
        "tool_call_id": tool_result.tool_call_id,
        "name": tool_result.name,
        "result": result_value,
        "error": tool_result.error,
    })
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 400;
    if value.len() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", &value[..LIMIT])
    }
}

fn sse_data_lines(raw_event: &str) -> Vec<String> {
    raw_event
        .lines()
        .map(str::trim_end)
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn find_sse_event_end(buffer: &str) -> Option<(usize, usize)> {
    let lf = buffer.find("\n\n").map(|index| (index, 2));
    let crlf = buffer.find("\r\n\r\n").map(|index| (index, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
}

fn process_openai_stream_data<F>(
    data: &str,
    tokens: &mut Option<crate::models::chat::TokenUsage>,
    content: &mut String,
    reasoning: &mut String,
    tool_builders: &mut BTreeMap<u32, ToolCallBuilder>,
    on_event: &mut F,
) -> Result<()>
where
    F: FnMut(AIStreamEvent),
{
    let parsed: OpenAIStreamResponse =
        serde_json::from_str(data).map_err(|e| anyhow!("provider_stream_parse_error: {}", e))?;

    if let Some(error) = parsed.error {
        return Err(anyhow!(
            "provider_stream_error code={}: {}",
            error
                .code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            error.message
        ));
    }

    if let Some(usage) = parsed.usage {
        *tokens = Some(token_usage_from_openai(usage));
    }

    for choice in parsed.choices.unwrap_or_default() {
        let delta = choice.delta;
        if let Some(part) = delta.content.filter(|part| !part.is_empty()) {
            content.push_str(&part);
            on_event(AIStreamEvent::ContentDelta(part));
        }
        let visible_reasoning = delta
            .reasoning
            .or(delta.reasoning_content)
            .or_else(|| reasoning_details_text(delta.reasoning_details.as_deref()));
        if let Some(part) = visible_reasoning.filter(|part| !part.is_empty()) {
            reasoning.push_str(&part);
            on_event(AIStreamEvent::ReasoningDelta(part));
        }
        for tool_delta in delta.tool_calls.unwrap_or_default() {
            let builder = tool_builders.entry(tool_delta.index).or_default();
            if let Some(id) = tool_delta.id {
                builder.id = Some(id);
            }
            if let Some(function) = tool_delta.function {
                if let Some(name) = function.name {
                    builder.name.push_str(&name);
                }
                if let Some(arguments) = function.arguments {
                    builder.arguments.push_str(&arguments);
                }
            }
        }
    }

    Ok(())
}

fn reasoning_details_text(details: Option<&[OpenAIReasoningDetail]>) -> Option<String> {
    let text = details?
        .iter()
        .filter_map(|detail| detail.text.as_ref().or(detail.summary.as_ref()))
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[derive(Deserialize)]
struct OpenAIChatResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    reasoning_details: Option<Vec<OpenAIReasoningDetail>>,
}

#[derive(Deserialize)]
struct OpenAIToolCall {
    id: String,
    function: OpenAIFunctionCall,
}

#[derive(Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAIUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    /// OpenRouter / OpenAI o-series sometimes reports reasoning tokens
    /// inside `completion_tokens_details.reasoning_tokens`. A handful of
    /// providers also surface a flat `reasoning_tokens` field.
    #[serde(default)]
    reasoning_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAICompletionTokensDetails>,
    /// W49: OpenRouter's `usage.cost` (USD, present when
    /// `usage.include = ["cost"]`). When provided, accounting uses it
    /// verbatim and skips the pricing-table fallback.
    #[serde(default)]
    cost: Option<f64>,
    /// Some upstream proxies use `total_cost` instead of `cost`. Accept
    /// either spelling.
    #[serde(default)]
    total_cost: Option<f64>,
}

#[derive(Deserialize)]
struct OpenAICompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

fn token_usage_from_openai(usage: OpenAIUsage) -> crate::models::chat::TokenUsage {
    let reasoning = usage.reasoning_tokens.or_else(|| {
        usage
            .completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens)
    });
    let provider_cost_usd = usage.cost.or(usage.total_cost).filter(|c| c.is_finite());
    crate::models::chat::TokenUsage {
        prompt: usage.prompt_tokens.unwrap_or(0),
        completion: usage.completion_tokens.unwrap_or(0),
        reasoning,
        provider_cost_usd,
    }
}

#[derive(Deserialize)]
struct OpenAIStreamResponse {
    choices: Option<Vec<OpenAIStreamChoice>>,
    usage: Option<OpenAIUsage>,
    error: Option<OpenAIStreamError>,
}

#[derive(Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
}

#[derive(Deserialize)]
struct OpenAIStreamDelta {
    content: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    reasoning_details: Option<Vec<OpenAIReasoningDetail>>,
    tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

#[derive(Deserialize)]
struct OpenAIReasoningDetail {
    text: Option<String>,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIStreamError {
    code: Option<serde_json::Value>,
    message: String,
}

#[derive(Deserialize)]
struct OpenAIStreamToolCall {
    index: u32,
    id: Option<String>,
    function: Option<OpenAIStreamFunctionCall>,
}

#[derive(Deserialize)]
struct OpenAIStreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaMessage,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(kind: ProviderKind, api_key: Option<&str>) -> LLMProvider {
        LLMProvider {
            id: "provider-1".into(),
            name: "Provider".into(),
            kind,
            base_url: "http://localhost:11434".into(),
            api_key: api_key.map(str::to_string),
            default_model: "model".into(),
            models: vec!["model".into()],
            is_enabled: true,
            is_unsupported: false,
        }
    }

    #[test]
    fn openrouter_requires_api_key() {
        let err = validate_provider(&provider(ProviderKind::Openrouter, None)).unwrap_err();
        assert!(err.to_string().contains("api_key"));
    }

    #[test]
    fn unsupported_provider_is_rejected_even_when_enabled() {
        let mut p = provider(ProviderKind::Ollama, None);
        p.is_unsupported = true;
        let err = validate_provider(&p).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn openai_endpoint_accepts_v1_or_exact_chat_url() {
        assert_eq!(
            openai_chat_endpoint("https://openrouter.ai/api/v1").unwrap(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            openai_chat_endpoint("https://example.test/v1/chat/completions").unwrap(),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn sse_event_end_accepts_lf_and_crlf() {
        assert_eq!(find_sse_event_end("data: {}\n\n"), Some((8, 2)));
        assert_eq!(find_sse_event_end("data: {}\r\n\r\n"), Some((8, 4)));
    }

    #[test]
    fn sse_data_lines_accept_crlf_events() {
        assert_eq!(
            sse_data_lines("event: message\r\ndata: {\"ok\":true}\r\n"),
            vec!["{\"ok\":true}".to_string()]
        );
    }

    #[test]
    fn stream_data_accepts_openrouter_reasoning_details() {
        let mut tokens = None;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_builders = BTreeMap::new();
        let mut events = Vec::new();

        process_openai_stream_data(
            r#"{"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.text","text":"thinking..."}]}}]}"#,
            &mut tokens,
            &mut content,
            &mut reasoning,
            &mut tool_builders,
            &mut |event| events.push(event),
        )
        .unwrap();

        assert_eq!(reasoning, "thinking...");
        assert!(matches!(
            events.first(),
            Some(AIStreamEvent::ReasoningDelta(value)) if value == "thinking..."
        ));
    }

    #[test]
    fn stream_data_reports_openrouter_top_level_error() {
        let mut tokens = None;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_builders = BTreeMap::new();

        let error = process_openai_stream_data(
            r#"{"error":{"code":"server_error","message":"Provider disconnected"},"choices":[{"delta":{"content":""},"finish_reason":"error"}]}"#,
            &mut tokens,
            &mut content,
            &mut reasoning,
            &mut tool_builders,
            &mut |_| {},
        )
        .unwrap_err();

        assert!(error.to_string().contains("Provider disconnected"));
    }

    #[test]
    fn stream_usage_chunk_yields_token_counts_including_reasoning() {
        let mut tokens = None;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_builders = BTreeMap::new();

        process_openai_stream_data(
            r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":80,"completion_tokens_details":{"reasoning_tokens":30}}}"#,
            &mut tokens,
            &mut content,
            &mut reasoning,
            &mut tool_builders,
            &mut |_| {},
        )
        .unwrap();

        let parsed = tokens.expect("usage parsed");
        assert_eq!(parsed.prompt, 120);
        assert_eq!(parsed.completion, 80);
        assert_eq!(parsed.reasoning, Some(30));
        assert!(parsed.provider_cost_usd.is_none());
    }

    #[test]
    fn stream_usage_chunk_captures_openrouter_total_cost() {
        // W49: when OpenRouter returns its own billing line, surface it
        // on the parsed `TokenUsage` so accounting can prefer it over
        // the local pricing-table fallback.
        let mut tokens = None;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_builders = BTreeMap::new();

        process_openai_stream_data(
            r#"{"choices":[],"usage":{"prompt_tokens":12000,"completion_tokens":800,"cost":0.0123}}"#,
            &mut tokens,
            &mut content,
            &mut reasoning,
            &mut tool_builders,
            &mut |_| {},
        )
        .unwrap();

        let parsed = tokens.expect("usage parsed");
        assert_eq!(parsed.prompt, 12000);
        assert_eq!(parsed.completion, 800);
        assert!((parsed.provider_cost_usd.unwrap() - 0.0123).abs() < 1e-9);
    }

    fn empty_dashboard() -> Dashboard {
        Dashboard {
            id: "dash-1".into(),
            name: "Dash".into(),
            description: None,
            layout: Vec::new(),
            workflows: Vec::new(),
            is_default: false,
            created_at: 0,
            updated_at: 0,
            parameters: Vec::new(),
            model_policy: None,
            language_policy: None,
        }
    }

    fn text_widget_with_override(
        override_policy: Option<crate::models::widget::WidgetModelOverride>,
    ) -> Widget {
        use crate::models::widget::{DatasourceConfig, TextAlign, TextConfig, TextFormat};
        Widget::Text {
            id: "widget-1".into(),
            title: "w".into(),
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
                workflow_id: "wf".into(),
                output_key: "value".into(),
                post_process: None,
                capture_traces: false,
                datasource_definition_id: None,
                binding_source: None,
                bound_at: None,
                tail_pipeline: Vec::new(),
                model_override: override_policy,
            }),
        }
    }

    fn openrouter_with_id(id: &str, model: &str) -> LLMProvider {
        LLMProvider {
            id: id.into(),
            name: format!("Provider {id}"),
            kind: ProviderKind::Openrouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: Some("sk-test".into()),
            default_model: model.into(),
            models: vec![model.into()],
            is_enabled: true,
            is_unsupported: false,
        }
    }

    #[test]
    fn effective_model_prefers_widget_override_over_dashboard_default() {
        let providers = vec![
            openrouter_with_id("p-default", "openai/gpt-4o-mini"),
            openrouter_with_id("p-override", "anthropic/claude-opus-4"),
        ];
        let mut dashboard = empty_dashboard();
        dashboard.model_policy = Some(crate::models::dashboard::DashboardModelPolicy {
            provider_id: "p-default".into(),
            model: "openai/gpt-4o-mini".into(),
            required_caps: Vec::new(),
        });
        let widget = text_widget_with_override(Some(crate::models::widget::WidgetModelOverride {
            provider_id: "p-override".into(),
            model: "anthropic/claude-opus-4".into(),
            required_caps: Vec::new(),
        }));
        let resolved = resolve_effective_widget_model(&widget, &dashboard, &providers, None)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, WidgetModelSource::WidgetOverride);
        assert_eq!(resolved.model, "anthropic/claude-opus-4");
        assert_eq!(resolved.provider.id, "p-override");
    }

    #[test]
    fn effective_model_falls_back_to_dashboard_default_when_no_override() {
        let providers = vec![openrouter_with_id("p-default", "openai/gpt-4o-mini")];
        let mut dashboard = empty_dashboard();
        dashboard.model_policy = Some(crate::models::dashboard::DashboardModelPolicy {
            provider_id: "p-default".into(),
            model: "openai/gpt-4o-mini".into(),
            required_caps: Vec::new(),
        });
        let widget = text_widget_with_override(None);
        let resolved = resolve_effective_widget_model(&widget, &dashboard, &providers, None)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, WidgetModelSource::DashboardDefault);
        assert_eq!(resolved.model, "openai/gpt-4o-mini");
    }

    #[test]
    fn effective_model_uses_app_active_when_no_policy_at_all() {
        let providers = vec![openrouter_with_id("p-active", "openai/gpt-4o")];
        let dashboard = empty_dashboard();
        let widget = text_widget_with_override(None);
        let resolved =
            resolve_effective_widget_model(&widget, &dashboard, &providers, Some(&providers[0]))
                .unwrap()
                .unwrap();
        assert_eq!(resolved.source, WidgetModelSource::AppActiveProvider);
        assert_eq!(resolved.model, "openai/gpt-4o");
    }

    #[test]
    fn effective_model_returns_typed_error_when_capability_unsupported() {
        let providers = vec![openrouter_with_id("p-default", "vendor-x/private-model-v9")];
        let mut dashboard = empty_dashboard();
        dashboard.model_policy = Some(crate::models::dashboard::DashboardModelPolicy {
            provider_id: "p-default".into(),
            model: "vendor-x/private-model-v9".into(),
            required_caps: vec![WidgetCapability::StructuredJsonObject],
        });
        let widget = text_widget_with_override(None);
        let err =
            resolve_effective_widget_model(&widget, &dashboard, &providers, None).unwrap_err();
        match err {
            WidgetModelError::CapabilityUnsupported {
                missing,
                source,
                model,
                ..
            } => {
                assert_eq!(source, WidgetModelSource::DashboardDefault);
                assert_eq!(model, "vendor-x/private-model-v9");
                assert!(missing.contains(&WidgetCapability::StructuredJsonObject));
            }
            other => panic!("expected CapabilityUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn effective_model_typed_error_when_provider_missing() {
        let providers: Vec<LLMProvider> = Vec::new();
        let mut dashboard = empty_dashboard();
        dashboard.model_policy = Some(crate::models::dashboard::DashboardModelPolicy {
            provider_id: "p-missing".into(),
            model: "any".into(),
            required_caps: Vec::new(),
        });
        let widget = text_widget_with_override(None);
        let err =
            resolve_effective_widget_model(&widget, &dashboard, &providers, None).unwrap_err();
        assert!(matches!(err, WidgetModelError::ProviderMissing { .. }));
    }

    #[test]
    fn ollama_lacks_structured_and_streaming_caps() {
        let caps = provider_capabilities(ProviderKind::Ollama, "llama3.1:8b");
        assert!(!caps.contains(&WidgetCapability::Streaming));
        assert!(!caps.contains(&WidgetCapability::StructuredJsonObject));
        assert!(!caps.contains(&WidgetCapability::ToolCalling));
    }

    #[test]
    fn known_openrouter_alias_has_full_caps() {
        let caps = provider_capabilities(ProviderKind::Openrouter, "openai/gpt-4o-mini");
        assert!(caps.contains(&WidgetCapability::Streaming));
        assert!(caps.contains(&WidgetCapability::ToolCalling));
        assert!(caps.contains(&WidgetCapability::StructuredJsonObject));
    }

    #[test]
    fn widget_model_override_serializes_without_api_key() {
        let override_policy = crate::models::widget::WidgetModelOverride {
            provider_id: "p-1".into(),
            model: "anthropic/claude-opus-4".into(),
            required_caps: vec![WidgetCapability::StructuredJsonObject],
        };
        let value = serde_json::to_value(&override_policy).unwrap();
        let obj = value.as_object().expect("object");
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        assert!(keys.contains(&"provider_id"));
        assert!(keys.contains(&"model"));
        assert!(!keys.iter().any(|k| k.contains("api_key")));
        assert!(!keys.iter().any(|k| k.contains("secret")));
    }

    #[test]
    fn tool_result_for_provider_runs_compressor_on_legacy_callsites() {
        // W51: a callsite that hasn't yet attached typed compression
        // metadata still goes through `context_compressor::compress`,
        // which unwraps the MCP envelope and caps total provider-visible
        // chars per profile. The provider must never see the raw 5 KB
        // payload, even through the legacy path.
        let result = crate::models::chat::ToolResult {
            tool_call_id: "call-1".to_string(),
            name: "mcp_tool".to_string(),
            result: json!({
                "content": [{
                    "type": "text",
                    "text": "x".repeat(5000),
                }],
            }),
            error: None,
            compression: None,
        };

        let compact = tool_result_for_provider(&result);
        let encoded = serde_json::to_string(&compact).unwrap();
        assert!(encoded.contains("_compressed"));
        assert!(
            encoded.len() < 3_500,
            "expected compressor cap, got {}",
            encoded.len()
        );
    }
}
