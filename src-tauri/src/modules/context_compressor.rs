//! W51 provider-context compression and raw-artifact retention.
//!
//! Sits between bulky local tool/MCP/HTTP/datasource/pipeline outputs and
//! the provider message stream. Per profile, returns a typed
//! [`CompressedArtifact`] with:
//!
//! - a small, high-signal compact value the provider sees,
//! - preserved facts (status, identity, shape hints, error class) that
//!   must never be silently dropped,
//! - byte counts + estimated token savings so the chat trace can report
//!   "raw 87 KB → 2.1 KB sent (~96% saved)",
//! - explicit truncation markers the model can call out as detail it can
//!   request via the `inspect_artifact` tool.
//!
//! Compression is loss-aware by design: errors, failed assertions,
//! validation issues, status codes, schema shape, counts, units,
//! timestamps, and provenance survive compaction. Secret-bearing keys
//! are redacted before anything leaves this module — even on the
//! preserved-facts path.
//!
//! Storage and retention are handled by [`crate::modules::storage`];
//! this module is pure (no I/O) so callsites can decide whether the raw
//! artifact is worth persisting.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Char-per-token heuristic shared with W49 [`context_budget`]. Slightly
/// over-estimates English/JSON content but stays conservative for the
/// CJK/Cyrillic strings Datrina actually ships.
const CHARS_PER_TOKEN: usize = 4;

/// Identifies the kind of artifact being compressed. The profile picks
/// which fact-preserving rules apply and how aggressively to summarise
/// the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionProfile {
    /// Generic chat tool result (http_request, recall, dry_run, etc.).
    ChatToolResult,
    /// MCP server tool envelope — unwrap `{ content: [...] }` to the
    /// parsed JSON root before compressing.
    McpToolResult,
    /// `http_request` body + headers.
    HttpResponse,
    /// W37 external-source tool output (HN, Wikipedia, CoinGecko, etc.).
    ExternalSourceResult,
    /// Datasource sample / test run.
    DatasourceSample,
    /// W23 widget pipeline trace.
    PipelineTrace,
    /// Provider-intermediate value the loop generated on its own (plan
    /// echoes, reflection summaries, etc.).
    ProviderIntermediate,
    /// Compression for an error/failure path. Preserves error message
    /// verbatim and skips body summarisation entirely.
    ErrorOrFailure,
}

impl CompressionProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            CompressionProfile::ChatToolResult => "chat_tool_result",
            CompressionProfile::McpToolResult => "mcp_tool_result",
            CompressionProfile::HttpResponse => "http_response",
            CompressionProfile::ExternalSourceResult => "external_source_result",
            CompressionProfile::DatasourceSample => "datasource_sample",
            CompressionProfile::PipelineTrace => "pipeline_trace",
            CompressionProfile::ProviderIntermediate => "provider_intermediate",
            CompressionProfile::ErrorOrFailure => "error_or_failure",
        }
    }

    /// Pick the right profile from a tool name. Unknown tool names map
    /// to `ChatToolResult` rather than `ProviderIntermediate` so we
    /// still apply redaction + array/object pruning.
    pub fn for_tool(tool_name: &str) -> Self {
        match tool_name {
            "http_request" => CompressionProfile::HttpResponse,
            "mcp_tool" => CompressionProfile::McpToolResult,
            "dry_run_widget" => CompressionProfile::DatasourceSample,
            name if name.starts_with("source_") => CompressionProfile::ExternalSourceResult,
            _ => CompressionProfile::ChatToolResult,
        }
    }
}

/// One explicit truncation marker. Surfaces in the compact body so the
/// model knows what it can request via `inspect_artifact`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncationMarker {
    /// Where the omission happened (`array`, `object`, `string`,
    /// `depth`, `lines`).
    pub kind: String,
    /// Human-readable JSON pointer / row range / line range / object
    /// path the agent can quote when calling `inspect_artifact`.
    pub path: String,
    /// What was omitted (e.g. `42 more items`, `2_300 chars`,
    /// `200 lines`).
    pub omitted: String,
}

