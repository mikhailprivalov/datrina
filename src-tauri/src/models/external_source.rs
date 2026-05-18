//! W37: external open-source / free-use source catalog.
//!
//! Datrina ships a small *built-in* catalog of reviewed HTTP/JSON sources
//! (`ExternalSource`). Each entry carries reviewed license/terms metadata,
//! a default request template, and a default typed pipeline. Per-user
//! state (enabled, optional API credential) lives in `external_source_state`.
//!
//! The catalog itself is static Rust data — it is NOT user-editable and is
//! never serialized back into the database. Upgrading the catalog (adding
//! a source, tightening review status) is a code change.

use super::{Id, Timestamp};
use crate::models::pipeline::PipelineStep;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Reviewed status for a built-in source. Only `Allowed` and
/// `AllowedWithConditions` are runnable; `NeedsReview` and `Blocked`
/// surface in the UI but fail closed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSourceReviewStatus {
    Allowed,
    AllowedWithConditions,
    NeedsReview,
    Blocked,
}

impl ExternalSourceReviewStatus {
    pub fn is_runnable(self) -> bool {
        matches!(self, Self::Allowed | Self::AllowedWithConditions)
    }
}

/// Adapter kind.
///
/// * `HttpJson` — request/response goes through the existing
///   `tool_engine.http_request` path with caller-supplied URL/query
///   substitution and an optional BYOK header.
/// * `WebFetch` — single-URL safe fetch through `tool_engine.web_fetch`
///   with robots.txt obedience, size cap, and text-first response shape.
/// * `McpRecommended` — informational catalog row that points at a
///   third-party MCP server. The user installs/configures it through
///   the existing MCP Settings surface; the catalog provides reviewed
///   metadata (license, terms, install command) and a copy button.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSourceAdapter {
    HttpJson,
    WebFetch,
    McpRecommended,
}

/// Coarse domain tag used by the catalog UI to group entries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSourceDomain {
    WebSearch,
    WebFetch,
    KnowledgeBase,
    CryptoMarket,
    DeveloperData,
    News,
    /// W37++: recommended third-party MCP servers — informational rows.
    McpRecommended,
}

/// W37++: machine-readable per-source rate / billing facts. The UI
/// renders these verbatim so the user can compare plan tiers before
/// enabling a paid source. Recorded as separate review metadata so a
/// promotion from `needs_review` to `allowed_with_conditions` is a code
/// change, not a runtime decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceRateLimit {
    /// Short human description ("free credit", "metered", "keyless").
    pub plan_name: String,
    /// Free quota text — e.g. "$5/mo credit (~1k requests)" or
    /// "60 requests/hour unauthenticated".
    pub free_quota: String,
    /// Paid tier description — None when there is no metered path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_tier: Option<String>,
    /// Queries-per-second cap on the entry tier. None for keyless
    /// best-effort APIs that don't publish a number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queries_per_second: Option<f32>,
    /// `true` when the source mandates a visible attribution beyond the
    /// catalog's standard `attribution` field — e.g. a public webpage
    /// crediting the provider.
    #[serde(default)]
    pub attribution_required: bool,
    /// `true` when storing the source's results requires an explicit
    /// licence (e.g. Brave Search storage rights). Saved datasources of
    /// such sources will refuse to refresh on a cron without surfacing
    /// the constraint.
    #[serde(default)]
    pub storage_rights_required: bool,
}

/// W37++: per-source recommended MCP install metadata, attached when
/// `adapter == McpRecommended`. Catalog only — never executed by
/// Datrina; the user picks it up and pastes it into MCP Settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInstallRecommendation {
    pub command: String,
    pub args: Vec<String>,
    /// Suggested env vars (key + human description). Values are entered
    /// by the user inside MCP Settings — never carried in the catalog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_hints: Vec<McpInstallEnvHint>,
    /// Display string for the package source (`npm`, `pypi`, `github`).
    pub package_kind: String,
    /// Short description of the package (e.g. PyPI summary).
    pub package_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInstallEnvHint {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// Credential policy: most W37 sources are keyless. A `Required` entry
/// will not appear in the chat tool list and cannot execute until the
/// user records an API key through the Source Catalog UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSourceCredentialPolicy {
    None,
    Optional,
    Required,
}

/// A typed parameter exposed to the LLM (chat tool spec) and to the
/// Source Catalog test UI. The Rust adapter substitutes `{name}` tokens
/// inside path/query templates and forwards declared body parameters
/// as a JSON object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceParam {
    pub name: String,
    pub description: String,
    /// JSON Schema fragment merged into the chat tool spec. Defaults to
    /// `{ "type": "string" }` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default)]
    pub required: bool,
    /// Default value used by the catalog test UI when the field is left
    /// blank. Not used by the LLM tool path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// HTTP request template. Tokens of the form `{name}` are substituted
/// from the call arguments before the URL is validated by the existing
/// `ToolEngine` URL policy. The static parts (host, path skeleton, query
/// keys) come from the catalog — the LLM only fills typed slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceHttpRequest {
    pub method: String,
    /// Base URL up to and including the path. Substitution applies.
    pub url: String,
    /// Static + parametrised query keys. Values may contain `{name}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
    /// Static headers (User-Agent is set by `ToolEngine`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Header name receiving the user-provided credential, when the
    /// source declares `credential_policy != None`. The actual value is
    /// pulled from `external_source_state`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_header: Option<String>,
    /// Optional credential value prefix (e.g. `"Bearer "`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_prefix: Option<String>,
    /// Argument names that should be sent as a JSON body instead of
    /// query string. Useful for POST-style search APIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_params: Vec<String>,
}

