use super::Id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMProvider {
    pub id: Id,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub default_model: String,
    pub models: Vec<String>,
    #[serde(default = "default_true")]
    pub is_enabled: bool,
    /// W29: marks providers whose stored `kind` is not a supported product
    /// kind anymore (e.g. legacy `local_mock` rows migrated at load time).
    /// Unsupported providers are force-disabled, cannot become active, and
    /// surface a typed `ProviderSetupError::Unsupported` in the UI.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_unsupported: bool,
}

/// W29: production provider kinds. `local_mock` is intentionally absent —
/// see `LegacyProviderKind` for the storage-only deserialisation shim that
/// catches pre-W29 rows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Openrouter,
    Ollama,
    Custom,
}

/// W29: storage-only enum covering both supported `ProviderKind` variants
/// and legacy/unknown kinds that the storage layer must not silently drop.
/// `From<LegacyProviderKind>` to `ProviderKind` is intentionally absent;
/// callers must inspect `kind()` and handle `Unsupported(_)` explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyProviderKind {
    Supported(ProviderKind),
    Unsupported(String),
}

impl LegacyProviderKind {
    pub fn parse(raw: &str) -> Self {
        match serde_json::from_str::<ProviderKind>(raw) {
            Ok(kind) => Self::Supported(kind),
            Err(_) => Self::Unsupported(raw.trim_matches('"').to_string()),
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub default_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProviderRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRuntimeStatus {
    Ok,
    InvalidConfig,
    Unavailable,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTestResult {
    pub status: ProviderRuntimeStatus,
    pub provider_id: Id,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub checked_at: i64,
}

/// W29: typed provider setup / runtime error surfaced to the chat path
/// and the frontend. Replaces ad-hoc string errors so the UI can render
/// remediation copy per kind instead of dumping the raw message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ProviderSetupError {
    /// No `active_provider_id` config row, and no provider rows exist yet.
    NoActiveProvider,
    /// `active_provider_id` points to a row that no longer exists.
    ActiveProviderMissing { active_provider_id: String },
    /// `active_provider_id` points to a row that is currently disabled.
    ActiveProviderDisabled {
        active_provider_id: String,
        provider_name: String,
    },
    /// Provider exists and is enabled but failed `validate_provider`
    /// (missing API key, malformed base URL, etc.).
    ProviderInvalidConfig {
        active_provider_id: String,
        provider_name: String,
        reason: String,
    },
    /// `active_provider_id` points to a legacy / migrated row whose
    /// `kind` is not a supported product kind anymore.
    ProviderUnsupported {
        active_provider_id: String,
        provider_name: String,
        reason: String,
    },
    /// Provider is configured but the live readiness check failed (DNS,
    /// HTTP error, unreachable Ollama). Carries the underlying provider
    /// error for diagnostics.
    ProviderUnavailable {
        active_provider_id: String,
        provider_name: String,
        reason: String,
    },
}

/// W33: provider/model capability tag for strict structured-output
/// response formats. Build proposal emission asks for the strictest mode
/// the model is known to honour; everything else falls back to plain
/// text. The fallback is *visible*: callers receive the resolved tag and
/// can record it in cost/acceptance reports so a soft fallback never
/// counts as strict-mode acceptance evidence.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputCapability {
    /// Provider/model supports `response_format: { "type": "json_object" }`
    /// reliably. Today this is the level Datrina actually exercises on
    /// the validator retry. A future `JsonSchema` tier is reserved.
    JsonObject,
    /// No structured-output guarantee — provider may emit prose-wrapped
    /// JSON. Callers must keep the existing prose JSON extractor in
    /// place and treat any acceptance signal as non-strict.
    PlainText,
}

impl StructuredOutputCapability {
    pub fn is_strict(self) -> bool {
        matches!(self, Self::JsonObject)
    }
}

/// W43: capability tag pinned by dashboard/widget model policy. Stays
/// deliberately small — only the capabilities the widget runtime can
/// actually verify and that have a typed failure path are listed. New
/// entries should follow the same rule (no soft / aspirational caps).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetCapability {
    /// Provider/model honours `response_format: {"type":"json_object"}`.
    /// Reuses the same allowlist as [`supports_structured_output`].
    StructuredJsonObject,
    /// Provider supports OpenAI-style streaming SSE for assistant
    /// content. Ollama and pure JSON-completion providers don't, and we
    /// already detect that at the engine level — this cap surfaces it
    /// to the policy layer so a streaming-only Text widget can be made
    /// to fail closed rather than silently fall back to blocking.
    Streaming,
    /// Provider can be invoked from the function-calling path
    /// (OpenAI-compatible `tools` array). Ollama can't today, even
    /// though it accepts chat completions.
    ToolCalling,
}

impl WidgetCapability {
    /// Stable kebab-case tag for surfacing in errors and the UI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StructuredJsonObject => "structured_json_object",
            Self::Streaming => "streaming",
            Self::ToolCalling => "tool_calling",
        }
    }
}