/// Outcome of a single compression pass. The compact value is what the
/// provider sees; everything else is metadata for the UI, the tool
/// loop, and the raw-artifact retention system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedArtifact {
    pub profile: CompressionProfile,
    /// Provider-visible compact JSON. Always non-null; an error/failure
    /// profile still returns a small object with at least `status` and
    /// `error`.
    pub compact: Value,
    /// Typed facts that must survive compaction (status, identity,
    /// shape, attribution, counts). Surfaced separately so the model
    /// and the UI can reason about them without re-parsing `compact`.
    pub preserved_facts: Map<String, Value>,
    pub truncation: Vec<TruncationMarker>,
    pub raw_bytes: usize,
    pub compact_bytes: usize,
    /// Optional pointer into [`crate::modules::storage`]'s
    /// `raw_artifacts` table. `None` when the callsite decided the raw
    /// payload was not worth persisting (already small, sensitive, etc.).
    pub raw_artifact_ref: Option<String>,
    /// Cheap byte/4 token estimate of `raw_bytes - compact_bytes`. Used
    /// by the chat trace to show "saved ~N tokens".
    pub estimated_tokens_saved: usize,
}

impl CompressedArtifact {
    pub fn reduction_ratio(&self) -> f32 {
        if self.raw_bytes == 0 {
            return 0.0;
        }
        let saved = self.raw_bytes.saturating_sub(self.compact_bytes) as f32;
        saved / self.raw_bytes as f32
    }

    /// Attach a `raw_artifact_ref` after persistence. Returns the
    /// updated artifact for fluent chaining.
    pub fn with_raw_artifact_ref(mut self, id: String) -> Self {
        self.raw_artifact_ref = Some(id);
        self
    }

    /// Serialise the compact body plus truncation hints into the JSON
    /// blob the provider will actually see in the tool message content.
    pub fn provider_payload(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "_compressed".into(),
            Value::String(self.profile.as_str().into()),
        );
        out.insert(
            "preserved".into(),
            Value::Object(self.preserved_facts.clone()),
        );
        out.insert("compact".into(), self.compact.clone());
        if !self.truncation.is_empty() {
            out.insert(
                "truncation".into(),
                json!(self
                    .truncation
                    .iter()
                    .map(|marker| {
                        json!({
                            "kind": marker.kind,
                            "path": marker.path,
                            "omitted": marker.omitted,
                        })
                    })
                    .collect::<Vec<_>>()),
            );
        }
        out.insert("raw_bytes".into(), Value::Number(self.raw_bytes.into()));
        out.insert(
            "compact_bytes".into(),
            Value::Number(self.compact_bytes.into()),
        );
        if let Some(id) = self.raw_artifact_ref.as_deref() {
            out.insert("raw_artifact_id".into(), Value::String(id.into()));
            out.insert(
                "_hint".into(),
                Value::String(format!(
                    "call inspect_artifact(artifact_id=\"{id}\", path=\"<json pointer>\") for bounded raw detail."
                )),
            );
        }
        Value::Object(out)
    }
}

/// Per-call compression limits. Defaults match RTK-class targets:
/// median 60-90% reduction on representative Datrina workloads while
/// keeping enough sample rows that the model can still reason about
/// shape.
#[derive(Debug, Clone, Copy)]
pub struct CompressionLimits {
    pub max_depth: usize,
    pub max_array_items: usize,
    pub max_object_keys: usize,
    pub max_string_chars: usize,
    pub max_log_lines: usize,
    pub max_total_chars: usize,
}

