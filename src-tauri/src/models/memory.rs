use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};

/// Scope at which a memory applies. `Global` is always-on; `Dashboard` and
/// `McpServer` are matched against the current chat session; `Session`
/// memories never persist beyond their originating session and are mostly
/// reserved for short-lived facts the agent records mid-turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Scope {
    Global,
    Dashboard(Id),
    McpServer(Id),
    Session(Id),
}

impl Scope {
    pub fn discriminator(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Dashboard(_) => "dashboard",
            Scope::McpServer(_) => "mcp_server",
            Scope::Session(_) => "session",
        }
    }

    pub fn scope_id(&self) -> Option<&str> {
        match self {
            Scope::Global => None,
            Scope::Dashboard(id) | Scope::McpServer(id) | Scope::Session(id) => Some(id.as_str()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    ToolShape,
    Lesson,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Fact => "fact",
            MemoryKind::Preference => "preference",
            MemoryKind::ToolShape => "tool_shape",
            MemoryKind::Lesson => "lesson",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "preference" => MemoryKind::Preference,
            "tool_shape" => MemoryKind::ToolShape,
            "lesson" => MemoryKind::Lesson,
            _ => MemoryKind::Fact,
        }
    }
}

/// A single persisted memory row. The hybrid retrieval view is a small
/// projection on top of this structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: Id,
    pub scope: Scope,
    pub kind: MemoryKind,
    pub content: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub created_at: Timestamp,
    pub accessed_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_into: Option<Id>,
}

/// What the retrieval API returns: a memory record plus the relevance
/// score that ranked it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub score: f64,
}

/// Observed shape for an MCP tool. The fingerprint collapses argument
/// *types*, so two calls with the same arg structure share a row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolShape {
    pub id: Id,
    pub server_id: Id,
    pub tool_name: String,
    pub args_fingerprint: String,
    pub shape_summary: String,
    pub shape_full: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_path: Option<String>,
    pub observed_at: Timestamp,
    pub observation_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberRequest {
    pub scope: Scope,
    pub kind: MemoryKind,
    pub content: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    #[serde(default)]
    pub dashboard_id: Option<Id>,
    #[serde(default)]
    pub mcp_server_ids: Vec<Id>,
    #[serde(default)]
    pub session_id: Option<Id>,
    #[serde(default = "default_top_n")]
    pub top_n: usize,
}

fn default_top_n() -> usize {
    5
}