/// W43: returns the capabilities a given provider/model pair supports,
/// derived from the existing capability heuristics. Conservative — when
/// in doubt, the cap is omitted so the policy check fails closed and
/// the user gets a typed remediation error instead of a silent runtime
/// surprise.
pub fn provider_capabilities(kind: ProviderKind, model: &str) -> Vec<WidgetCapability> {
    let mut caps = Vec::new();
    if supports_structured_output(kind, model).is_strict() {
        caps.push(WidgetCapability::StructuredJsonObject);
    }
    match kind {
        ProviderKind::Openrouter | ProviderKind::Custom => {
            caps.push(WidgetCapability::Streaming);
            caps.push(WidgetCapability::ToolCalling);
        }
        ProviderKind::Ollama => {
            // Ollama supports neither SSE streaming on our `/api/chat`
            // wrapper nor function-calling in our current path.
        }
    }
    caps
}

/// W43: typed model-selection / capability error returned by the widget
/// refresh path when the resolved policy cannot run the widget. Frontend
/// matches on `code` to render the remediation copy and offer to clear
/// the override.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum WidgetModelError {
    /// Policy points to a provider id that no longer exists.
    ProviderMissing {
        provider_id: Id,
        source: WidgetModelSource,
    },
    /// Policy provider exists but is disabled or marked unsupported.
    ProviderDisabled {
        provider_id: Id,
        provider_name: String,
        source: WidgetModelSource,
    },
    /// Policy provider failed `validate_provider` (missing key, malformed url).
    ProviderInvalidConfig {
        provider_id: Id,
        provider_name: String,
        source: WidgetModelSource,
        reason: String,
    },
    /// Provider is fine but the requested model is missing one or more
    /// of the capabilities the policy pinned. Carries the list of
    /// unmet caps so the UI can render remediation.
    CapabilityUnsupported {
        provider_id: Id,
        provider_name: String,
        model: String,
        source: WidgetModelSource,
        missing: Vec<WidgetCapability>,
    },
}

impl WidgetModelError {
    pub fn message(&self) -> String {
        match self {
            Self::ProviderMissing { provider_id, source } => format!(
                "widget_model_provider_missing: {} policy points to provider '{}' which no longer exists. Pick a new model in Provider Settings.",
                source.as_str(),
                provider_id
            ),
            Self::ProviderDisabled {
                provider_name,
                source,
                ..
            } => format!(
                "widget_model_provider_disabled: {} policy uses provider '{}' which is disabled or unsupported. Re-enable it or pick a different model.",
                source.as_str(),
                provider_name
            ),
            Self::ProviderInvalidConfig {
                provider_name,
                source,
                reason,
                ..
            } => format!(
                "widget_model_provider_invalid_config: {} policy provider '{}' is not usable — {}.",
                source.as_str(),
                provider_name,
                reason
            ),
            Self::CapabilityUnsupported {
                provider_name,
                model,
                source,
                missing,
                ..
            } => {
                let caps = missing
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "widget_model_capability_unsupported: {} policy provider '{}' model '{}' does not support required capability/capabilities: {}.",
                    source.as_str(),
                    provider_name,
                    model,
                    caps
                )
            }
        }
    }
}

impl std::fmt::Display for WidgetModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for WidgetModelError {}

/// W43: provenance for `EffectiveWidgetModel` — which surface chose the
/// resolved model. Surfaced in the widget inspector so users can tell
/// the dashboard default apart from a per-widget override at a glance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidgetModelSource {
    WidgetOverride,
    DashboardDefault,
    AppActiveProvider,
}

impl WidgetModelSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WidgetOverride => "widget_override",
            Self::DashboardDefault => "dashboard_default",
            Self::AppActiveProvider => "app_active_provider",
        }
    }
}

/// W43: resolved widget model selection passed down the refresh path.
/// Carries the live [`LLMProvider`] (with api key loaded server-side
/// only), the model id actually applied, and the source/inheritance
/// metadata for the UI.
#[derive(Debug, Clone)]
pub struct EffectiveWidgetModel {
    pub provider: LLMProvider,
    pub model: String,
    pub source: WidgetModelSource,
    pub required_caps: Vec<WidgetCapability>,
}

