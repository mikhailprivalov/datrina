use super::Id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServer {
    pub id: Id,
    pub name: String,
    pub transport: MCPTransport,
    pub is_enabled: bool,
    // stdio transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    // http transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPTransport {
    Stdio,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPTool {
    pub server_id: Id,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequest {
    pub server_id: Id,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerStatus {
    pub id: Id,
    pub status: ConnectionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    Connecting,
    Error,
}
