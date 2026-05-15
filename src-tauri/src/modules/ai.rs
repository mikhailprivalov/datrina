use anyhow::{anyhow, Result};
use reqwest::{header, Client};
use serde::Deserialize;
use serde_json::json;
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
                .timeout(Duration::from_secs(45))
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

fn to_openai_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    let mut result = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::User => result.push(json!({"role": "user", "content": message.content})),
            MessageRole::Assistant => {
                let mut value = json!({"role": "assistant", "content": message.content});
                if let Some(tool_calls) = &message.tool_calls {
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
                }
                result.push(value);
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
                            "content": serde_json::to_string(tool_result).unwrap_or_else(|_| message.content.clone()),
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

fn truncate(value: &str) -> String {
    const LIMIT: usize = 400;
    if value.len() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", &value[..LIMIT])
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
}
