use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::models::mcp::{ConnectionStatus, MCPServer, MCPServerStatus, MCPTool, MCPTransport};
use crate::models::Id;

/// Active MCP connection with a server process
struct MCPConnection {
    server: MCPServer,
    process: Child,
    request_id: u64,
    #[allow(dead_code)]
    tools: Vec<MCPTool>,
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

    /// Connect to an MCP server via stdio transport
    pub async fn connect(&self, server: MCPServer) -> Result<Vec<MCPTool>> {
        // Disconnect existing if any
        self.disconnect(&server.id).await.ok();

        match server.transport {
            MCPTransport::Stdio => self.connect_stdio(&server).await,
            MCPTransport::Http => {
                // HTTP transport - tools discovered via endpoint
                info!("HTTP MCP transport for {} not yet implemented", server.id);
                Ok(vec![])
            }
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
                "protocolVersion": "2024-11-05",
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
        let response =
            tokio::time::timeout(tokio::time::Duration::from_secs(10), reader.next_line()).await;

        match response {
            Ok(Ok(Some(line))) => {
                let parsed: Value = serde_json::from_str(&line)?;
                if parsed.get("error").is_some() {
                    error!("MCP initialize error: {}", line);
                }
            }
            Ok(Ok(None)) => warn!("MCP server closed connection during init"),
            Ok(Err(e)) => error!("MCP read error: {}", e),
            Err(_) => warn!("MCP initialize timeout"),
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

        let mut tools = Vec::new();

        let tools_response =
            tokio::time::timeout(tokio::time::Duration::from_secs(10), reader.next_line()).await;

        match tools_response {
            Ok(Ok(Some(line))) => {
                let parsed: Value = serde_json::from_str(&line)?;
                if let Some(result) = parsed.get("result") {
                    if let Some(tool_list) = result.get("tools").and_then(|t| t.as_array()) {
                        for tool_val in tool_list {
                            if let (Some(name), Some(schema)) = (
                                tool_val.get("name").and_then(|n| n.as_str()),
                                tool_val.get("inputSchema"),
                            ) {
                                tools.push(MCPTool {
                                    server_id: server.id.clone(),
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
                    }
                }
            }
            _ => warn!("Could not discover tools for {}", server.name),
        }

        info!(
            "📡 MCP '{}' connected with {} tools",
            server.name,
            tools.len()
        );

        // Store connection
        let connection = MCPConnection {
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

    /// Disconnect from an MCP server
    pub async fn disconnect(&self, server_id: &str) -> Result<()> {
        let mut connections = self.connections.lock().await;

        if let Some(mut conn) = connections.remove(server_id) {
            // Try graceful shutdown
            let shutdown = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "reason": "client disconnect" }
            });

            if let Some(stdin) = conn.process.stdin.as_mut() {
                let _ = stdin
                    .write_all(serde_json::to_string(&shutdown)?.as_bytes())
                    .await;
                let _ = stdin.write_all(b"\n").await;
            }

            // Kill process
            let _ = conn.process.kill().await;
            info!("🔌 MCP server '{}' disconnected", conn.server.name);
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

        let request_id = conn.request_id;
        conn.request_id += 1;

        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments.unwrap_or(json!({}))
            }
        });

        let stdin = conn
            .process
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("Cannot access stdin"))?;

        let request_json = serde_json::to_string(&request)?;
        stdin.write_all(request_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        // Read response
        let stdout = conn
            .process
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("Cannot access stdout"))?;
        let mut reader = BufReader::new(stdout).lines();

        let response =
            tokio::time::timeout(tokio::time::Duration::from_secs(60), reader.next_line()).await;

        match response {
            Ok(Ok(Some(line))) => {
                let parsed: Value = serde_json::from_str(&line)?;
                if let Some(error) = parsed.get("error") {
                    Err(anyhow!("Tool error: {}", error))
                } else {
                    Ok(parsed.get("result").cloned().unwrap_or(json!({})))
                }
            }
            Ok(Ok(None)) => Err(anyhow!("MCP server closed connection")),
            Ok(Err(e)) => Err(anyhow!("Read error: {}", e)),
            Err(_) => Err(anyhow!("Tool call timeout")),
        }
    }

    /// Get all tools from all connected servers
    pub async fn list_tools(&self) -> Vec<MCPTool> {
        let connections = self.connections.lock().await;
        connections.values().flat_map(|c| c.tools.clone()).collect()
    }

    /// Get status of all servers
    pub async fn get_statuses(&self) -> Vec<MCPServerStatus> {
        let connections = self.connections.lock().await;
        connections
            .values()
            .map(|c| MCPServerStatus {
                id: c.server.id.clone(),
                status: ConnectionStatus::Connected,
                tool_count: Some(c.tools.len()),
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
                    let _ = conn.process.start_kill();
                }
            }
        }
    }
}
