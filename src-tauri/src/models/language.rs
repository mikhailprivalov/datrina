//! W47: assistant language policy model + curated BCP-47 catalog.
//!
//! Datrina does not localize JSON schema keys, tool names, ids, or
//! validation issue codes. The language policy is a small instruction
//! prepended to provider system prompts (chat, Build chat, pipeline
//! `LlmPostprocess`, workflow LLM node) so the assistant's prose output
//! follows the user's chosen language across providers.
//!
//! The catalog is intentionally static and local-first. Provider
//! support hints are practical — GPT/Claude/Kimi all follow the prompt
//! reliably for these languages; we don't claim an exhaustive feature
//! matrix per model variant.

use serde::{Deserialize, Serialize};

pub const APP_LANGUAGE_CONFIG_KEY: &str = "assistant_language_policy";

/// Per-scope language policy. `Auto` means "follow the user's latest
/// natural language". `Explicit` pins a curated BCP-47 tag from the
/// static catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AssistantLanguagePolicy {
    Auto,
    Explicit { tag: String },
}

impl Default for AssistantLanguagePolicy {
    fn default() -> Self {
        Self::Auto
    }
}

impl AssistantLanguagePolicy {
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::Auto => None,
            Self::Explicit { tag } => Some(tag.as_str()),
        }
    }
}

/// Text direction hint for the UI. The injection prompt itself does not
/// change; the React side reads this to render Arabic/Hebrew text RTL.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextDirection {
    Ltr,
    Rtl,
}

/// Practical provider support hint. Three providers Datrina actually
/// targets today: GPT (OpenAI / OpenRouter `openai/*`, `gpt-*`), Claude
/// (`anthropic/*`), Kimi (`moonshotai/kimi-*`). We mark every catalog
/// language as `prompt_supported` because none of the three exposes a
/// formal per-language capability flag — quality varies but the prompt
/// is honoured. `notes` carries the caveat where it matters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageProviderSupport {
    pub provider: String,
    pub prompt_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantLanguageOption {
    /// BCP-47 tag — e.g. `en`, `ru`, `zh-Hans`, `zh-Hant`, `pt-BR`.
    pub tag: String,
    /// English-facing label used in catalog UIs (settings dropdown).
    pub label: String,
    /// Native endonym shown next to the English label so users can
    /// recognise their own language without translating.
    pub native_label: String,
    pub direction: TextDirection,
    /// English name of the language used inside the system instruction
    /// ("Respond in <name>"). Kept separate from `label` so we can
    /// brand-cap the dropdown without affecting the prompt wording.
    pub prompt_name: String,
    pub provider_support: Vec<LanguageProviderSupport>,
}

/// W47: provenance for [`EffectiveAssistantLanguage`]. Tells the UI
/// which surface the resolved policy came from so the chat header /
/// dashboard inspector can render a "language: ru (dashboard override)"
/// chip rather than just the tag.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssistantLanguageSource {
    /// No policy set anywhere — the assistant follows the user's prompt
    /// language. The `Auto` mode at the app level resolves to this too.
    Auto,
    AppDefault,
    DashboardOverride,
    SessionOverride,
}

impl AssistantLanguageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AppDefault => "app_default",
            Self::DashboardOverride => "dashboard_override",
            Self::SessionOverride => "session_override",
        }
    }
}

/// Resolved language for one request scope. `option == None` means
/// "auto, no injection" — the system prompt is left untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveAssistantLanguage {
    pub source: AssistantLanguageSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<AssistantLanguageOption>,
}

impl EffectiveAssistantLanguage {
    pub fn auto() -> Self {
        Self {
            source: AssistantLanguageSource::Auto,
            option: None,
        }
    }

    /// System-prompt fragment injected into provider requests when the
    /// policy is explicit. The wording is high-priority and short so it
    /// survives ahead of long context blocks.
    pub fn system_directive(&self) -> Option<String> {
        self.option.as_ref().map(|option| {
            format!(
                "Respond in {0}. Keep all assistant prose, summaries, and \
                 explanations in {0}. Do not translate JSON keys, schema \
                 field names, tool names, widget ids, datasource ids, \
                 parameter names, validation issue codes, code identifiers, \
                 or other machine-readable tokens — those stay in their \
                 original form. If the user explicitly asks in a different \
                 language, switch for that turn only.",
                option.prompt_name
            )
        })
    }
}

