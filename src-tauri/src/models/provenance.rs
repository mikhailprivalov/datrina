//! W41: typed widget execution summary surfaced by `get_widget_provenance`.
//!
//! Reuses existing primitives (workflow run summary, datasource health,
//! W23 pipeline traces) and adds an explicit LLM participation tag plus
//! redacted source/call metadata for the widget inspector and reflection
//! prompts. Secrets never leave Rust: argument previews are scrubbed at
//! the boundary by `redact_value`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::widget::DatasourceBindingSource;
use super::workflow::{RunStatus, SchedulePauseState, TriggerKind};
use super::{Id, Timestamp};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetProvenance {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub widget_title: String,
    pub widget_kind: String,
    pub llm_participation: LlmParticipation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<DatasourceProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderProvenance>,
    pub tail: TailSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRunSummary>,
    pub links: ProvenanceLinks,
    /// Compact summary text the reflection turn can paste verbatim.
    /// Empty when the widget has no datasource.
    pub redacted_summary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmParticipation {
    None,
    ProviderSource,
    LlmPostprocess,
    WidgetTextGeneration,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceProvenance {
    pub workflow_id: Id,
    pub output_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_source: Option<DatasourceBindingSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_at: Option<Timestamp>,
    pub source: SourceProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// W50: user pause state on the backing workflow. `None` when the
    /// workflow could not be resolved at provenance time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_state: Option<SchedulePauseState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceProvenance {
    McpTool {
        server_id: String,
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments_preview: Option<Value>,
    },
    BuiltinTool {
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments_preview: Option<Value>,
    },
    ProviderPrompt {
        prompt_preview: String,
    },
    Compose {
        inputs: Vec<ComposeInputSummary>,
    },
    /// Workflow exists but its source node is not a shape we recognise.
    Unknown,
    /// Widget points to a workflow id that could not be resolved at the
    /// time of inspection — surfaced explicitly instead of pretending the
    /// widget has no datasource.
    Missing {
        workflow_id: Id,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeInputSummary {
    pub name: String,
    pub source: Box<SourceProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProvenance {
    pub provider_id: Id,
    pub provider_name: String,
    pub provider_kind: String,
    pub model: String,
    pub is_active_provider: bool,
    /// W43: which surface chose this provider/model — the widget
    /// override, the dashboard default, or the app active provider.
    /// Surfaced in the inspector so the user can tell at a glance
    /// whether the widget inherits or overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_source: Option<crate::models::provider::WidgetModelSource>,
    /// W43: capabilities the resolved policy pinned. Empty when no
    /// policy was applied. Shown next to the model badge so the user
    /// understands why a particular model was required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_caps: Vec<crate::models::provider::WidgetCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailSummary {
    pub step_count: u32,
    pub has_llm_postprocess: bool,
    pub has_mcp_call: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRunSummary {
    pub run_id: Id,
    pub status: RunStatus,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvenanceLinks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<Id>,
    /// `true` when at least one W23 trace exists for this widget.
    #[serde(default)]
    pub has_pipeline_traces: bool,
}

/// Header / argument keys we always replace with `<redacted>` before
/// returning a payload to the UI or to the reflection prompt. Comparison
/// is case-insensitive on the *key name only* — we never inspect the
/// value to decide.
pub const SENSITIVE_KEY_PARTS: &[&str] = &[
    "authorization",
    "api_key",
    "apikey",
    "api-key",
    "x-api-key",
    "bearer",
    "access_token",
    "refresh_token",
    "client_secret",
    "password",
    "passwd",
    "secret",
    "token",
    "cookie",
    "set-cookie",
    "session",
    "private_key",
];

/// Recursively scrub a JSON value: any object key whose lowercase form
/// matches one of [`SENSITIVE_KEY_PARTS`] is replaced with the literal
/// string `"<redacted>"`. The structure stays so the UI can still render
/// it as a shape preview. Strings/numbers at the top level are returned
/// untouched; we never guess sensitivity from a value alone.
pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, val) in map {
                if is_sensitive_key(key) {
                    out.insert(key.clone(), Value::String("<redacted>".to_string()));
                } else {
                    out.insert(key.clone(), redact_value(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_KEY_PARTS
        .iter()
        .any(|needle| lower == *needle || lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redact_replaces_known_keys() {
        let input = json!({
            "url": "https://example.com",
            "headers": {
                "Authorization": "Bearer hunter2",
                "X-Api-Key": "abc",
                "Accept": "application/json"
            },
            "api_key": "leaked",
        });
        let scrubbed = redact_value(&input);
        assert_eq!(scrubbed["url"], json!("https://example.com"));
        assert_eq!(scrubbed["headers"]["Authorization"], json!("<redacted>"));
        assert_eq!(scrubbed["headers"]["X-Api-Key"], json!("<redacted>"));
        assert_eq!(scrubbed["headers"]["Accept"], json!("application/json"));
        assert_eq!(scrubbed["api_key"], json!("<redacted>"));
    }

    #[test]
    fn redact_is_recursive_through_arrays() {
        let input = json!([
            { "token": "x", "id": 1 },
            { "token": "y", "id": 2 }
        ]);
        let scrubbed = redact_value(&input);
        assert_eq!(scrubbed[0]["token"], json!("<redacted>"));
        assert_eq!(scrubbed[0]["id"], json!(1));
        assert_eq!(scrubbed[1]["token"], json!("<redacted>"));
    }

    #[test]
    fn redact_leaves_clean_payloads_untouched() {
        let input = json!({ "query": "tag:climate", "limit": 5 });
        assert_eq!(redact_value(&input), input);
    }

    #[test]
    fn sensitive_key_detection_is_case_insensitive() {
        assert!(is_sensitive_key("Authorization"));
        assert!(is_sensitive_key("X-API-Key"));
        assert!(is_sensitive_key("session_id"));
        assert!(!is_sensitive_key("query"));
        assert!(!is_sensitive_key("limit"));
    }
}