/// W33: capability lookup for the structured-output response format the
/// validator retry path should request. Conservative: known
/// OpenAI-compatible models (OpenAI proper, Anthropic via OpenRouter,
/// Moonshot Kimi family, Mistral, DeepSeek) opt into `json_object`;
/// Ollama and unknown OpenRouter aliases stay on `PlainText` so we do
/// not silently send a body the provider will reject.
pub fn supports_structured_output(kind: ProviderKind, model: &str) -> StructuredOutputCapability {
    let model_lc = model.trim().to_ascii_lowercase();
    match kind {
        ProviderKind::Ollama => StructuredOutputCapability::PlainText,
        ProviderKind::Openrouter | ProviderKind::Custom => {
            const JSON_OBJECT_PREFIXES: &[&str] = &[
                "openai/",
                "gpt-3.5",
                "gpt-4",
                "gpt-4o",
                "gpt-4.1",
                "gpt-5",
                "o1",
                "o3",
                "o4",
                "moonshotai/kimi-k1",
                "moonshotai/kimi-k2",
                "anthropic/claude-3",
                "anthropic/claude-sonnet-4",
                "anthropic/claude-opus-4",
                "anthropic/claude-haiku-4",
                "mistralai/",
                "deepseek/",
                "qwen/qwen2.5",
                "x-ai/grok-2",
                "x-ai/grok-3",
                "google/gemini-1.5",
                "google/gemini-2",
            ];
            if JSON_OBJECT_PREFIXES
                .iter()
                .any(|prefix| model_lc.starts_with(prefix) || model_lc.contains(prefix))
            {
                StructuredOutputCapability::JsonObject
            } else {
                StructuredOutputCapability::PlainText
            }
        }
    }
}

impl ProviderSetupError {
    /// Stable, human-readable string for command results. The frontend
    /// can match on the typed `code` field; this exists so legacy
    /// callers that surface the raw `error` field still produce useful
    /// copy until they migrate.
    pub fn message(&self) -> String {
        match self {
            Self::NoActiveProvider => {
                "no_active_provider: no LLM provider is configured. Open Provider Settings and add OpenRouter, Ollama, or a custom OpenAI-compatible endpoint.".to_string()
            }
            Self::ActiveProviderMissing { active_provider_id } => format!(
                "active_provider_missing: stored active provider '{}' no longer exists. Open Provider Settings and choose a new active provider.",
                active_provider_id
            ),
            Self::ActiveProviderDisabled { provider_name, .. } => format!(
                "active_provider_disabled: provider '{}' is disabled. Re-enable it or pick a different active provider.",
                provider_name
            ),
            Self::ProviderInvalidConfig {
                provider_name,
                reason,
                ..
            } => format!(
                "provider_invalid_config: provider '{}' is not usable yet — {}",
                provider_name, reason
            ),
            Self::ProviderUnsupported {
                provider_name,
                reason,
                ..
            } => format!(
                "provider_unsupported: provider '{}' uses a legacy kind that is no longer supported — {}. Replace it with OpenRouter, Ollama, or a custom OpenAI-compatible provider.",
                provider_name, reason
            ),
            Self::ProviderUnavailable {
                provider_name,
                reason,
                ..
            } => format!(
                "provider_unavailable: provider '{}' did not pass the readiness check — {}",
                provider_name, reason
            ),
        }
    }
}

impl std::fmt::Display for ProviderSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ProviderSetupError {}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_kind_round_trips_supported() {
        assert!(matches!(
            LegacyProviderKind::parse("\"openrouter\""),
            LegacyProviderKind::Supported(ProviderKind::Openrouter)
        ));
    }

    #[test]
    fn legacy_kind_catches_local_mock_as_unsupported() {
        let parsed = LegacyProviderKind::parse("\"local_mock\"");
        assert!(matches!(parsed, LegacyProviderKind::Unsupported(ref s) if s == "local_mock"));
        assert!(!parsed.is_supported());
    }

    #[test]
    fn legacy_kind_catches_unknown_string() {
        let parsed = LegacyProviderKind::parse("\"vendor_x\"");
        assert!(matches!(parsed, LegacyProviderKind::Unsupported(ref s) if s == "vendor_x"));
    }

    #[test]
    fn provider_setup_error_message_includes_remediation() {
        let err = ProviderSetupError::NoActiveProvider;
        assert!(err.message().contains("Provider Settings"));
    }

    #[test]
    fn structured_output_capability_known_openai_aliases_get_json_object() {
        assert_eq!(
            supports_structured_output(ProviderKind::Openrouter, "openai/gpt-4o-mini"),
            StructuredOutputCapability::JsonObject
        );
        assert_eq!(
            supports_structured_output(
                ProviderKind::Openrouter,
                "anthropic/claude-sonnet-4-20250514"
            ),
            StructuredOutputCapability::JsonObject
        );
        assert_eq!(
            supports_structured_output(ProviderKind::Openrouter, "moonshotai/kimi-k2.6-instruct"),
            StructuredOutputCapability::JsonObject
        );
    }

    #[test]
    fn structured_output_capability_ollama_is_plain_text() {
        assert_eq!(
            supports_structured_output(ProviderKind::Ollama, "llama3.1:8b"),
            StructuredOutputCapability::PlainText
        );
    }

    #[test]
    fn structured_output_capability_unknown_aliases_fall_back_visibly() {
        assert_eq!(
            supports_structured_output(ProviderKind::Custom, "vendor-x/private-model-v9"),
            StructuredOutputCapability::PlainText
        );
        assert!(
            !supports_structured_output(ProviderKind::Custom, "vendor-x/private-model-v9")
                .is_strict()
        );
    }
}