impl CompressionLimits {
    pub fn for_profile(profile: CompressionProfile) -> Self {
        match profile {
            CompressionProfile::ErrorOrFailure => Self {
                max_depth: 6,
                max_array_items: 10,
                max_object_keys: 30,
                max_string_chars: 4_000,
                max_log_lines: 40,
                max_total_chars: 4_000,
            },
            CompressionProfile::PipelineTrace => Self {
                max_depth: 6,
                max_array_items: 12,
                max_object_keys: 40,
                max_string_chars: 1_200,
                max_log_lines: 40,
                max_total_chars: 3_000,
            },
            CompressionProfile::DatasourceSample => Self {
                max_depth: 5,
                max_array_items: 6,
                max_object_keys: 30,
                max_string_chars: 600,
                max_log_lines: 40,
                max_total_chars: 2_400,
            },
            _ => Self {
                max_depth: 5,
                max_array_items: 6,
                max_object_keys: 24,
                max_string_chars: 600,
                max_log_lines: 40,
                max_total_chars: 2_400,
            },
        }
    }
}

/// Top-level entry point. Pure, no I/O. The caller decides whether to
/// persist the raw payload (and call [`CompressedArtifact::with_raw_artifact_ref`]).
pub fn compress(profile: CompressionProfile, raw: &Value) -> CompressedArtifact {
    let limits = CompressionLimits::for_profile(profile);
    let raw_bytes = json_byte_size(raw);
    let mut markers: Vec<TruncationMarker> = Vec::new();
    let mut facts: Map<String, Value> = Map::new();

    let compact = match profile {
        CompressionProfile::McpToolResult => compress_mcp(raw, &limits, &mut markers, &mut facts),
        CompressionProfile::HttpResponse => compress_http(raw, &limits, &mut markers, &mut facts),
        CompressionProfile::PipelineTrace => {
            compress_pipeline_trace(raw, &limits, &mut markers, &mut facts)
        }
        CompressionProfile::ErrorOrFailure => {
            compress_error(raw, &limits, &mut markers, &mut facts)
        }
        _ => compress_generic_with_facts(raw, &limits, &mut markers, &mut facts),
    };

    let compact_bytes = json_byte_size(&compact);
    let estimated_tokens_saved = raw_bytes
        .saturating_sub(compact_bytes)
        .saturating_div(CHARS_PER_TOKEN);

    CompressedArtifact {
        profile,
        compact,
        preserved_facts: facts,
        truncation: markers,
        raw_bytes,
        compact_bytes,
        raw_artifact_ref: None,
        estimated_tokens_saved,
    }
}

/// Loss-aware compression for log/text bodies. Detects repetitive lines
/// and preserves errors/warnings + the first/last informative lines.
pub fn compress_text_log(raw: &str, limits: &CompressionLimits) -> (Value, Vec<TruncationMarker>) {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.len() <= limits.max_log_lines {
        return (Value::String(redact_text(raw)), Vec::new());
    }
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for line in &lines {
        let lc = line.to_ascii_lowercase();
        if lc.contains("error") || lc.contains("panic") || lc.contains("assertion failed") {
            if errors.len() < 12 {
                errors.push(redact_text(line));
            }
        } else if lc.contains("warn") && warnings.len() < 6 {
            warnings.push(redact_text(line));
        }
    }
    let head: Vec<String> = lines.iter().take(8).map(|line| redact_text(line)).collect();
    let tail: Vec<String> = lines
        .iter()
        .rev()
        .take(8)
        .map(|line| redact_text(line))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let omitted = lines.len().saturating_sub(head.len() + tail.len());
    let summary = json!({
        "total_lines": lines.len(),
        "head": head,
        "errors": errors,
        "warnings": warnings,
        "tail": tail,
    });
    let markers = vec![TruncationMarker {
        kind: "lines".into(),
        path: format!("lines[{}..{}]", 8, lines.len().saturating_sub(8)),
        omitted: format!("{omitted} lines omitted between head and tail"),
    }];
    (summary, markers)
}

