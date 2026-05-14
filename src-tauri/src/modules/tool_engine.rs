use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tracing::{info, warn};

use crate::models::mcp::{MCPServer, MCPTransport};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAuditDecision {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAuditEvent {
    pub timestamp: i64,
    pub target_kind: String,
    pub target: String,
    pub action: String,
    pub decision: ToolAuditDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicySnapshot {
    pub builtin_tools: Vec<String>,
    pub mcp_commands: Vec<String>,
    pub network_schemes: Vec<String>,
    pub blocked_networks: Vec<String>,
}

/// Policy gateway for built-in tools, MCP process launch, and network access.
pub struct ToolEngine {
    builtin_tools: HashSet<String>,
    mcp_commands: HashSet<String>,
}

impl ToolEngine {
    pub fn new(builtin_tools: Vec<String>, mcp_commands: Vec<String>) -> Self {
        Self {
            builtin_tools: builtin_tools.into_iter().collect(),
            mcp_commands: mcp_commands.into_iter().collect(),
        }
    }

    pub fn policy_snapshot(&self) -> ToolPolicySnapshot {
        let mut builtin_tools: Vec<_> = self.builtin_tools.iter().cloned().collect();
        let mut mcp_commands: Vec<_> = self.mcp_commands.iter().cloned().collect();
        builtin_tools.sort();
        mcp_commands.sort();

        ToolPolicySnapshot {
            builtin_tools,
            mcp_commands,
            network_schemes: vec!["https".to_string(), "http".to_string()],
            blocked_networks: vec![
                "localhost".to_string(),
                "loopback".to_string(),
                "private_ipv4".to_string(),
                "link_local".to_string(),
                "unique_local_ipv6".to_string(),
            ],
        }
    }

    /// Check if a built-in tool is allowed.
    pub fn is_builtin_allowed(&self, tool: &str) -> bool {
        self.builtin_tools.contains(tool)
    }

    pub fn validate_mcp_server(&self, server: &MCPServer) -> Result<()> {
        let result = self.validate_mcp_server_inner(server);
        self.audit(
            "mcp_server",
            &server.id,
            "connect",
            result.as_ref().err().map(|e| e.to_string()),
        );
        result
    }

    pub fn validate_mcp_tool_call(&self, server_id: &str, tool_name: &str) -> Result<()> {
        let result = if tool_name.trim().is_empty() {
            Err(anyhow!("MCP tool name cannot be empty"))
        } else {
            Ok(())
        };
        self.audit(
            "mcp_tool",
            &format!("{server_id}.{tool_name}"),
            "call",
            result.as_ref().err().map(|e| e.to_string()),
        );
        result
    }

    /// Execute curl with sandboxed arguments
    pub async fn execute_curl(&self, args: Vec<String>) -> Result<Value> {
        let validation = self.validate_curl_args(&args);
        self.audit(
            "builtin_tool",
            "curl",
            "execute",
            validation.as_ref().err().map(|e| e.to_string()),
        );
        validation?;

        info!("curl {}", args.join(" "));

        let output = tokio::process::Command::new("curl")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| anyhow!("Failed to execute curl: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("curl failed: {}", stderr));
        }

        // Try parse as JSON
        match serde_json::from_str::<Value>(&stdout) {
            Ok(json) => Ok(json),
            Err(_) => Ok(json!({ "raw": stdout.to_string() })),
        }
    }

    /// Execute a generic HTTP request using reqwest
    pub async fn http_request(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
        headers: Option<Value>,
    ) -> Result<Value> {
        let validation = self.validate_http_request(method, url);
        self.audit(
            "builtin_tool",
            "http_request",
            "execute",
            validation.as_ref().err().map(|e| e.to_string()),
        );
        validation?;

        info!("{} {}", method, url);

        let client = reqwest::Client::new();
        let mut request_builder = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            "DELETE" => client.delete(url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        // Add headers
        if let Some(hdrs) = headers {
            if let Some(obj) = hdrs.as_object() {
                for (key, value) in obj {
                    request_builder = request_builder.header(key, value.as_str().unwrap_or(""));
                }
            }
        }

        // Add body
        if let Some(b) = body {
            request_builder = request_builder.json(&b);
        }

        let response = request_builder
            .send()
            .await
            .map_err(|e| anyhow!("HTTP request failed: {}", e))?;

        let status = response.status();
        let text = response.text().await?;

        // Try parse as JSON
        let body = match serde_json::from_str::<Value>(&text) {
            Ok(json) => json,
            Err(_) => json!({ "raw": text }),
        };

        Ok(json!({
            "status": status.as_u16(),
            "body": body
        }))
    }

    /// Get current whitelist
    pub fn get_whitelist(&self) -> Vec<String> {
        let mut tools: Vec<_> = self.builtin_tools.iter().cloned().collect();
        tools.sort();
        tools
    }

    fn validate_mcp_server_inner(&self, server: &MCPServer) -> Result<()> {
        match server.transport {
            MCPTransport::Stdio => {}
            MCPTransport::Http => {
                return Err(anyhow!(
                    "HTTP MCP transport is unsupported in MVP; configure a stdio MCP server"
                ));
            }
        }

        let command = server
            .command
            .as_deref()
            .ok_or_else(|| anyhow!("No command specified for stdio MCP server"))?;
        let command_name = command.rsplit('/').next().unwrap_or(command);
        if !self.mcp_commands.contains(command_name) {
            return Err(anyhow!(
                "MCP command '{}' is not in the allowlist",
                command_name
            ));
        }

        Ok(())
    }

    fn validate_curl_args(&self, args: &[String]) -> Result<()> {
        if !self.is_builtin_allowed("curl") {
            return Err(anyhow!("curl is not in the built-in tool allowlist"));
        }

        for (i, arg) in args.iter().enumerate() {
            if arg.starts_with("http://") || arg.starts_with("https://") {
                self.validate_url(arg)?;
            }

            if matches!(
                arg.as_str(),
                "-o" | "--output" | "-O" | "--remote-name" | "--config" | "-K"
            ) {
                return Err(anyhow!("curl flag '{}' is blocked by tool policy", arg));
            }

            if arg == "--next" {
                return Err(anyhow!("curl --next is blocked by tool policy"));
            }

            if arg == "--url" {
                let url = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("curl --url requires a URL argument"))?;
                self.validate_url(url)?;
            }
        }

        Ok(())
    }

    fn validate_http_request(&self, method: &str, url: &str) -> Result<()> {
        if !self.is_builtin_allowed("http_request") {
            return Err(anyhow!(
                "http_request is not in the built-in tool allowlist"
            ));
        }
        match method.to_uppercase().as_str() {
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" => {}
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        }
        self.validate_url(url)
    }

    fn validate_url(&self, url: &str) -> Result<()> {
        let parsed =
            reqwest::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(anyhow!("URL scheme '{}' is not allowed", parsed.scheme()));
        }

        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("URL must include a host"))?;
        if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
            return Err(anyhow!("Blocked local URL host: {}", host));
        }

        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_blocked_ip(ip) {
                return Err(anyhow!("Blocked private/local URL host: {}", host));
            }
        }

        Ok(())
    }

    fn audit(&self, target_kind: &str, target: &str, action: &str, reason: Option<String>) {
        let event = ToolAuditEvent {
            timestamp: Utc::now().timestamp_millis(),
            target_kind: target_kind.to_string(),
            target: target.to_string(),
            action: action.to_string(),
            decision: if reason.is_some() {
                ToolAuditDecision::Rejected
            } else {
                ToolAuditDecision::Accepted
            },
            reason,
        };

        match serde_json::to_string(&event) {
            Ok(serialized) => info!(target: "datrina::tool_audit", "{}", serialized),
            Err(e) => warn!("failed to serialize tool audit event: {}", e),
        }
    }
}

impl Default for ToolEngine {
    fn default() -> Self {
        Self::new(
            vec!["curl".to_string(), "http_request".to_string()],
            vec![
                "node".to_string(),
                "npx".to_string(),
                "bun".to_string(),
                "bunx".to_string(),
                "uvx".to_string(),
            ],
        )
    }
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00
}
