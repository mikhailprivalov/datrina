use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::info;

use crate::models::mcp::{ConnectionStatus, MCPServer, MCPServerStatus, MCPTool, MCPTransport};
use crate::models::Id;
use crate::modules::tool_engine::DEFAULT_USER_AGENT;

const INIT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Active MCP connection with a server. The two transports keep their own
/// state because their I/O models are incompatible (long-lived process vs.
/// short-lived HTTP requests).
enum MCPConnection {
    Stdio {
        server: MCPServer,
        process: Child,
        request_id: u64,
        tools: Vec<MCPTool>,
    },
    Http {
        server: MCPServer,
        client: reqwest::Client,
        url: String,
        session_id: Option<String>,
        request_id: u64,
        tools: Vec<MCPTool>,
    },
}

impl MCPConnection {
    fn server(&self) -> &MCPServer {
        match self {
            Self::Stdio { server, .. } | Self::Http { server, .. } => server,
        }
    }

    fn tools(&self) -> &[MCPTool] {
        match self {
            Self::Stdio { tools, .. } | Self::Http { tools, .. } => tools,
        }
    }

    fn next_request_id(&mut self) -> u64 {
        match self {
            Self::Stdio { request_id, .. } | Self::Http { request_id, .. } => {
                let id = *request_id;
                *request_id += 1;
                id
            }
        }
    }
}

/// Manages MCP server lifecycle and tool execution
pub struct MCPManager {
    connections: Mutex<HashMap<Id, MCPConnection>>,
}

impl MCPManager {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to an MCP server. Picks the transport based on
    /// `server.transport`.
    pub async fn connect(&self, server: MCPServer) -> Result<Vec<MCPTool>> {
        // Disconnect existing if any
        self.disconnect(&server.id).await.ok();

        match server.transport {
            MCPTransport::Stdio => self.connect_stdio(&server).await,
            MCPTransport::Http => self.connect_http(&server).await,
        }
    }

    async fn connect_stdio(&self, server: &MCPServer) -> Result<Vec<MCPTool>> {
        let command = server
            .command
            .as_ref()
            .ok_or_else(|| anyhow!("No command specified for stdio transport"))?;

        let args = server.args.as_ref().map(|a| a.as_slice()).unwrap_or(&[]);

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Set environment variables
        if let Some(env) = &server.env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }

        let mut process = cmd
            .spawn()
            .map_err(|e| anyhow!("Failed to spawn MCP server {}: {}", server.name, e))?;

        info!(
            "🚀 MCP server '{}' started (pid: {:?})",
            server.name,
            process.id()
        );