fn compress_generic_with_facts(
    raw: &Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
    facts: &mut Map<String, Value>,
) -> Value {
    facts.insert("shape".into(), describe_shape_value(raw));
    if let Value::Array(items) = raw {
        facts.insert(
            "row_count".into(),
            Value::Number(serde_json::Number::from(items.len())),
        );
        if let Some(first) = items.first() {
            facts.insert("first_kind".into(), describe_shape_value(first));
        }
    }
    if let Value::Object(map) = raw {
        facts.insert(
            "key_count".into(),
            Value::Number(serde_json::Number::from(map.len())),
        );
        if let Some(status) = map.get("status").cloned() {
            facts.insert("status".into(), status);
        }
        if let Some(error) = map.get("error").cloned() {
            if !matches!(error, Value::Null) {
                facts.insert("error".into(), error);
            }
        }
    }
    let compact = compact_value(raw, limits, "$", markers, 0);
    enforce_compact_size(compact, limits, markers)
}

fn compress_mcp(
    raw: &Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
    facts: &mut Map<String, Value>,
) -> Value {
    // Unwrap the MCP `{ content: [{type, text|json}, ...] }` envelope
    // when present so the model sees the parsed inner value rather than
    // the wire-format text wrapper.
    let unwrapped = mcp_unwrap_envelope(raw, facts);
    facts.insert("shape".into(), describe_shape_value(&unwrapped));
    let compact = compact_value(&unwrapped, limits, "$", markers, 0);
    enforce_compact_size(compact, limits, markers)
}

fn mcp_unwrap_envelope(raw: &Value, facts: &mut Map<String, Value>) -> Value {
    let Value::Object(map) = raw else {
        return raw.clone();
    };
    let Some(Value::Array(items)) = map.get("content") else {
        return raw.clone();
    };
    facts.insert(
        "envelope".into(),
        Value::String(format!("mcp_content(len={})", items.len())),
    );
    let mut unwrapped: Vec<Value> = Vec::new();
    for item in items {
        let Value::Object(inner) = item else {
            unwrapped.push(item.clone());
            continue;
        };
        match inner.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = inner.get("text").and_then(Value::as_str) {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        unwrapped.push(parsed);
                    } else {
                        unwrapped.push(Value::String(text.into()));
                    }
                } else {
                    unwrapped.push(Value::Object(inner.clone()));
                }
            }
            Some("json") => {
                if let Some(value) = inner.get("json").cloned() {
                    unwrapped.push(value);
                } else {
                    unwrapped.push(Value::Object(inner.clone()));
                }
            }
            _ => unwrapped.push(Value::Object(inner.clone())),
        }
    }
    if unwrapped.len() == 1 {
        unwrapped.into_iter().next().unwrap()
    } else {
        Value::Array(unwrapped)
    }
}

fn compress_http(
    raw: &Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
    facts: &mut Map<String, Value>,
) -> Value {
    let Value::Object(map) = raw else {
        return compress_generic_with_facts(raw, limits, markers, facts);
    };
    // Common shapes from `ToolEngine::http_request`:
    //   { status, headers, body, duration_ms } — preserve identity bits
    //   as typed facts so the model sees them outside the truncated body.
    if let Some(status) = map.get("status").cloned() {
        facts.insert("http_status".into(), status);
    }
    if let Some(duration) = map.get("duration_ms").cloned() {
        facts.insert("duration_ms".into(), duration);
    }
    if let Some(url) = map.get("url").cloned() {
        facts.insert("url".into(), url);
    }
    if let Some(method) = map.get("method").cloned() {
        facts.insert("method".into(), method);
    }
    // headers — redact secret keys but keep shape (content-type etc.).
    if let Some(Value::Object(headers)) = map.get("headers") {
        let mut redacted = Map::new();
        for (k, v) in headers {
            if is_secretish_key(k) {
                redacted.insert(k.clone(), Value::String("[redacted]".into()));
            } else {
                redacted.insert(k.clone(), v.clone());
            }
        }
        facts.insert("headers".into(), Value::Object(redacted));
    }
    let body = map.get("body").cloned().unwrap_or(Value::Null);
    facts.insert("body_shape".into(), describe_shape_value(&body));
    let compact = compact_value(&body, limits, "$.body", markers, 0);
    enforce_compact_size(compact, limits, markers)
}

