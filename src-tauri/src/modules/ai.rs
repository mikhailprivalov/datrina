use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::models::chat::{ChatMessage, MessageRole};
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
}

impl Default for AIEngine {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl AIEngine {
    pub async fn complete_chat(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
    ) -> Result<AIResponse> {
        validate_provider(provider)?;

        let content = match provider.kind {
            ProviderKind::LocalMock => local_mock_response(messages),
            ProviderKind::Ollama => self.complete_ollama(provider, messages).await?,
            ProviderKind::Openrouter | ProviderKind::Custom => {
                self.complete_openai_compatible(provider, messages).await?
            }
        };

        Ok(AIResponse {
            content,
            provider_id: provider.id.clone(),
            model: provider.default_model.clone(),
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
    ) -> Result<String> {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut request = self.client.post(endpoint).json(&json!({
            "model": provider.default_model,
            "stream": false,
            "messages": to_openai_messages(messages),
        }));

        if let Some(api_key) = provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "provider returned HTTP {}: {}",
                status,
                truncate(&body)
            ));
        }

        let parsed: OpenAIChatResponse = serde_json::from_str(&body)?;
        parsed
            .choices
            .into_iter()
            .find_map(|choice| choice.message.content)
            .filter(|content| !content.trim().is_empty())
            .ok_or_else(|| anyhow!("provider response did not include assistant content"))
    }

    async fn complete_ollama(
        &self,
        provider: &LLMProvider,
        messages: &[ChatMessage],
    ) -> Result<String> {
        let endpoint = join_url(&provider.base_url, "/api/chat")?;
        let response = self
            .client
            .post(endpoint)
            .json(&json!({
                "model": provider.default_model,
                "stream": false,
                "messages": to_ollama_messages(messages),
            }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "provider returned HTTP {}: {}",
                status,
                truncate(&body)
            ));
        }

        let parsed: OllamaChatResponse = serde_json::from_str(&body)?;
        if parsed.message.content.trim().is_empty() {
            return Err(anyhow!(
                "provider response did not include assistant content"
            ));
        }
        Ok(parsed.message.content)
    }

    async fn test_openai_compatible(&self, provider: &LLMProvider) -> Result<()> {
        let endpoint = openai_chat_endpoint(&provider.base_url)?;
        let mut request = self.client.post(endpoint).json(&json!({
            "model": provider.default_model,
            "stream": false,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        }));

        if let Some(api_key) = provider
            .api_key
            .as_ref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await?;
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(anyhow!(
                "provider returned HTTP {}: {}",
                status,
                truncate(&body)
            ))
        }
    }

    async fn test_ollama(&self, provider: &LLMProvider) -> Result<()> {
        let endpoint = join_url(&provider.base_url, "/api/tags")?;
        let response = self.client.get(endpoint).send().await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("provider returned HTTP {}", response.status()))
        }
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

fn to_openai_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter_map(|message| match message.role {
            MessageRole::User => Some(json!({"role": "user", "content": message.content})),
            MessageRole::Assistant => {
                Some(json!({"role": "assistant", "content": message.content}))
            }
            MessageRole::System => Some(json!({"role": "system", "content": message.content})),
            MessageRole::Tool => None,
        })
        .collect()
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
        "Local mock AI response. Received {} user characters. Tool calling and dashboard generation are not enabled in W6.",
        latest_user.chars().count()
    )
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
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIMessage {
    content: Option<String>,
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
