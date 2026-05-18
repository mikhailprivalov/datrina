//! W49 context-economy layer.
//!
//! Provider-facing chat context grows fast: every tool result is
//! re-sent on every resume turn, and a 50 KB MCP/HTTP payload anchored
//! in the transcript turns into hundreds of thousands of input tokens
//! after a few corrections. This module is the single place that:
//!
//! - replaces large tool-role message `content` with a compact summary
//!   (status + shape + sampled rows + error class) while the local
//!   transcript keeps the full payload for debug/inspection,
//! - prunes assistant-side reasoning blobs that the provider does not
//!   need to recompute,
//! - drops the oldest non-system turns when total estimated tokens go
//!   past the budget, and inserts an explicit
//!   `[N earlier turns omitted: see local transcript]` marker so the
//!   model cannot mistake the truncated window for an empty history.
//!
//! The compactor is deliberately byte/char based — a real tokenizer is
//! out of scope for this milestone — and conservative: char/4 is the
//! token estimate. That underestimates worst-case multi-byte unicode
//! but matches what OpenAI-style tokenizers produce for the kinds of
//! payloads we encode (JSON + ASCII heavy English/Russian).

use crate::models::chat::{ChatMessage, MessageRole};
use serde_json::Value;

/// Per-turn budget shared by Build and Context chat. Defaults match the
/// W22 footer's expectations: roughly 120k input tokens, with the most
/// recent 6 user/assistant turns reserved untouched.
#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    /// Soft cap on the **provider-visible** message body, in chars.
    /// Char/4 ≈ tokens; 480_000 chars ≈ 120k tokens.
    pub max_total_chars: usize,
    /// Per tool-role message budget after compaction.
    pub tool_summary_max_chars: usize,
    /// Keep the last N user+assistant pairs verbatim no matter what.
    pub keep_recent_turns: usize,
    /// Reasoning blobs older than `keep_recent_turns` are dropped from
    /// the provider context entirely (kept locally). Length cap when we
    /// do include them.
    pub reasoning_max_chars: usize,
    /// Per assistant-message content cap. Long terminal answers are
    /// truncated with an explicit marker so the model knows it has the
    /// summary, not the verbatim earlier reply.
    pub assistant_content_max_chars: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_total_chars: 480_000,
            tool_summary_max_chars: 4_000,
            keep_recent_turns: 6,
            reasoning_max_chars: 2_000,
            assistant_content_max_chars: 12_000,
        }
    }
}

/// Outcome of running [`compact_for_provider`]. The Rust chat command
/// uses `dropped_messages` to decide whether to prepend a synthetic
/// system note explaining the truncation to the model.
#[derive(Debug, Clone)]
pub struct CompactedContext {
    pub messages: Vec<ChatMessage>,
    /// Estimated chars before compaction.
    pub original_chars: usize,
    /// Estimated chars after compaction.
    pub final_chars: usize,
    /// Tool messages whose `content` was rewritten to a compact summary.
    pub tool_summaries_applied: u32,
    /// Non-system messages removed because total chars still exceeded
    /// the budget after summarising. Always paired with a synthetic
    /// `[N earlier turns omitted…]` system marker.
    pub dropped_messages: u32,
}

impl CompactedContext {
    pub fn was_truncated(&self) -> bool {
        self.dropped_messages > 0 || self.tool_summaries_applied > 0
    }
}

/// Approximate token count (char/4 heuristic). Conservative — most
/// English/JSON content sits at ~3.6 chars/token, so this slightly
/// over-estimates and keeps us safely below the provider's hard cap.
pub fn estimate_tokens(chars: usize) -> usize {
    chars / 4
}

fn message_chars(message: &ChatMessage) -> usize {
    let mut total = message.content.chars().count();
    if let Some(metadata) = message.metadata.as_ref() {
        if let Some(reasoning) = metadata.reasoning.as_ref() {
            total = total.saturating_add(reasoning.chars().count());
        }
    }
    total
}

