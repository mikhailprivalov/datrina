use super::{Id, Timestamp};
use serde::{Deserialize, Serialize};

/// W20: a saved Data Playground query. Captures the source tool + arguments
/// users keep returning to. Stored in `playground_presets`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaygroundPreset {
    pub id: Id,
    pub tool_kind: PlaygroundToolKind,
    /// MCP server fingerprint; `None` for HTTP / builtin sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    pub tool_name: String,
    pub display_name: String,
    pub arguments: serde_json::Value,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaygroundToolKind {
    Mcp,
    Http,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SavePlaygroundPresetRequest {
    pub tool_kind: PlaygroundToolKind,
    #[serde(default)]
    pub server_id: Option<String>,
    pub tool_name: String,
    pub display_name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}
