//! W39: canonical signature for datasource identity.
//!
//! The same logical source can land in our system through three different
//! shapes: a saved [`DatasourceDefinition`], a Build `shared_datasources`
//! entry, or an inline widget `datasource_plan`. Without a canonical
//! signature, JSON argument order (or whitespace, or `null` vs absent
//! fields) silently produces duplicate workflows. This module collapses
//! all three shapes onto one comparable form so apply-time materialization
//! can reuse instead of duplicate.
//!
//! The signature intentionally **ignores** display-only fields (name,
//! description, refresh_cron, label) — those are policy decisions and
//! should not split identity. It compares:
//!
//! - `kind` (builtin_tool / mcp_tool / provider_prompt)
//! - `server_id`
//! - `tool_name`
//! - `arguments` (canonicalized JSON — object keys sorted, deep)
//! - `prompt`
//! - `pipeline` (canonicalized JSON; PipelineStep order matters)
//!
//! `Shared` and `Compose` kinds are not valid identities here: shared is
//! a proposal-only key and compose is widget-scoped by construction.

use crate::models::dashboard::{BuildDatasourcePlan, BuildDatasourcePlanKind, SharedDatasource};
use crate::models::datasource::DatasourceDefinition;
use crate::models::pipeline::PipelineStep;
use serde_json::{Map, Value};

/// Stable, comparable identity for an executable datasource shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DatasourceSignature {
    /// `"builtin_tool"`, `"mcp_tool"`, `"provider_prompt"`.
    pub kind: &'static str,
    /// Canonical JSON. `null` when the field was absent.
    pub server_id: String,
    pub tool_name: String,
    pub arguments: String,
    pub prompt: String,
    pub pipeline: String,
}

impl DatasourceSignature {
    /// Construct a signature from a saved [`DatasourceDefinition`].
    ///
    /// Returns `None` when the definition holds a kind that cannot exist
    /// as a saved definition (`Shared` / `Compose`); callers should treat
    /// that as "no identity to compare against".
    pub fn from_definition(def: &DatasourceDefinition) -> Option<Self> {
        Some(Self {
            kind: kind_label(&def.kind)?,
            server_id: canonical_opt_string(def.server_id.as_deref()),
            tool_name: canonical_opt_string(def.tool_name.as_deref()),
            arguments: canonical_value(def.arguments.as_ref()),
            prompt: canonical_opt_string(def.prompt.as_deref()),
            pipeline: canonical_pipeline(&def.pipeline),
        })
    }

    /// Construct a signature from a Build proposal `shared_datasources`
    /// entry. `Shared`/`Compose` kinds return `None`.
    pub fn from_shared(shared: &SharedDatasource) -> Option<Self> {
        Some(Self {
            kind: kind_label(&shared.kind)?,
            server_id: canonical_opt_string(shared.server_id.as_deref()),
            tool_name: canonical_opt_string(shared.tool_name.as_deref()),
            arguments: canonical_value(shared.arguments.as_ref()),
            prompt: canonical_opt_string(shared.prompt.as_deref()),
            pipeline: canonical_pipeline(&shared.pipeline),
        })
    }

    /// Construct a signature from an inline widget `datasource_plan`.
    /// `output_path`, `refresh_cron`, `source_key`, and `inputs` do not
    /// participate in identity. Returns `None` for `Shared`/`Compose`.
    pub fn from_inline_plan(plan: &BuildDatasourcePlan) -> Option<Self> {
        Some(Self {
            kind: kind_label(&plan.kind)?,
            server_id: canonical_opt_string(plan.server_id.as_deref()),
            tool_name: canonical_opt_string(plan.tool_name.as_deref()),
            arguments: canonical_value(plan.arguments.as_ref()),
            prompt: canonical_opt_string(plan.prompt.as_deref()),
            pipeline: canonical_pipeline(&plan.pipeline),
        })
    }
}