/// W47: curated static catalog. Order matches the docs table in
/// `docs/W47_LLM_CONVERSATION_LANGUAGE_SETTINGS.md` so the dropdown
/// reads top-down without surprises.
pub fn language_catalog() -> Vec<AssistantLanguageOption> {
    const PROVIDERS: &[&str] = &["openai", "anthropic", "moonshot"];

    let practical = |notes: Option<&str>| -> Vec<LanguageProviderSupport> {
        PROVIDERS
            .iter()
            .map(|p| LanguageProviderSupport {
                provider: (*p).to_string(),
                prompt_supported: true,
                notes: notes.map(str::to_string),
            })
            .collect()
    };

    let with_kimi_note = |label_note: &str| practical(Some(&format!("Kimi: {label_note}")));

    vec![
        opt(
            "en",
            "English",
            "English",
            TextDirection::Ltr,
            "English",
            practical(None),
        ),
        opt(
            "ru",
            "Russian",
            "Русский",
            TextDirection::Ltr,
            "Russian",
            practical(None),
        ),
        opt(
            "zh-Hans",
            "Chinese (Simplified)",
            "简体中文",
            TextDirection::Ltr,
            "Simplified Chinese",
            practical(None),
        ),
        opt(
            "zh-Hant",
            "Chinese (Traditional)",
            "繁體中文",
            TextDirection::Ltr,
            "Traditional Chinese",
            practical(None),
        ),
        opt(
            "ja",
            "Japanese",
            "日本語",
            TextDirection::Ltr,
            "Japanese",
            practical(None),
        ),
        opt(
            "ko",
            "Korean",
            "한국어",
            TextDirection::Ltr,
            "Korean",
            practical(None),
        ),
        opt(
            "es",
            "Spanish",
            "Español",
            TextDirection::Ltr,
            "Spanish",
            practical(None),
        ),
        opt(
            "fr",
            "French",
            "Français",
            TextDirection::Ltr,
            "French",
            practical(None),
        ),
        opt(
            "de",
            "German",
            "Deutsch",
            TextDirection::Ltr,
            "German",
            practical(None),
        ),
        opt(
            "pt",
            "Portuguese",
            "Português",
            TextDirection::Ltr,
            "Portuguese",
            practical(None),
        ),
        opt(
            "it",
            "Italian",
            "Italiano",
            TextDirection::Ltr,
            "Italian",
            practical(None),
        ),
        opt(
            "nl",
            "Dutch",
            "Nederlands",
            TextDirection::Ltr,
            "Dutch",
            practical(None),
        ),
        opt(
            "pl",
            "Polish",
            "Polski",
            TextDirection::Ltr,
            "Polish",
            practical(None),
        ),
        opt(
            "uk",
            "Ukrainian",
            "Українська",
            TextDirection::Ltr,
            "Ukrainian",
            practical(None),
        ),
        opt(
            "tr",
            "Turkish",
            "Türkçe",
            TextDirection::Ltr,
            "Turkish",
            practical(None),
        ),
        opt(
            "ar",
            "Arabic",
            "العربية",
            TextDirection::Rtl,
            "Arabic",
            with_kimi_note("quality varies; prefer GPT/Claude for fluency"),
        ),
        opt(
            "he",
            "Hebrew",
            "עברית",
            TextDirection::Rtl,
            "Hebrew",
            with_kimi_note("quality varies; prefer GPT/Claude for fluency"),
        ),
        opt(
            "hi",
            "Hindi",
            "हिन्दी",
            TextDirection::Ltr,
            "Hindi",
            practical(None),
        ),
        opt(
            "bn",
            "Bengali",
            "বাংলা",
            TextDirection::Ltr,
            "Bengali",
            practical(None),
        ),
        opt(
            "ur",
            "Urdu",
            "اردو",
            TextDirection::Rtl,
            "Urdu",
            with_kimi_note("quality varies; prefer GPT/Claude for fluency"),
        ),
        opt(
            "id",
            "Indonesian",
            "Bahasa Indonesia",
            TextDirection::Ltr,
            "Indonesian",
            practical(None),
        ),
        opt(
            "vi",
            "Vietnamese",
            "Tiếng Việt",
            TextDirection::Ltr,
            "Vietnamese",
            practical(None),
        ),
        opt(
            "th",
            "Thai",
            "ไทย",
            TextDirection::Ltr,
            "Thai",
            practical(None),
        ),
        opt(
            "ms",
            "Malay",
            "Bahasa Melayu",
            TextDirection::Ltr,
            "Malay",
            practical(None),
        ),
        opt(
            "cs",
            "Czech",
            "Čeština",
            TextDirection::Ltr,
            "Czech",
            practical(None),
        ),
        opt(
            "el",
            "Greek",
            "Ελληνικά",
            TextDirection::Ltr,
            "Greek",
            practical(None),
        ),
        opt(
            "sv",
            "Swedish",
            "Svenska",
            TextDirection::Ltr,
            "Swedish",
            practical(None),
        ),
        opt(
            "no",
            "Norwegian",
            "Norsk",
            TextDirection::Ltr,
            "Norwegian",
            practical(None),
        ),
        opt(
            "da",
            "Danish",
            "Dansk",
            TextDirection::Ltr,
            "Danish",
            practical(None),
        ),
        opt(
            "fi",
            "Finnish",
            "Suomi",
            TextDirection::Ltr,
            "Finnish",
            practical(None),
        ),
    ]
}