fn compress_pipeline_trace(
    raw: &Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
    facts: &mut Map<String, Value>,
) -> Value {
    let Value::Array(steps) = raw else {
        return compress_generic_with_facts(raw, limits, markers, facts);
    };
    facts.insert(
        "step_count".into(),
        Value::Number(serde_json::Number::from(steps.len())),
    );
    let mut first_empty_idx: Option<usize> = None;
    let mut first_error_idx: Option<usize> = None;
    let mut total_duration: u64 = 0;
    let mut compact_steps: Vec<Value> = Vec::with_capacity(steps.len().min(limits.max_array_items));
    for (idx, step) in steps.iter().enumerate() {
        let Value::Object(map) = step else {
            compact_steps.push(step.clone());
            continue;
        };
        let kind = map
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".into());
        let status = map
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string);
        let error = map.get("error").cloned();
        let duration = map.get("duration_ms").and_then(Value::as_u64).unwrap_or(0);
        total_duration = total_duration.saturating_add(duration);
        let output_shape = describe_shape_value(map.get("output").unwrap_or(&Value::Null));
        let row_count = match map.get("output") {
            Some(Value::Array(items)) => Some(items.len()),
            _ => None,
        };
        if row_count == Some(0) && first_empty_idx.is_none() {
            first_empty_idx = Some(idx);
        }
        if error.is_some() && !matches!(error, Some(Value::Null)) && first_error_idx.is_none() {
            first_error_idx = Some(idx);
        }
        let mut entry = Map::new();
        entry.insert("idx".into(), Value::Number(serde_json::Number::from(idx)));
        entry.insert("kind".into(), Value::String(kind));
        if let Some(status) = status {
            entry.insert("status".into(), Value::String(status));
        }
        if !matches!(error, Some(Value::Null) | None) {
            entry.insert("error".into(), error.unwrap_or(Value::Null));
        }
        entry.insert("duration_ms".into(), Value::Number(duration.into()));
        entry.insert("output_shape".into(), output_shape);
        if let Some(count) = row_count {
            entry.insert(
                "row_count".into(),
                Value::Number(serde_json::Number::from(count)),
            );
        }
        if compact_steps.len() < limits.max_array_items {
            compact_steps.push(Value::Object(entry));
        } else if compact_steps.len() == limits.max_array_items {
            markers.push(TruncationMarker {
                kind: "array".into(),
                path: format!("$.steps[{}..]", limits.max_array_items),
                omitted: format!("{} step(s) omitted", steps.len() - limits.max_array_items),
            });
        }
    }
    facts.insert(
        "total_duration_ms".into(),
        Value::Number(total_duration.into()),
    );
    if let Some(idx) = first_empty_idx {
        facts.insert(
            "first_empty_step_idx".into(),
            Value::Number(serde_json::Number::from(idx)),
        );
    }
    if let Some(idx) = first_error_idx {
        facts.insert(
            "first_error_step_idx".into(),
            Value::Number(serde_json::Number::from(idx)),
        );
    }
    enforce_compact_size(Value::Array(compact_steps), limits, markers)
}

fn compress_error(
    raw: &Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
    facts: &mut Map<String, Value>,
) -> Value {
    facts.insert("shape".into(), describe_shape_value(raw));
    if let Value::Object(map) = raw {
        if let Some(error) = map.get("error").cloned() {
            facts.insert("error".into(), error);
        }
        if let Some(status) = map.get("status").cloned() {
            facts.insert("status".into(), status);
        }
    }
    // Errors stay verbatim within the depth/length caps — losing the
    // failure detail defeats the point.
    let compact = compact_value(raw, limits, "$", markers, 0);
    enforce_compact_size(compact, limits, markers)
}

