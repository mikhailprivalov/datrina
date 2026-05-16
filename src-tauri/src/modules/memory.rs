//! W17 agent memory engine.
//!
//! Stores long-lived facts, preferences, lessons, and observed MCP tool
//! shapes in SQLite. Retrieval is FTS5 keyword search plus a small
//! deterministic boost from scope priority and recency; this trades the
//! semantic recall of vector search for zero new runtime dependencies, and
//! is the v1 step on the way to the hybrid retrieval the W17 doc plans.
//!
//! An `Embedder` trait is exposed so the future sqlite-vec/fastembed swap
//! is a single-file substitution.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::models::memory::{
    MemoryHit, MemoryKind, MemoryRecord, RememberRequest, Scope, ToolShape,
};
use crate::modules::storage::Storage;

const MAX_HITS_PER_LEG: usize = 20;

/// Tag used in fingerprint canonicalisation when we want to collapse
/// concrete leaf values into a "shape only" signature.
const SHAPE_PLACEHOLDER_STRING: &str = "<string>";
const SHAPE_PLACEHOLDER_NUMBER: &str = "<number>";
const SHAPE_PLACEHOLDER_BOOL: &str = "<bool>";
const SHAPE_PLACEHOLDER_NULL: &str = "<null>";

#[derive(Clone)]
pub struct MemoryEngine {
    storage: Arc<Storage>,
}

impl MemoryEngine {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub async fn remember(&self, req: RememberRequest) -> Result<MemoryRecord> {
        let now = Utc::now().timestamp_millis();
        let record = MemoryRecord {
            id: uuid::Uuid::new_v4().to_string(),
            scope: req.scope,
            kind: req.kind,
            content: req.content.trim().to_string(),
            metadata: req.metadata.unwrap_or(Value::Object(Default::default())),
            created_at: now,
            accessed_count: 0,
            last_accessed_at: None,
            expires_at: None,
            compressed_into: None,
        };
        if record.content.is_empty() {
            return Err(anyhow::anyhow!("memory content is empty"));
        }
        self.storage.insert_memory(&record).await?;
        Ok(record)
    }

    pub async fn forget(&self, id: &str) -> Result<bool> {
        self.storage.delete_memory(id).await
    }

    pub async fn list(&self) -> Result<Vec<MemoryRecord>> {
        self.storage.list_memories().await
    }

    /// Hybrid retrieval: BM25 keyword search via FTS5 over all scope
    /// filters, then a per-row composite score that adds a small scope
    /// priority boost (dashboard > mcp_server > session > global) and a
    /// recency boost. Returns the top `top_n` after de-dup.
    pub async fn retrieve(
        &self,
        query: &str,
        scopes: &[Scope],
        top_n: usize,
    ) -> Result<Vec<MemoryHit>> {
        if top_n == 0 {
            return Ok(Vec::new());
        }
        let filters: Vec<(String, Option<String>)> = scopes
            .iter()
            .map(|s| {
                (
                    s.discriminator().to_string(),
                    s.scope_id().map(String::from),
                )
            })
            .collect();
        let raw = self
            .storage
            .search_memories_fts(query, &filters, MAX_HITS_PER_LEG)
            .await?;

        // BM25 ranks descend toward more negative values for better
        // matches in SQLite FTS5; normalise to a [0,1) score and fold in
        // scope/recency priors.
        let now = Utc::now().timestamp_millis();
        let mut hits: Vec<MemoryHit> = raw
            .into_iter()
            .map(|(record, rank)| {
                let bm25_score = (-rank).max(0.0); // larger = better
                let scope_boost = scope_priority(&record.scope);
                let age_days = ((now - record.created_at).max(0) as f64) / 86_400_000.0;
                let recency_boost = (-age_days / 30.0).exp(); // half-life ~3w
                let access_boost = (record.accessed_count as f64 + 1.0).ln() * 0.1;
                let score = bm25_score + scope_boost + recency_boost + access_boost;
                MemoryHit { record, score }
            })
            .collect();

        // Stable dedupe by id (FTS could surface the same row twice on
        // term repeats in the query); keep best score.
        let mut best: HashMap<String, MemoryHit> = HashMap::new();
        for hit in hits.drain(..) {
            best.entry(hit.record.id.clone())
                .and_modify(|existing| {
                    if hit.score > existing.score {
                        *existing = hit.clone();
                    }
                })
                .or_insert(hit);
        }
        let mut ranked: Vec<MemoryHit> = best.into_values().collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(top_n);

        // Best-effort touch for analytics; do not fail retrieval if the
        // update is unable to land.
        for hit in &ranked {
            let _ = self.storage.touch_memory_access(&hit.record.id).await;
        }
        Ok(ranked)
    }