fn opt(
    tag: &str,
    label: &str,
    native_label: &str,
    direction: TextDirection,
    prompt_name: &str,
    provider_support: Vec<LanguageProviderSupport>,
) -> AssistantLanguageOption {
    AssistantLanguageOption {
        tag: tag.to_string(),
        label: label.to_string(),
        native_label: native_label.to_string(),
        direction,
        prompt_name: prompt_name.to_string(),
        provider_support,
    }
}

/// Look up a catalog option by BCP-47 tag. Comparison is
/// case-insensitive; the catalog tag (canonical form) is returned.
pub fn find_language(tag: &str) -> Option<AssistantLanguageOption> {
    let needle = tag.trim().to_ascii_lowercase();
    language_catalog()
        .into_iter()
        .find(|option| option.tag.to_ascii_lowercase() == needle)
}

/// Parse a JSON-encoded [`AssistantLanguagePolicy`] from the
/// `app_config` table. Tolerates malformed / empty payloads by
/// returning [`AssistantLanguagePolicy::Auto`] — the storage layer
/// already redacted the policy, no point in failing chat over it.
pub fn parse_policy(raw: &str) -> AssistantLanguagePolicy {
    serde_json::from_str(raw).unwrap_or(AssistantLanguagePolicy::Auto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_required_languages() {
        let tags: Vec<String> = language_catalog().into_iter().map(|o| o.tag).collect();
        for required in [
            "en", "ru", "zh-Hans", "zh-Hant", "ja", "ko", "es", "fr", "de", "pt", "it", "nl", "pl",
            "uk", "tr", "ar", "he", "hi", "bn", "ur", "id", "vi", "th", "ms", "cs", "el", "sv",
            "no", "da", "fi",
        ] {
            assert!(
                tags.iter().any(|t| t == required),
                "catalog missing required tag {required}"
            );
        }
    }

    #[test]
    fn rtl_languages_marked_correctly() {
        for tag in ["ar", "he", "ur"] {
            let opt = find_language(tag).expect("catalog lookup");
            assert_eq!(opt.direction, TextDirection::Rtl, "{tag} must be RTL");
        }
        for tag in ["en", "ru", "ja"] {
            let opt = find_language(tag).expect("catalog lookup");
            assert_eq!(opt.direction, TextDirection::Ltr, "{tag} must be LTR");
        }
    }

    #[test]
    fn auto_policy_emits_no_directive() {
        let resolved = EffectiveAssistantLanguage::auto();
        assert!(resolved.system_directive().is_none());
    }

    #[test]
    fn explicit_policy_emits_directive_with_prompt_name() {
        let resolved = EffectiveAssistantLanguage {
            source: AssistantLanguageSource::AppDefault,
            option: find_language("ru"),
        };
        let directive = resolved.system_directive().expect("directive");
        assert!(directive.contains("Respond in Russian"));
        assert!(directive.contains("Do not translate JSON keys"));
    }

    #[test]
    fn parse_policy_tolerates_malformed_payload() {
        let policy = parse_policy("not json");
        assert!(matches!(policy, AssistantLanguagePolicy::Auto));
    }

    #[test]
    fn parse_policy_round_trips_explicit() {
        let raw = serde_json::to_string(&AssistantLanguagePolicy::Explicit {
            tag: "fr".to_string(),
        })
        .unwrap();
        match parse_policy(&raw) {
            AssistantLanguagePolicy::Explicit { tag } => assert_eq!(tag, "fr"),
            AssistantLanguagePolicy::Auto => panic!("expected explicit"),
        }
    }
}