        // Send initialize request
        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "datrina",
                    "version": "0.1.0"
                }
            }
        });

        let stdin = process
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("Cannot access stdin"))?;

        let stdin_json = serde_json::to_string(&init_request)?;
        stdin.write_all(stdin_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        // Read initialize response
        let stdout = process
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("Cannot access stdout"))?;
        let mut reader = BufReader::new(stdout).lines();

        // Wait for initialize response with timeout
        let response = tokio::time::timeout(INIT_TIMEOUT, reader.next_line()).await;

        match response {
            Ok(Ok(Some(line))) => {
                let parsed: Value = serde_json::from_str(&line)?;
                if parsed.get("error").is_some() {
                    let _ = process.kill().await;
                    return Err(anyhow!("MCP initialize error: {}", line));
                }
                if parsed.get("result").is_none() {
                    let _ = process.kill().await;
                    return Err(anyhow!("MCP initialize response missing result"));
                }
            }
            Ok(Ok(None)) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP server closed connection during init"));
            }
            Ok(Err(e)) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP initialize read error: {}", e));
            }
            Err(_) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP initialize timeout"));
            }
        }

        // Send initialized notification
        let init_notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let notif_json = serde_json::to_string(&init_notif)?;
        stdin.write_all(notif_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        // Discover tools
        let tools_request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });

        let tools_json = serde_json::to_string(&tools_request)?;
        stdin.write_all(tools_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        let tools_response = tokio::time::timeout(INIT_TIMEOUT, reader.next_line()).await;

        let tools = match tools_response {
            Ok(Ok(Some(line))) => {
                let parsed: Value = serde_json::from_str(&line)?;
                parse_tools_list(&server.id, &parsed).map_err(|e| {
                    // Best-effort cleanup on parse failure handled outside the
                    // borrow scope below.
                    e
                })?
            }
            Ok(Ok(None)) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP server closed connection during tools/list"));
            }
            Ok(Err(e)) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP tools/list read error: {}", e));
            }
            Err(_) => {
                let _ = process.kill().await;
                return Err(anyhow!("MCP tools/list timeout"));
            }
        };

        info!(
            "📡 MCP '{}' (stdio) connected with {} tools",
            server.name,
            tools.len()
        );

        let connection = MCPConnection::Stdio {
            server: server.clone(),
            process,
            request_id: 3,
            tools: tools.clone(),
        };

        self.connections
            .lock()
            .await
            .insert(server.id.clone(), connection);

        Ok(tools)
    }

    async fn connect_http(&self, server: &MCPServer) -> Result<Vec<MCPTool>> {
        let url = server
            .url
            .as_ref()
            .ok_or_else(|| anyhow!("No url specified for HTTP transport"))?
            .clone();

        let client = reqwest::Client::builder()
            .timeout(CALL_TIMEOUT)
            .user_agent(DEFAULT_USER_AGENT)
            .build()
            .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

        info!(
            "🚀 MCP server '{}' (http) connecting to {}",
            server.name, url
        );

        // initialize
        let init_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "datrina",
                    "version": "0.1.0"
                }
            }
        });

        let (init_result, session_id) = http_rpc(&client, &url, None, init_body).await?;
        if init_result.get("error").is_some() {
            return Err(anyhow!("MCP initialize error: {}", init_result));
        }
        if init_result.get("result").is_none() {
            return Err(anyhow!("MCP initialize response missing result"));
        }

        // initialized notification (no response expected; servers may ignore)
        let init_notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let _ = http_rpc(&client, &url, session_id.as_deref(), init_notif).await;

        // tools/list
        let tools_body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });
        let (tools_envelope, session_id_after) =
            http_rpc(&client, &url, session_id.as_deref(), tools_body).await?;
        let tools = parse_tools_list(&server.id, &tools_envelope)?;
        let session_id = session_id_after.or(session_id);

        info!(
            "📡 MCP '{}' (http) connected with {} tools",
            server.name,
            tools.len()
        );

        let connection = MCPConnection::Http {
            server: server.clone(),
            client,
            url,
            session_id,
            request_id: 3,
            tools: tools.clone(),
        };

        self.connections
            .lock()
            .await
            .insert(server.id.clone(), connection);

        Ok(tools)
    }

    /// Disconnect from an MCP server
    pub async fn disconnect(&self, server_id: &str) -> Result<()> {
        let mut connections = self.connections.lock().await;

        if let Some(conn) = connections.remove(server_id) {
            match conn {
                MCPConnection::Stdio {
                    server,
                    mut process,
                    ..
                } => {
                    let shutdown = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/cancelled",
                        "params": { "reason": "client disconnect" }
                    });

                    if let Some(stdin) = process.stdin.as_mut() {
                        let _ = stdin
                            .write_all(serde_json::to_string(&shutdown)?.as_bytes())
                            .await;
                        let _ = stdin.write_all(b"\n").await;
                    }
                    let _ = process.kill().await;
                    info!("🔌 MCP server '{}' (stdio) disconnected", server.name);
                }
                MCPConnection::Http {
                    server,
                    client,
                    url,
                    session_id,
                    ..
                } => {
                    if let Some(sid) = session_id {
                        let _ = client
                            .delete(&url)
                            .header("Mcp-Session-Id", sid)
                            .send()
                            .await;
                    }
                    info!("🔌 MCP server '{}' (http) disconnected", server.name);
                }
            }
        }

        Ok(())
    }

    /// Call a tool on a connected MCP server
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<Value> {
        let mut connections = self.connections.lock().await;

        let conn = connections
            .get_mut(server_id)
            .ok_or_else(|| anyhow!("MCP server '{}' not connected", server_id))?;

        let request_id = conn.next_request_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments.unwrap_or(json!({}))
            }
        });

        match conn {
            MCPConnection::Stdio { process, .. } => {
                let stdin = process
                    .stdin
                    .as_mut()
                    .ok_or_else(|| anyhow!("Cannot access stdin"))?;
                let request_json = serde_json::to_string(&request)?;
                stdin.write_all(request_json.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;

                let stdout = process
                    .stdout
                    .as_mut()
                    .ok_or_else(|| anyhow!("Cannot access stdout"))?;
                let mut reader = BufReader::new(stdout).lines();

                let response = tokio::time::timeout(CALL_TIMEOUT, reader.next_line()).await;
                match response {
                    Ok(Ok(Some(line))) => {
                        let parsed: Value = serde_json::from_str(&line)?;
                        envelope_result(parsed)
                    }
                    Ok(Ok(None)) => Err(anyhow!("MCP server closed connection")),
                    Ok(Err(e)) => Err(anyhow!("Read error: {}", e)),
                    Err(_) => Err(anyhow!("Tool call timeout")),
                }
            }
            MCPConnection::Http {
                client,
                url,
                session_id,
                ..
            } => {
                let (envelope, new_session) =
                    http_rpc(client, url, session_id.as_deref(), request).await?;
                if let Some(sid) = new_session {
                    *session_id = Some(sid);
                }
                envelope_result(envelope)
            }
        }
    }

    /// Get all tools from all connected servers
    pub async fn list_tools(&self) -> Vec<MCPTool> {
        let connections = self.connections.lock().await;
        connections
            .values()
            .flat_map(|c| c.tools().iter().cloned().collect::<Vec<_>>())
            .collect()
    }

    pub async fn is_connected(&self, server_id: &str) -> bool {
        self.connections.lock().await.contains_key(server_id)
    }

    /// Get status of all servers
    pub async fn get_statuses(&self) -> Vec<MCPServerStatus> {
        let connections = self.connections.lock().await;
        connections
            .values()
            .map(|c| MCPServerStatus {
                id: c.server().id.clone(),
                status: ConnectionStatus::Connected,
                tool_count: Some(c.tools().len()),
                last_error: None,
            })
            .collect()
    }

    /// Disconnect all servers (cleanup)
    pub async fn disconnect_all(&self) {
        let ids: Vec<String> = {
            let connections = self.connections.lock().await;
            connections.keys().cloned().collect()
        };

        for id in ids {
            let _ = self.disconnect(&id).await;
        }
    }
}