fn compact_value(
    value: &Value,
    limits: &CompressionLimits,
    path: &str,
    markers: &mut Vec<TruncationMarker>,
    depth: usize,
) -> Value {
    if depth >= limits.max_depth {
        markers.push(TruncationMarker {
            kind: "depth".into(),
            path: path.into(),
            omitted: format!("subtree past depth {}", limits.max_depth),
        });
        return Value::String(format!("[depth_truncated at {path}]"));
    }
    match value {
        Value::String(text) => {
            let chars = text.chars().count();
            if chars > limits.max_string_chars {
                markers.push(TruncationMarker {
                    kind: "string".into(),
                    path: path.into(),
                    omitted: format!("{} chars", chars - limits.max_string_chars),
                });
                let head: String = text.chars().take(limits.max_string_chars).collect();
                Value::String(format!("{}… [{} chars total]", redact_text(&head), chars))
            } else {
                Value::String(redact_text(text))
            }
        }
        Value::Array(items) => {
            let take = limits.max_array_items.min(items.len());
            let mut out: Vec<Value> = items
                .iter()
                .take(take)
                .enumerate()
                .map(|(idx, item)| {
                    compact_value(item, limits, &format!("{path}[{idx}]"), markers, depth + 1)
                })
                .collect();
            if items.len() > take {
                markers.push(TruncationMarker {
                    kind: "array".into(),
                    path: path.into(),
                    omitted: format!("{} of {} items", items.len() - take, items.len()),
                });
                out.push(Value::String(format!(
                    "… {} more item(s) — request via inspect_artifact",
                    items.len() - take
                )));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = Map::new();
            let mut taken = 0usize;
            for (k, v) in map {
                if is_secretish_key(k) {
                    out.insert(k.clone(), Value::String("[redacted]".into()));
                    continue;
                }
                if taken >= limits.max_object_keys {
                    markers.push(TruncationMarker {
                        kind: "object".into(),
                        path: path.into(),
                        omitted: format!(
                            "{} keys omitted at {path}",
                            map.len() - limits.max_object_keys
                        ),
                    });
                    break;
                }
                let child_path = format!("{path}.{k}");
                out.insert(
                    k.clone(),
                    compact_value(v, limits, &child_path, markers, depth + 1),
                );
                taken += 1;
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

fn enforce_compact_size(
    value: Value,
    limits: &CompressionLimits,
    markers: &mut Vec<TruncationMarker>,
) -> Value {
    let encoded = serde_json::to_string(&value).unwrap_or_default();
    if encoded.chars().count() <= limits.max_total_chars {
        return value;
    }
    // Hard fallback: re-emit a brutal one-line summary so the provider
    // never receives more than max_total_chars from the compressor.
    let shape = describe_shape_value(&value);
    markers.push(TruncationMarker {
        kind: "hard_cap".into(),
        path: "$".into(),
        omitted: format!(
            "{} chars (above per-call cap {})",
            encoded.chars().count(),
            limits.max_total_chars
        ),
    });
    json!({
        "_compact_overflow": true,
        "shape": shape,
        "char_count": encoded.chars().count(),
        "max_total_chars": limits.max_total_chars,
        "hint": "raw payload exceeded per-call compact cap; call inspect_artifact for bounded slices",
    })
}

fn describe_shape_value(value: &Value) -> Value {
    let s = match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(s) => format!("string({} chars)", s.chars().count()),
        Value::Array(items) => {
            let first = items
                .first()
                .map(short_describe)
                .unwrap_or_else(|| "empty".to_string());
            format!("array(len={}, item={})", items.len(), first)
        }
        Value::Object(map) => format!("object(keys={})", map.len()),
    };
    Value::String(s)
}

fn short_describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
    }
}

fn json_byte_size(value: &Value) -> usize {
    serde_json::to_string(value).map(|s| s.len()).unwrap_or(0)
}

/// W51 redaction guard. Mirrors [`crate::modules::context_budget`]'s
/// guard so we redact at both layers and never accidentally pass an
/// `Authorization: Bearer …` header through a custom profile.
pub fn is_secretish_key(key: &str) -> bool {
    let lc = key.to_ascii_lowercase();
    matches!(
        lc.as_str(),
        "authorization"
            | "auth"
            | "cookie"
            | "set-cookie"
            | "api_key"
            | "api-key"
            | "apikey"
            | "openai_api_key"
            | "openrouter_api_key"
            | "anthropic_api_key"
            | "secret"
            | "client_secret"
            | "password"
            | "token"
            | "access_token"
            | "refresh_token"
            | "private_key"
            | "x-api-key"
    ) || lc.contains("api_key")
        || lc.contains("secret")
        || lc.contains("password")
        || lc.contains("bearer")
}

/// W51 text-level redaction. Detects high-entropy "looks like a bearer
/// token / sk-…" patterns in free text and masks them before either the
/// provider or the local artifact preview see them.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_inclusive(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        if looks_like_secret_token(token.trim_end_matches([' ', '\t', '\n', '"', '\''])) {
            // Keep first 4 chars so the user can identify the kind of
            // token; mask the rest.
            let trimmed = token.trim_end();
            let prefix: String = trimmed.chars().take(4).collect();
            out.push_str(&format!("{prefix}…[redacted]"));
            // Re-add the original trailing separator if any.
            let trailing = &token[trimmed.len()..];
            out.push_str(trailing);
        } else {
            out.push_str(token);
        }
    }
    out
}