fn total_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_chars).sum()
}

/// W49 entry point. Walks the provider-facing message list and returns
/// a compacted copy plus diagnostics. Idempotent — running it twice on
/// the same input produces the same output (the second pass is a
/// no-op).
pub fn compact_for_provider(messages: Vec<ChatMessage>, budget: ContextBudget) -> CompactedContext {
    let original_chars = total_chars(&messages);
    let mut tool_summaries_applied: u32 = 0;

    // Phase 1: replace big tool-role content with a compact summary.
    let mut working: Vec<ChatMessage> = messages
        .into_iter()
        .map(|mut message| {
            if matches!(message.role, MessageRole::Tool)
                && message.content.chars().count() > budget.tool_summary_max_chars
            {
                let compact =
                    compact_tool_message_content(&message.content, budget.tool_summary_max_chars);
                if compact.len() < message.content.len() {
                    tool_summaries_applied = tool_summaries_applied.saturating_add(1);
                    message.content = compact;
                }
            }
            message
        })
        .collect();

    // Phase 2: cap assistant content and reasoning length.
    for message in working.iter_mut() {
        if matches!(message.role, MessageRole::Assistant)
            && message.content.chars().count() > budget.assistant_content_max_chars
        {
            message.content = truncate_with_marker(
                &message.content,
                budget.assistant_content_max_chars,
                "earlier assistant reply truncated",
            );
        }
        if let Some(metadata) = message.metadata.as_mut() {
            if let Some(reasoning) = metadata.reasoning.as_mut() {
                if reasoning.chars().count() > budget.reasoning_max_chars {
                    *reasoning = truncate_with_marker(
                        reasoning,
                        budget.reasoning_max_chars,
                        "reasoning truncated",
                    );
                }
            }
        }
    }

    let after_summary_chars = total_chars(&working);

    // Phase 3: drop oldest non-system turns when still over budget.
    let mut dropped_messages: u32 = 0;
    if after_summary_chars > budget.max_total_chars {
        let target = budget.max_total_chars;
        let system_count = working
            .iter()
            .filter(|m| matches!(m.role, MessageRole::System))
            .count();
        let keep_tail = recent_turn_tail_index(&working, budget.keep_recent_turns);
        // Iterate from the oldest non-system, non-tail message and drop
        // until under budget. We don't reorder anything — kept messages
        // stay in their original positions; dropped ones are simply
        // omitted from the next provider request.
        let mut current = total_chars(&working);
        let mut keep_flags: Vec<bool> = (0..working.len()).map(|_| true).collect();
        for (idx, message) in working.iter().enumerate() {
            if current <= target {
                break;
            }
            if matches!(message.role, MessageRole::System) {
                continue;
            }
            if idx >= keep_tail {
                break;
            }
            let chars = message_chars(message);
            keep_flags[idx] = false;
            dropped_messages = dropped_messages.saturating_add(1);
            current = current.saturating_sub(chars);
        }
        if dropped_messages > 0 {
            let preserved: Vec<ChatMessage> = working
                .into_iter()
                .zip(keep_flags.into_iter())
                .filter_map(|(message, keep)| if keep { Some(message) } else { None })
                .collect();
            // Splice in the explicit truncation marker right after the
            // existing system block so the model sees it before any
            // user/assistant content.
            working = insert_truncation_marker(preserved, system_count, dropped_messages);
        }
    }

    let final_chars = total_chars(&working);
    CompactedContext {
        messages: working,
        original_chars,
        final_chars,
        tool_summaries_applied,
        dropped_messages,
    }
}