impl Drop for MCPManager {
    fn drop(&mut self) {
        // Best-effort cleanup
        let rt = tokio::runtime::Handle::try_current();
        if rt.is_ok() {
            let connections = self.connections.try_lock();
            if let Ok(mut conns) = connections {
                for conn in conns.values_mut() {
                    if let MCPConnection::Stdio { process, .. } = conn {
                        let _ = process.start_kill();
                    }
                }
            }
        }
    }
}

/// Extract the JSON-RPC `result` from an envelope, surfacing `error` as a
/// rust error. Used by both stdio and http call paths.
fn envelope_result(envelope: Value) -> Result<Value> {
    if let Some(error) = envelope.get("error") {
        return Err(anyhow!("Tool error: {}", error));
    }
    Ok(envelope.get("result").cloned().unwrap_or(json!({})))
}

/// Parse a `tools/list` envelope into `MCPTool` records.
fn parse_tools_list(server_id: &str, envelope: &Value) -> Result<Vec<MCPTool>> {
    if let Some(error) = envelope.get("error") {
        return Err(anyhow!("MCP tools/list error: {}", error));
    }
    let result = envelope
        .get("result")
        .ok_or_else(|| anyhow!("MCP tools/list response missing result"))?;
    let tool_list = result
        .get("tools")
        .and_then(|t| t.as_array())
        .ok_or_else(|| anyhow!("MCP tools/list response missing tools array"))?;
    let mut tools = Vec::with_capacity(tool_list.len());
    for tool_val in tool_list {
        if let (Some(name), Some(schema)) = (
            tool_val.get("name").and_then(|n| n.as_str()),
            tool_val.get("inputSchema"),
        ) {
            tools.push(MCPTool {
                server_id: server_id.to_string(),
                name: name.to_string(),
                description: tool_val
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: schema.clone(),
            });
        }
    }
    Ok(tools)
}

/// One MCP-over-HTTP request/response cycle (Streamable HTTP transport).
///
/// Handles both response content-types defined by the spec:
/// - `application/json`: single JSON-RPC envelope in the body.
/// - `text/event-stream`: SSE frames; we take the first `data:` frame that
///   parses as a JSON-RPC envelope.
///
/// Returns the parsed envelope plus any session id that the server assigned
/// (echoed back via the `Mcp-Session-Id` response header on the initialize
/// call; subsequent calls reuse it).
async fn http_rpc(
    client: &reqwest::Client,
    url: &str,
    session_id: Option<&str>,
    body: Value,
) -> Result<(Value, Option<String>)> {
    let mut req = client
        .post(url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&body);
    if let Some(sid) = session_id {
        req = req.header("Mcp-Session-Id", sid);
    }

    let response = req
        .send()
        .await
        .map_err(|e| anyhow!("HTTP MCP request failed: {}", e))?;

    let status = response.status();
    let session_out = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = response.text().await?;

    if !status.is_success() {
        return Err(anyhow!(
            "HTTP MCP request failed with status {}: {}",
            status,
            text
        ));
    }

    // Notification responses can be empty per JSON-RPC spec.
    if text.trim().is_empty() {
        return Ok((json!({}), session_out));
    }

    if content_type.starts_with("text/event-stream") {
        let envelope = parse_sse_envelope(&text)?;
        Ok((envelope, session_out))
    } else {
        let envelope: Value = serde_json::from_str(&text).map_err(|e| {
            anyhow!(
                "Failed to parse MCP response as JSON: {}; body: {}",
                e,
                text
            )
        })?;
        Ok((envelope, session_out))
    }
}

/// Pull the first JSON-RPC envelope out of an SSE stream. Streamable HTTP
/// servers may emit multiple `data:` frames (e.g. progress notifications);
/// the first frame that has `result` or `error` is the actual reply.
fn parse_sse_envelope(text: &str) -> Result<Value> {
    let mut data_buf = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let chunk = rest.trim_start();
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(chunk);
            continue;
        }
        if line.trim().is_empty() && !data_buf.is_empty() {
            if let Ok(value) = serde_json::from_str::<Value>(&data_buf) {
                if value.get("result").is_some() || value.get("error").is_some() {
                    return Ok(value);
                }
            }
            data_buf.clear();
        }
    }
    if !data_buf.is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(&data_buf) {
            return Ok(value);
        }
    }
    Err(anyhow!(
        "MCP SSE response contained no usable JSON-RPC envelope"
    ))
}