    /// Record what a successful MCP tool call returned so the next session
    /// can plan against the same shape without re-discovering it.
    pub async fn observe_tool_shape(
        &self,
        server_id: &str,
        tool_name: &str,
        args: &Value,
        result: &Value,
    ) -> Result<()> {
        let fingerprint = args_fingerprint(args);
        let shape_summary = summarise_shape(result, 4, 32);
        let shape_full = serde_json::to_string(&type_sketch(result, 5, 24)).unwrap_or_default();
        let now = Utc::now().timestamp_millis();
        let shape = ToolShape {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: server_id.to_string(),
            tool_name: tool_name.to_string(),
            args_fingerprint: fingerprint,
            shape_summary: shape_summary.clone(),
            shape_full,
            sample_path: pick_sample_path(result),
            observed_at: now,
            observation_count: 1,
        };
        self.storage.upsert_tool_shape(&shape).await?;

        // Also surface this as a retrievable memory so RAG hits it.
        let content = format!(
            "MCP tool `{}` on server `{}` returns {}.",
            tool_name, server_id, shape_summary
        );
        let req = RememberRequest {
            scope: Scope::McpServer(server_id.to_string()),
            kind: MemoryKind::ToolShape,
            content,
            metadata: Some(serde_json::json!({
                "tool_name": tool_name,
                "server_id": server_id,
            })),
        };
        // Don't fail the surrounding chat turn if the memory write fails.
        if let Err(e) = self.remember(req).await {
            tracing::warn!("memory: tool-shape remember failed: {}", e);
        }
        Ok(())
    }

    pub async fn lookup_tool_shape(
        &self,
        server_id: &str,
        tool_name: &str,
        args: &Value,
    ) -> Option<ToolShape> {
        let fingerprint = args_fingerprint(args);
        self.storage
            .lookup_tool_shape(server_id, tool_name, &fingerprint)
            .await
            .ok()
            .flatten()
    }

    pub async fn list_tool_shapes(&self, server_id: &str) -> Result<Vec<ToolShape>> {
        self.storage
            .list_tool_shapes_for_server(server_id, 64)
            .await
    }
}

fn scope_priority(scope: &Scope) -> f64 {
    match scope {
        Scope::Dashboard(_) => 1.6,
        Scope::McpServer(_) => 1.2,
        Scope::Session(_) => 0.8,
        Scope::Global => 0.4,
    }
}