/// A single curated catalog entry. Static catalog data only — no user
/// state. Pair with [`ExternalSourceState`] to render the catalog UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSource {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub domain: ExternalSourceDomain,
    pub adapter: ExternalSourceAdapter,
    pub review_status: ExternalSourceReviewStatus,
    /// Date the review was performed (ISO-8601 string), recorded so a
    /// stale review is visible in the catalog row.
    pub review_date: String,
    /// Adapter / repository license (e.g. `"MIT"`, `"Apache-2.0"`,
    /// `"native"` for first-party adapters).
    pub adapter_license: String,
    /// Upstream terms / API policy URL.
    pub terms_url: String,
    /// One-line reason behind the review status. Surfaced verbatim in
    /// the catalog UI so the conditions are explicit.
    pub review_notes: String,
    /// Attribution text the UI must show when results are rendered.
    /// `None` means upstream terms do not require attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    pub credential_policy: ExternalSourceCredentialPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_help: Option<String>,
    pub http: ExternalSourceHttpRequest,
    pub params: Vec<ExternalSourceParam>,
    /// Typed pipeline applied to the raw HTTP response, so the LLM and
    /// the catalog test UI see the shaped result, not the wire format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_pipeline: Vec<PipelineStep>,
    /// W37++: machine-readable rate/billing metadata. Optional because
    /// keyless public APIs without published numbers leave this blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<ExternalSourceRateLimit>,
    /// W37++: MCP install metadata. Only populated when
    /// `adapter == McpRecommended`; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_install: Option<McpInstallRecommendation>,
}

/// User-facing per-source state. Persisted in `external_source_state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceState {
    pub source_id: String,
    pub is_enabled: bool,
    /// `true` when the user has stored a credential. The actual value
    /// is never returned through commands — the React side only sees a
    /// "credential set" boolean.
    pub has_credential: bool,
    pub updated_at: Timestamp,
}

/// Composite shape returned by `list_external_sources`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceWithState {
    #[serde(flatten)]
    pub source: ExternalSource,
    pub state: ExternalSourceState,
    /// `true` when this source is currently exposed to chat as a tool.
    /// Equivalent to `is_runnable && is_enabled && credential satisfied`.
    pub is_runnable: bool,
    /// When `is_runnable == false`, this carries the user-visible reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

/// Argument envelope for `test_external_source` / `set_external_source_enabled`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceTestRequest {
    pub source_id: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceTestResult {
    pub source_id: String,
    pub duration_ms: u32,
    pub raw_response: Value,
    pub final_value: Value,
    pub pipeline_steps: u32,
    /// Final URL invoked (with substitutions applied). Surfaced in the
    /// test UI so the user can verify what hit the wire.
    pub effective_url: String,
}

/// Argument envelope for `save_external_source_as_datasource`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveExternalSourceRequest {
    pub source_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
}

/// W37 surface forwarded to chat — one entry per *runnable* external
/// source. Mirrors the existing `AIToolSpec` shape so the same
/// dispatcher can register it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceToolDescriptor {
    pub source_id: String,
    pub tool_name: String,
    pub description: String,
    pub parameters_schema: Value,
}

/// W37: impact preview for the catalog UI before the user disables a
/// source that still has saved datasources originating from it. The
/// shape is intentionally narrow so the React side only renders names
/// and ids — full consumer expansion lives behind the existing
/// `datasourceApi.previewImpact` path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSourceImpactPreview {
    pub source_id: String,
    pub originating_datasources: Vec<OriginatingDatasource>,
    pub has_credential: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginatingDatasource {
    pub datasource_id: Id,
    pub name: String,
    pub workflow_id: Id,
}

impl ExternalSource {
    /// Tool name used by the chat AI tool spec. Stable + prefixed so the
    /// LLM cannot collide with built-in tool names.
    pub fn tool_name(&self) -> String {
        format!("source_{}", self.id)
    }

    /// JSON Schema describing the LLM-callable arguments for this source.
    pub fn tool_parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for param in &self.params {
            let schema = param
                .schema
                .clone()
                .unwrap_or_else(|| serde_json::json!({ "type": "string" }));
            let mut entry = match schema {
                Value::Object(map) => map,
                other => {
                    let mut m = serde_json::Map::new();
                    m.insert("schema".to_string(), other);
                    m
                }
            };
            if !param.description.is_empty() {
                entry
                    .entry("description")
                    .or_insert_with(|| Value::String(param.description.clone()));
            }
            properties.insert(param.name.clone(), Value::Object(entry));
            if param.required {
                required.push(Value::String(param.name.clone()));
            }
        }
        serde_json::json!({
            "type": "object",
            "properties": Value::Object(properties),
            "required": Value::Array(required),
            "additionalProperties": false,
        })
    }

    pub fn is_built_in(&self) -> bool {
        true
    }
}

/// Compact summary returned by `save_external_source_as_datasource` so
/// the UI can navigate to the new entry without re-fetching the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveExternalSourceResult {
    pub source_id: String,
    pub datasource_id: Id,
    pub workflow_id: Id,
}