fn looks_like_secret_token(candidate: &str) -> bool {
    if candidate.len() < 20 {
        return false;
    }
    if candidate.starts_with("sk-")
        || candidate.starts_with("xoxb-")
        || candidate.starts_with("ghp_")
        || candidate.starts_with("gho_")
        || candidate.starts_with("Bearer ")
    {
        return true;
    }
    // Look for long base64/hex-ish runs with no whitespace.
    let alnum = candidate
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '+' || *c == '/')
        .count();
    alnum == candidate.chars().count() && candidate.len() >= 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_large_array_significantly() {
        let items: Vec<Value> = (0..500)
            .map(|i| json!({"idx": i, "name": format!("row-{i}"), "value": i * 7}))
            .collect();
        let raw = Value::Array(items);
        let artifact = compress(CompressionProfile::ChatToolResult, &raw);
        assert!(artifact.compact_bytes < artifact.raw_bytes / 5);
        assert!(artifact.reduction_ratio() > 0.6);
        assert!(!artifact.truncation.is_empty());
        // The preserved row_count must survive — it's a load-bearing
        // fact for downstream pipeline construction.
        assert_eq!(
            artifact
                .preserved_facts
                .get("row_count")
                .and_then(Value::as_u64),
            Some(500)
        );
    }

    #[test]
    fn mcp_envelope_unwraps_inner_json() {
        let raw = json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "rows": (0..50).map(|i| json!({"i": i})).collect::<Vec<_>>(),
                        "status": "ok",
                    })).unwrap(),
                }
            ]
        });
        let artifact = compress(CompressionProfile::McpToolResult, &raw);
        // After unwrapping the inner JSON, the compact view must be
        // smaller than the raw envelope and still contain a shape hint.
        assert!(artifact.compact_bytes < artifact.raw_bytes);
        let envelope = artifact
            .preserved_facts
            .get("envelope")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(envelope.starts_with("mcp_content("));
    }

    #[test]
    fn http_profile_preserves_status_and_redacts_auth() {
        let raw = json!({
            "method": "GET",
            "url": "https://api.example.com/data",
            "status": 200,
            "duration_ms": 134,
            "headers": {
                "content-type": "application/json",
                "Authorization": "Bearer sk-supersecretvalue123",
            },
            "body": {
                "rows": (0..40).map(|i| json!({"i": i})).collect::<Vec<_>>(),
                "ok": true,
            }
        });
        let artifact = compress(CompressionProfile::HttpResponse, &raw);
        assert_eq!(
            artifact
                .preserved_facts
                .get("http_status")
                .and_then(Value::as_u64),
            Some(200)
        );
        let provider_payload = artifact.provider_payload();
        let encoded = serde_json::to_string(&provider_payload).unwrap();
        assert!(!encoded.contains("sk-supersecretvalue123"));
        assert!(encoded.contains("[redacted]"));
    }

    #[test]
    fn pipeline_trace_marks_first_empty_step() {
        let raw = json!([
            {"kind": "fetch", "status": "ok", "duration_ms": 50, "output": [{"a": 1},{"a": 2}]},
            {"kind": "filter", "status": "ok", "duration_ms": 12, "output": []},
            {"kind": "format", "status": "ok", "duration_ms": 3, "output": []}
        ]);
        let artifact = compress(CompressionProfile::PipelineTrace, &raw);
        assert_eq!(
            artifact
                .preserved_facts
                .get("first_empty_step_idx")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            artifact
                .preserved_facts
                .get("step_count")
                .and_then(Value::as_u64),
            Some(3)
        );
    }

    #[test]
    fn error_profile_keeps_error_message_verbatim() {
        let raw = json!({
            "status": "error",
            "error": "validation_failed: widget pipeline returned an empty array",
        });
        let artifact = compress(CompressionProfile::ErrorOrFailure, &raw);
        let preserved_error = artifact
            .preserved_facts
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(preserved_error.contains("validation_failed"));
        let encoded = serde_json::to_string(&artifact.compact).unwrap();
        assert!(encoded.contains("validation_failed"));
    }

    #[test]
    fn log_compression_keeps_errors_and_collapses_passes() {
        let mut log = String::new();
        for i in 0..400 {
            log.push_str(&format!("pass {i}: noop\n"));
        }
        log.push_str("ERROR: assertion failed at row 42\n");
        for i in 0..400 {
            log.push_str(&format!("pass-tail {i}: noop\n"));
        }
        let limits = CompressionLimits::for_profile(CompressionProfile::ChatToolResult);
        let (summary, markers) = compress_text_log(&log, &limits);
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(encoded.contains("assertion failed"));
        assert!(encoded.len() < log.len() / 5);
        assert!(!markers.is_empty());
    }

    #[test]
    fn redact_text_masks_bearer_tokens_in_free_text() {
        let raw =
            "Got back error: Authorization Bearer sk-supersecretkey1234567890abcd response 401";
        let redacted = redact_text(raw);
        assert!(!redacted.contains("sk-supersecretkey1234567890abcd"));
        // Still contains the status code so debug remains useful.
        assert!(redacted.contains("401"));
    }

    #[test]
    fn provider_payload_attaches_artifact_id_hint() {
        let raw = json!({"rows": [1, 2, 3]});
        let artifact = compress(CompressionProfile::ChatToolResult, &raw)
            .with_raw_artifact_ref("art-xyz".into());
        let payload = artifact.provider_payload();
        let encoded = serde_json::to_string(&payload).unwrap();
        assert!(encoded.contains("art-xyz"));
        assert!(encoded.contains("inspect_artifact"));
    }

    #[test]
    fn profile_for_tool_routes_known_tools() {
        assert_eq!(
            CompressionProfile::for_tool("mcp_tool"),
            CompressionProfile::McpToolResult
        );
        assert_eq!(
            CompressionProfile::for_tool("http_request"),
            CompressionProfile::HttpResponse
        );
        assert_eq!(
            CompressionProfile::for_tool("dry_run_widget"),
            CompressionProfile::DatasourceSample
        );
        assert_eq!(
            CompressionProfile::for_tool("source_hackernews_search"),
            CompressionProfile::ExternalSourceResult
        );
        assert_eq!(
            CompressionProfile::for_tool("recall"),
            CompressionProfile::ChatToolResult
        );
    }
}