/// Canonical sha1 of an args object with concrete leaf values replaced by
/// their type tag. `{a: 1, b: "x"}` and `{a: 2, b: "y"}` collide; `{a: 1}`
/// and `{a: "1"}` don't.
pub fn args_fingerprint(args: &Value) -> String {
    let mut canonical = String::new();
    walk_for_fingerprint(args, &mut canonical);
    let mut hasher = Sha1::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn walk_for_fingerprint(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str(SHAPE_PLACEHOLDER_NULL),
        Value::Bool(_) => out.push_str(SHAPE_PLACEHOLDER_BOOL),
        Value::Number(_) => out.push_str(SHAPE_PLACEHOLDER_NUMBER),
        Value::String(_) => out.push_str(SHAPE_PLACEHOLDER_STRING),
        Value::Array(items) => {
            out.push('[');
            if let Some(first) = items.first() {
                walk_for_fingerprint(first, out);
                if items.len() > 1 {
                    out.push_str(",...");
                }
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(key);
                out.push(':');
                walk_for_fingerprint(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// One-line human-readable summary like:
/// `object with keys: data.items[*].{id,name,version}; pagination.{next,total}`.
pub fn summarise_shape(value: &Value, max_depth: usize, max_keys: usize) -> String {
    let mut paths: Vec<String> = Vec::new();
    collect_leaf_paths(value, "", 0, max_depth, &mut paths);
    paths.sort();
    paths.dedup();
    if paths.len() > max_keys {
        paths.truncate(max_keys);
        paths.push("...".to_string());
    }
    let kind = describe_root_kind(value);
    if paths.is_empty() {
        kind
    } else {
        format!("{} ({})", kind, paths.join(", "))
    }
}

fn describe_root_kind(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => format!("array[{}]", items.len()),
        Value::Object(map) => format!("object with {} keys", map.len()),
    }
}

fn collect_leaf_paths(
    value: &Value,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<String>,
) {
    if depth >= max_depth {
        if !prefix.is_empty() {
            out.push(format!("{prefix}: ..."));
        }
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter() {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_leaf_paths(inner, &next, depth + 1, max_depth, out);
            }
        }
        Value::Array(items) => {
            if let Some(first) = items.first() {
                let next = format!("{prefix}[*]");
                collect_leaf_paths(first, &next, depth + 1, max_depth, out);
            } else if !prefix.is_empty() {
                out.push(format!("{prefix}: []"));
            }
        }
        leaf => {
            if !prefix.is_empty() {
                out.push(format!("{prefix}: {}", describe_leaf(leaf)));
            }
        }
    }
}

fn describe_leaf(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        _ => "value",
    }
}

/// Heuristic: if the result has a single deeply-nested array of objects,
/// return its dotted path so the agent can wire up `output_path`/`pick`
/// without re-exploring the shape.
fn pick_sample_path(value: &Value) -> Option<String> {
    fn find_array(value: &Value, prefix: &str, depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        match value {
            Value::Array(items) if items.iter().any(|i| i.is_object()) => Some(prefix.to_string()),
            Value::Object(map) => {
                let mut found: Option<String> = None;
                for (k, v) in map.iter() {
                    let next = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    if let Some(path) = find_array(v, &next, depth + 1) {
                        if found.is_none()
                            || path.split('.').count() < found.as_ref().unwrap().split('.').count()
                        {
                            found = Some(path);
                        }
                    }
                }
                found
            }
            _ => None,
        }
    }
    find_array(value, "", 0)
}

/// Build a pruned JSON shape sketch suitable for storage/inspection.
fn type_sketch(value: &Value, max_depth: usize, max_keys: usize) -> Value {
    if max_depth == 0 {
        return Value::String("...".into());
    }
    match value {
        Value::Null => Value::String(SHAPE_PLACEHOLDER_NULL.into()),
        Value::Bool(_) => Value::String(SHAPE_PLACEHOLDER_BOOL.into()),
        Value::Number(_) => Value::String(SHAPE_PLACEHOLDER_NUMBER.into()),
        Value::String(_) => Value::String(SHAPE_PLACEHOLDER_STRING.into()),
        Value::Array(items) => {
            if let Some(first) = items.first() {
                Value::Array(vec![type_sketch(first, max_depth - 1, max_keys)])
            } else {
                Value::Array(Vec::new())
            }
        }
        Value::Object(map) => {
            let mut next = serde_json::Map::new();
            for (idx, (key, inner)) in map.iter().enumerate() {
                if idx >= max_keys {
                    next.insert("_truncated".into(), Value::Bool(true));
                    break;
                }
                next.insert(key.clone(), type_sketch(inner, max_depth - 1, max_keys));
            }
            Value::Object(next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_fingerprint_collapses_leaf_values() {
        let a = serde_json::json!({"limit": 10, "filter": "active"});
        let b = serde_json::json!({"limit": 9999, "filter": "stale"});
        let c = serde_json::json!({"limit": "10", "filter": "active"});
        assert_eq!(args_fingerprint(&a), args_fingerprint(&b));
        assert_ne!(args_fingerprint(&a), args_fingerprint(&c));
    }

    #[test]
    fn summarise_shape_lists_dotted_paths() {
        let value = serde_json::json!({
            "data": {"items": [{"id": 1, "name": "x"}]},
            "page": 1,
        });
        let summary = summarise_shape(&value, 4, 16);
        assert!(summary.contains("data.items[*].id"), "got: {summary}");
        assert!(summary.contains("data.items[*].name"), "got: {summary}");
    }
}