/// Find the index of the oldest message inside the "keep last N turns"
/// tail. Messages below this index are eligible for trimming; messages
/// at or above it are reserved no matter what.
fn recent_turn_tail_index(messages: &[ChatMessage], keep_turns: usize) -> usize {
    if keep_turns == 0 {
        return messages.len();
    }
    let mut seen_assistants: usize = 0;
    for (idx, message) in messages.iter().enumerate().rev() {
        if matches!(message.role, MessageRole::Assistant) {
            seen_assistants = seen_assistants.saturating_add(1);
            if seen_assistants >= keep_turns {
                return idx;
            }
        }
    }
    0
}

fn insert_truncation_marker(
    mut messages: Vec<ChatMessage>,
    system_count: usize,
    dropped: u32,
) -> Vec<ChatMessage> {
    let marker = ChatMessage {
        id: format!("ctx-trunc-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        role: MessageRole::System,
        content: format!(
            "[context_truncated] {} earlier turn(s) were omitted from this request to stay under the per-turn context budget. The local transcript still has them — ask explicitly if you need detail.",
            dropped
        ),
        parts: Vec::new(),
        mode: crate::models::chat::ChatMode::Build,
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    let insert_at = system_count.min(messages.len());
    messages.insert(insert_at, marker);
    messages
}

fn truncate_with_marker(input: &str, max_chars: usize, reason: &str) -> String {
    let total = input.chars().count();
    if total <= max_chars {
        return input.to_string();
    }
    let head_chars = max_chars.saturating_sub(80).max(64);
    let head: String = input.chars().take(head_chars).collect();
    format!(
        "{}\n…[{}: {} chars omitted of {} total]",
        head,
        reason,
        total.saturating_sub(head_chars),
        total
    )
}

/// W49: take a tool-role `content` (stringified JSON of a `Vec<ToolResult>`
/// most of the time) and rewrite it as a compact summary the provider
/// can reason about: per-call status, error class, shape hint, and the
/// first few sampled values. Full payload remains in the local
/// transcript / `message.tool_results` parts.
pub fn compact_tool_message_content(raw: &str, max_chars: usize) -> String {
    // Try parsing as `[{tool_call_id, name, result, error?}]` first;
    // fall back to a generic JSON value if that doesn't match.
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Value::Array(items) = &value {
            let compact: Vec<Value> = items.iter().map(compact_tool_result_value).collect();
            let candidate = Value::Array(compact);
            let s = serde_json::to_string(&candidate).unwrap_or_default();
            if !s.is_empty() && s.chars().count() <= max_chars {
                return s;
            }
            // Still too big — collapse to per-entry single-line summaries.
            let lines: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(idx, item)| short_tool_summary_line(idx, item))
                .collect();
            return enforce_char_cap(lines.join("\n"), max_chars);
        }
        // Single JSON value (legacy paths or non-Vec wire shapes).
        let candidate = compact_json_value(&value, 4, 6, 256);
        return enforce_char_cap(
            serde_json::to_string(&candidate).unwrap_or_default(),
            max_chars,
        );
    }
    // Not JSON at all — straight string truncation with a marker.
    truncate_with_marker(raw, max_chars, "tool result truncated")
}

fn enforce_char_cap(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        truncate_with_marker(&value, max_chars, "tool result truncated")
    }
}

fn compact_tool_result_value(item: &Value) -> Value {
    let obj = match item {
        Value::Object(map) => map,
        other => {
            return Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(
                    "summary".into(),
                    Value::String(format!("non-object tool result: {}", short_describe(other))),
                );
                m
            });
        }
    };
    let mut out = serde_json::Map::new();
    if let Some(name) = obj.get("name") {
        out.insert("name".into(), name.clone());
    }
    if let Some(call_id) = obj.get("tool_call_id") {
        out.insert("tool_call_id".into(), call_id.clone());
    }
    if let Some(error) = obj.get("error") {
        if !matches!(error, Value::Null) {
            out.insert("error".into(), error.clone());
            out.insert("status".into(), Value::String("error".into()));
        }
    }
    let result = obj.get("result").cloned().unwrap_or(Value::Null);
    out.insert("shape".into(), describe_shape(&result));
    let compact_result = compact_json_value(&result, 4, 5, 240);
    out.insert("result_sample".into(), compact_result);
    if !out.contains_key("status") {
        out.insert("status".into(), Value::String("ok".into()));
    }
    Value::Object(out)
}