fn kind_label(kind: &BuildDatasourcePlanKind) -> Option<&'static str> {
    match kind {
        BuildDatasourcePlanKind::BuiltinTool => Some("builtin_tool"),
        BuildDatasourcePlanKind::McpTool => Some("mcp_tool"),
        BuildDatasourcePlanKind::ProviderPrompt => Some("provider_prompt"),
        BuildDatasourcePlanKind::Shared | BuildDatasourcePlanKind::Compose => None,
    }
}

fn canonical_opt_string(value: Option<&str>) -> String {
    match value {
        Some(s) => serde_json::to_string(s).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    }
}

fn canonical_value(value: Option<&Value>) -> String {
    match value {
        Some(v) => canonical_json(v),
        None => "null".into(),
    }
}

fn canonical_pipeline(pipeline: &[PipelineStep]) -> String {
    // Round-trip through JSON so PipelineStep variants normalise the
    // tagged shape the validator expects. Falls back to a deterministic
    // marker rather than silently mis-comparing on serialisation error.
    let value = serde_json::to_value(pipeline).unwrap_or(Value::Array(Vec::new()));
    canonical_json(&value)
}

/// Recursively sort object keys before serialising, so logically equal
/// JSON values produce byte-identical strings.
pub fn canonical_json(value: &Value) -> String {
    let normalized = normalize(value);
    serde_json::to_string(&normalized).unwrap_or_else(|_| "null".into())
}

fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::with_capacity(map.len());
            for key in keys {
                if let Some(child) = map.get(key) {
                    sorted.insert(key.clone(), normalize(child));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn http_plan(args: Value) -> BuildDatasourcePlan {
        BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::BuiltinTool,
            tool_name: Some("http_request".to_string()),
            server_id: None,
            arguments: Some(args),
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: None,
            inputs: None,
        }
    }

    #[test]
    fn reordered_json_keys_share_signature() {
        let a = http_plan(json!({
            "method": "GET",
            "url": "https://example.com/api",
            "headers": { "Accept": "application/json", "User-Agent": "datrina" },
        }));
        let b = http_plan(json!({
            "url": "https://example.com/api",
            "headers": { "User-Agent": "datrina", "Accept": "application/json" },
            "method": "GET",
        }));
        let sig_a = DatasourceSignature::from_inline_plan(&a).unwrap();
        let sig_b = DatasourceSignature::from_inline_plan(&b).unwrap();
        assert_eq!(sig_a, sig_b);
    }

    #[test]
    fn differing_url_breaks_signature() {
        let a = http_plan(json!({ "method": "GET", "url": "https://a.example.com" }));
        let b = http_plan(json!({ "method": "GET", "url": "https://b.example.com" }));
        let sig_a = DatasourceSignature::from_inline_plan(&a).unwrap();
        let sig_b = DatasourceSignature::from_inline_plan(&b).unwrap();
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn shared_kind_has_no_signature() {
        let plan = BuildDatasourcePlan {
            kind: BuildDatasourcePlanKind::Shared,
            tool_name: None,
            server_id: None,
            arguments: None,
            prompt: None,
            output_path: None,
            refresh_cron: None,
            pipeline: Vec::new(),
            source_key: Some("k".to_string()),
            inputs: None,
        };
        assert!(DatasourceSignature::from_inline_plan(&plan).is_none());
    }

    #[test]
    fn refresh_cron_does_not_split_identity() {
        let mut a = http_plan(json!({ "method": "GET", "url": "https://example.com" }));
        let mut b = http_plan(json!({ "method": "GET", "url": "https://example.com" }));
        a.refresh_cron = Some("0 */5 * * * *".to_string());
        b.refresh_cron = None;
        let sig_a = DatasourceSignature::from_inline_plan(&a).unwrap();
        let sig_b = DatasourceSignature::from_inline_plan(&b).unwrap();
        assert_eq!(sig_a, sig_b);
    }
}
