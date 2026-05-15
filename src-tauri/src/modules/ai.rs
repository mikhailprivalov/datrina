use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::{header, Client};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::models::chat::{ChatMessage, MessageRole, ToolCall};
use crate::models::provider::{
    LLMProvider, ProviderKind, ProviderRuntimeStatus, ProviderTestResult,
};

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

impl Default for AIEngine {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(180))
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
        validate_provider(provider)?;

        let started = Instant::now();
        let completion = match provider.kind {
            ProviderKind::LocalMock => AICompletion {
                content: local_mock_response(messages),
                tokens: Some(local_mock_tokens(messages)),
                tool_calls: vec![],
                reasoning: None,
            },
            ProviderKind::Ollama => self.complete_ollama(provider, messages).await?,
            ProviderKind::Openrouter | ProviderKind::Custom => {
                self.complete_openai_compatible(provider, messages, tools)
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
            ProviderKind::LocalMock => {
                let content = local_mock_response(messages);
                on_event(AIStreamEvent::ContentDelta(content.clone()));
                AICompletion {
                    content,
                    tokens: Some(local_mock_tokens(messages)),
                    tool_calls: vec![],
                    reasoning: None,
                }
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
            ProviderKind::LocalMock => Ok(()),
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
    ) -> Result<AICompletion> {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut payload = json!({
            "model": provider.default_model,
            "stream": false,
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
            tokens: parsed.usage.map(|usage| crate::models::chat::TokenUsage {
                prompt: usage.prompt_tokens.unwrap_or(0),
                completion: usage.completion_tokens.unwrap_or(0),
            }),
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

        while let Some(chunk) = stream.next().await {
            if is_cancelled() {
                return Err(anyhow!("chat_stream_cancelled"));
            }
            let chunk = chunk.map_err(|e| anyhow!("provider_stream_error: {}", e))?;
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
        ProviderKind::LocalMock => Ok(()),
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

fn local_mock_response(messages: &[ChatMessage]) -> String {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, MessageRole::User))
        .map(|message| message.content.trim())
        .unwrap_or("");

    format!(
        "Local mock AI response. Received {} user characters. Build changes require the visible Apply controls, and tool execution stays behind Rust policy gates.",
        latest_user.chars().count()
    )
}

fn local_mock_tokens(messages: &[ChatMessage]) -> crate::models::chat::TokenUsage {
    let prompt_chars = messages
        .iter()
        .map(|message| message.content.len())
        .sum::<usize>();
    crate::models::chat::TokenUsage {
        prompt: (prompt_chars / 4).max(1) as u32,
        completion: 24,
    }
}

fn tool_result_for_provider(tool_result: &crate::models::chat::ToolResult) -> serde_json::Value {
    json!({
        "tool_call_id": tool_result.tool_call_id,
        "name": tool_result.name,
        "result": compact_json_for_provider(&tool_result.result, 0),
        "error": tool_result.error,
    })
}

fn compact_json_for_provider(value: &serde_json::Value, depth: usize) -> serde_json::Value {
    const MAX_STRING_CHARS: usize = 4000;
    const MAX_ARRAY_ITEMS: usize = 20;
    const MAX_OBJECT_KEYS: usize = 40;
    const MAX_DEPTH: usize = 5;

    if depth >= MAX_DEPTH {
        return json!({"_truncated": true, "reason": "max_depth"});
    }

    match value {
        serde_json::Value::String(text) => {
            if text.chars().count() > MAX_STRING_CHARS {
                json!({
                    "_truncated": true,
                    "text": text.chars().take(MAX_STRING_CHARS).collect::<String>(),
                    "original_chars": text.chars().count(),
                })
            } else {
                value.clone()
            }
        }
        serde_json::Value::Array(items) => {
            let mut compact = items
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|item| compact_json_for_provider(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > MAX_ARRAY_ITEMS {
                compact.push(json!({
                    "_truncated": true,
                    "remaining_items": items.len() - MAX_ARRAY_ITEMS,
                }));
            }
            serde_json::Value::Array(compact)
        }
        serde_json::Value::Object(map) => {
            let mut compact = serde_json::Map::new();
            for (index, (key, item)) in map.iter().enumerate() {
                if index >= MAX_OBJECT_KEYS {
                    compact.insert(
                        "_truncated".to_string(),
                        json!({
                            "remaining_keys": map.len() - MAX_OBJECT_KEYS,
                        }),
                    );
                    break;
                }
                compact.insert(key.clone(), compact_json_for_provider(item, depth + 1));
            }
            serde_json::Value::Object(compact)
        }
        _ => value.clone(),
    }
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
        *tokens = Some(crate::models::chat::TokenUsage {
            prompt: usage.prompt_tokens.unwrap_or(0),
            completion: usage.completion_tokens.unwrap_or(0),
        });
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
        }
    }

    #[test]
    fn openrouter_requires_api_key() {
        let err = validate_provider(&provider(ProviderKind::Openrouter, None)).unwrap_err();
        assert!(err.to_string().contains("api_key"));
    }

    #[test]
    fn local_mock_does_not_require_base_url_or_key() {
        let mut provider = provider(ProviderKind::LocalMock, None);
        provider.base_url.clear();
        assert!(validate_provider(&provider).is_ok());
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
    fn tool_result_for_provider_truncates_large_results() {
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
        };

        let compact = tool_result_for_provider(&result);
        let text = compact["result"]["content"][0]["text"]["text"]
            .as_str()
            .unwrap();
        assert_eq!(text.chars().count(), 4000);
        assert_eq!(
            compact["result"]["content"][0]["text"]["original_chars"],
            5000
        );
    }
}