fn short_tool_summary_line(idx: usize, item: &Value) -> String {
    if let Value::Object(map) = item {
        let name = map
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let status = if matches!(map.get("error"), Some(v) if !v.is_null()) {
            "error"
        } else {
            "ok"
        };
        let shape = describe_shape(map.get("result").unwrap_or(&Value::Null));
        format!("[{idx}] {} {} {}", name, status, shape)
    } else {
        format!("[{idx}] non-object tool result")
    }
}

fn describe_shape(value: &Value) -> Value {
    let s = match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(s) => format!("string({} chars)", s.chars().count()),
        Value::Array(items) => {
            let first_kind = items
                .first()
                .map(short_describe)
                .unwrap_or_else(|| "empty".to_string());
            format!("array(len={}, item={})", items.len(), first_kind)
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

/// Compact a JSON value: cap object key count, array length, string
/// length, recursion depth. Mirrors W23's `prune_value` but lives here
/// so the compactor doesn't depend on the workflow engine.
fn compact_json_value(
    value: &Value,
    max_depth: usize,
    max_array: usize,
    max_string_chars: usize,
) -> Value {
    if max_depth == 0 {
        return Value::String(match value {
            Value::Array(items) => format!("[…{} items]", items.len()),
            Value::Object(map) => format!("{{…{} keys}}", map.len()),
            other => other.to_string(),
        });
    }
    match value {
        Value::String(s) => {
            if s.chars().count() > max_string_chars {
                let head: String = s.chars().take(max_string_chars).collect();
                Value::String(format!("{head}… [{} chars total]", s.chars().count()))
            } else {
                Value::String(s.clone())
            }
        }
        Value::Array(items) => {
            let mut head: Vec<Value> = items
                .iter()
                .take(max_array)
                .map(|item| compact_json_value(item, max_depth - 1, max_array, max_string_chars))
                .collect();
            if items.len() > max_array {
                head.push(Value::String(format!(
                    "… {} more item(s)",
                    items.len() - max_array
                )));
            }
            Value::Array(head)
        }
        Value::Object(map) => {
            let mut pruned = serde_json::Map::new();
            for (k, v) in map.iter() {
                if is_secretish_key(k) {
                    pruned.insert(k.clone(), Value::String("[redacted]".into()));
                    continue;
                }
                pruned.insert(
                    k.clone(),
                    compact_json_value(v, max_depth - 1, max_array, max_string_chars),
                );
            }
            Value::Object(pruned)
        }
        other => other.clone(),
    }
}

/// W49 redaction guard. Tool results sometimes echo provider headers,
/// MCP env values, or auth tokens; we never want those flowing back
/// into the provider context.
fn is_secretish_key(key: &str) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::chat::{ChatMessage, ChatMode, MessageRole};

    fn msg(role: MessageRole, content: &str) -> ChatMessage {
        ChatMessage {
            id: format!("{:?}-{}", role, content.len()),
            role,
            content: content.to_string(),
            parts: Vec::new(),
            mode: ChatMode::Build,
            tool_calls: None,
            tool_results: None,
            metadata: None,
            timestamp: 0,
        }
    }

    #[test]
    fn compact_tool_message_collapses_large_array() {
        // A 50 KB tool result blob should come out under the budget,
        // preserving status + shape + a small sample, not the verbatim
        // array.
        let payload = serde_json::json!([
            {
                "tool_call_id": "t1",
                "name": "http_get",
                "result": (0..500).map(|i| serde_json::json!({
                    "ts": i,
                    "temperature_c": 1.0 + i as f64,
                    "city": "Berlin",
                })).collect::<Vec<_>>(),
                "error": null,
            }
        ]);
        let raw = serde_json::to_string(&payload).unwrap();
        assert!(raw.len() > 20_000, "fixture too small: {}", raw.len());
        let compact = compact_tool_message_content(&raw, 4_000);
        assert!(compact.len() < 4_500, "still too large: {}", compact.len());
        assert!(compact.contains("http_get"));
        assert!(compact.contains("array(len=500"));
    }

    #[test]
    fn compactor_drops_oldest_turns_when_over_budget() {
        // Build a transcript with 20 turns; the recent 4 must survive
        // verbatim, the oldest get replaced by a single truncation
        // marker.
        let mut messages = vec![msg(MessageRole::System, "system primer")];
        for i in 0..20 {
            messages.push(msg(
                MessageRole::User,
                &format!("user turn {i} {}", "X".repeat(5_000)),
            ));
            messages.push(msg(
                MessageRole::Assistant,
                &format!("assistant turn {i} {}", "Y".repeat(5_000)),
            ));
        }
        let budget = ContextBudget {
            max_total_chars: 100_000,
            tool_summary_max_chars: 4_000,
            keep_recent_turns: 4,
            reasoning_max_chars: 1_000,
            assistant_content_max_chars: 50_000,
        };
        let original_len = messages.len();
        let result = compact_for_provider(messages, budget);
        assert!(result.dropped_messages > 0);
        assert!(
            result.final_chars <= budget.max_total_chars + 2_000,
            "final_chars {} not within budget {}",
            result.final_chars,
            budget.max_total_chars
        );
        assert!(result.was_truncated());
        let marker_count = result
            .messages
            .iter()
            .filter(|m| m.content.contains("[context_truncated]"))
            .count();
        assert_eq!(marker_count, 1);
        // System primer is still there.
        assert!(result.messages.iter().any(|m| m.content == "system primer"));
        // The last 4 assistant turns survive verbatim (no truncation marker).
        for i in 16..20 {
            let needle = format!("assistant turn {i}");
            assert!(
                result
                    .messages
                    .iter()
                    .any(|m| m.content.starts_with(&needle)),
                "recent assistant turn {i} should be kept"
            );
        }
        // The oldest turn must have been dropped.
        assert!(!result
            .messages
            .iter()
            .any(|m| m.content.starts_with("user turn 0 ")));
        // Final length is shorter than input.
        assert!(result.messages.len() < original_len);
    }

    #[test]
    fn redacts_secret_keys_inside_tool_results() {
        let payload = serde_json::json!([
            {
                "tool_call_id": "t1",
                "name": "mcp_tool",
                "result": {
                    "url": "https://api.example.com",
                    "Authorization": "Bearer sk-supersecret",
                    "headers": { "x-api-key": "abcd", "ok": true },
                },
                "error": null,
            }
        ]);
        let raw = serde_json::to_string(&payload).unwrap();
        let compact = compact_tool_message_content(&raw, 4_000);
        assert!(!compact.contains("sk-supersecret"));
        assert!(!compact.contains("abcd"));
        assert!(compact.contains("[redacted]"));
    }

    #[test]
    fn compactor_is_idempotent() {
        let messages = vec![
            msg(MessageRole::System, "primer"),
            msg(MessageRole::User, "hello"),
            msg(MessageRole::Assistant, "world"),
        ];
        let budget = ContextBudget::default();
        let first = compact_for_provider(messages, budget);
        let second = compact_for_provider(first.messages.clone(), budget);
        assert_eq!(first.final_chars, second.final_chars);
        assert_eq!(second.dropped_messages, 0);
        assert_eq!(second.tool_summaries_applied, 0);
    }

    #[test]
    fn estimate_tokens_is_conservative() {
        assert_eq!(estimate_tokens(400), 100);
        assert_eq!(estimate_tokens(0), 0);
    }
}
