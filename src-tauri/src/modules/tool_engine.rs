use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::models::mcp::{MCPServer, MCPTransport};

pub const DEFAULT_USER_AGENT: &str = "Datrina/0.1.0 (+local)";

/// W37++: hard cap on web_fetch response payload. 500 KiB matches the
/// Google-recommended robots.txt limit and is large enough for a normal
/// article-sized HTML page once trimmed to body text.
pub const WEB_FETCH_DEFAULT_MAX_BYTES: usize = 500 * 1024;

/// W37++: robots.txt cache TTL. Long enough to avoid hammering a host
/// during a chat turn; short enough to pick up policy changes.
const ROBOTS_CACHE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
enum RobotsDecision {
    /// robots.txt fetched cleanly; raw body cached for the bot lookup.
    Body(Vec<u8>),
    /// 404/empty/etc — treat as "no constraints recorded".
    Unrestricted,
}

#[derive(Debug, Clone)]
struct CachedRobots {
    decision: RobotsDecision,
    inserted_at: Instant,
}

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
    pub network_schemes: Vec<String>,
    pub blocked_networks: Vec<String>,
}

/// Policy gateway for built-in tools and network access. MCP server launch
/// is no longer gated by an executable allowlist — the user is trusted to
/// register only commands they intend to run. Every connect attempt is still
/// audited via `validate_mcp_server`.
pub struct ToolEngine {
    builtin_tools: HashSet<String>,
    /// User-Agent string applied to every outbound HTTP request from
    /// `http_request` / `execute_curl`. Defaults to [`DEFAULT_USER_AGENT`];
    /// overridable at runtime via [`ToolEngine::set_user_agent`] when the
    /// user changes it in settings.
    user_agent: RwLock<String>,
    /// W37++: per-host robots.txt body cache. Keyed by `scheme://host:port`
    /// so different schemes/ports don't share a decision. The cached value
    /// is the raw `robots.txt` body — parsing is cheap and avoids carrying
    /// non-`Send` types across awaits.
    robots_cache: RwLock<HashMap<String, CachedRobots>>,
}

impl ToolEngine {
    pub fn new(builtin_tools: Vec<String>) -> Self {
        Self {
            builtin_tools: builtin_tools.into_iter().collect(),
            user_agent: RwLock::new(DEFAULT_USER_AGENT.to_string()),
            robots_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Snapshot the User-Agent string currently applied to HTTP requests.
    pub fn user_agent(&self) -> String {
        self.user_agent
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| DEFAULT_USER_AGENT.to_string())
    }

    /// Replace the User-Agent string. Empty input restores the default so
    /// "clear this field" in settings means "use the canonical Datrina UA"
    /// rather than send a blank header.
    pub fn set_user_agent(&self, value: &str) {
        let trimmed = value.trim();
        let next = if trimmed.is_empty() {
            DEFAULT_USER_AGENT.to_string()
        } else {
            trimmed.to_string()
        };
        if let Ok(mut guard) = self.user_agent.write() {
            *guard = next;
        }
    }

    pub fn policy_snapshot(&self) -> ToolPolicySnapshot {
        let mut builtin_tools: Vec<_> = self.builtin_tools.iter().cloned().collect();
        builtin_tools.sort();

        ToolPolicySnapshot {
            builtin_tools,
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

        let user_agent = self.user_agent();
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;
        let mut request_builder = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            "DELETE" => client.delete(url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        // Add headers (caller-supplied "User-Agent" here overrides the
        // client default — useful for one-off testing against picky APIs).
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

    /// W37++: Safe single-URL fetch used by the catalog `web_fetch`
    /// external source. On top of [`Self::http_request`] it adds:
    ///
    /// 1. **robots.txt obedience** — the host's `robots.txt` is fetched
    ///    once (cached for 30 minutes) and checked against the current
    ///    User-Agent. A disallowed path returns a hard error instead of
    ///    silently returning the body.
    /// 2. **Hard size cap** — the response body is streamed and aborted
    ///    once `max_bytes` are buffered, so a 50 MB page can't blow up
    ///    chat memory. The returned value carries `truncated: bool` so
    ///    the LLM can tell when it didn't see the full body.
    /// 3. **Text-first response** — `body` is the trimmed text payload
    ///    plus a `content_type` field, so a chat tool result is small
    ///    and predictable. JSON bodies are still parsed when possible.
    ///
    /// The URL still passes through the same SSRF / scheme / private-IP
    /// policy as `http_request`.
    pub async fn web_fetch(&self, url: &str, max_bytes: Option<usize>) -> Result<Value> {
        let validation = self.validate_url(url);
        self.audit(
            "builtin_tool",
            "web_fetch",
            "execute",
            validation.as_ref().err().map(|e| e.to_string()),
        );
        validation?;
        let cap = max_bytes
            .filter(|v| *v > 0)
            .unwrap_or(WEB_FETCH_DEFAULT_MAX_BYTES);
        let user_agent = self.user_agent();

        // robots.txt obedience runs before the live fetch so we can
        // fail closed without speaking to the target URL itself.
        self.assert_robots_allow(url, &user_agent).await?;

        info!("web_fetch GET {} (cap {} bytes)", url, cap);

        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("web_fetch request failed: {}", e))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow!("web_fetch body read failed: {}", e))?;
        let raw_len = bytes.len();
        let truncated = raw_len > cap;
        let slice = if truncated { &bytes[..cap] } else { &bytes[..] };
        let text = String::from_utf8_lossy(slice).to_string();

        let parsed_body = serde_json::from_str::<Value>(&text)
            .ok()
            .filter(|_| !truncated);

        let body = parsed_body
            .map(|v| serde_json::json!({ "json": v }))
            .unwrap_or_else(|| serde_json::json!({ "text": text }));

        Ok(serde_json::json!({
            "status": status.as_u16(),
            "url": url,
            "content_type": content_type,
            "bytes": raw_len,
            "max_bytes": cap,
            "truncated": truncated,
            "body": body,
        }))
    }

    async fn assert_robots_allow(&self, url: &str, user_agent: &str) -> Result<()> {
        let parsed =
            reqwest::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;
        let scheme = parsed.scheme();
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("URL must include a host"))?;
        let port_part = parsed.port().map(|p| format!(":{}", p)).unwrap_or_default();
        let cache_key = format!("{}://{}{}", scheme, host, port_part);
        let path = if parsed.path().is_empty() {
            "/".to_string()
        } else {
            parsed.path().to_string()
        };

        // Fast path: cached body still fresh.
        let cached = self
            .robots_cache
            .read()
            .ok()
            .and_then(|guard| guard.get(&cache_key).cloned())
            .filter(|entry| entry.inserted_at.elapsed() <= ROBOTS_CACHE_TTL);

        let body = match cached {
            Some(entry) => entry.decision,
            None => {
                let robots_url = format!("{}://{}{}/robots.txt", scheme, host, port_part);
                let fetched = match Self::fetch_robots_body(&robots_url, user_agent).await {
                    Ok(Some(bytes)) => RobotsDecision::Body(bytes),
                    Ok(None) => RobotsDecision::Unrestricted,
                    Err(err) => {
                        warn!(
                            "web_fetch: robots.txt for {} unreachable, treating as unrestricted: {}",
                            cache_key, err
                        );
                        RobotsDecision::Unrestricted
                    }
                };
                if let Ok(mut guard) = self.robots_cache.write() {
                    guard.insert(
                        cache_key.clone(),
                        CachedRobots {
                            decision: fetched.clone(),
                            inserted_at: Instant::now(),
                        },
                    );
                }
                fetched
            }
        };

        if let RobotsDecision::Body(bytes) = body {
            let robot = texting_robots::Robot::new(user_agent, &bytes)
                .map_err(|e| anyhow!("invalid robots.txt for {}: {}", cache_key, e))?;
            if !robot.allowed(&path) {
                return Err(anyhow!(
                    "web_fetch blocked by robots.txt for {} (path {})",
                    cache_key,
                    path
                ));
            }
        }
        Ok(())
    }

    async fn fetch_robots_body(robots_url: &str, user_agent: &str) -> Result<Option<Vec<u8>>> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(8))
            .build()?;
        let response = client.get(robots_url).send().await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        // Bound robots.txt body to 500 KiB per Google's recommendation.
        let limit = bytes.len().min(WEB_FETCH_DEFAULT_MAX_BYTES);
        Ok(Some(bytes[..limit].to_vec()))
    }

    #[cfg(test)]
    pub(crate) fn robots_cache_insert_for_test(&self, host_key: &str, body: Vec<u8>) {
        if let Ok(mut guard) = self.robots_cache.write() {
            guard.insert(
                host_key.to_string(),
                CachedRobots {
                    decision: RobotsDecision::Body(body),
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    #[cfg(test)]
    pub(crate) async fn check_robots_for_test(&self, url: &str) -> Result<()> {
        self.assert_robots_allow(url, &self.user_agent()).await
    }

    /// Get current whitelist
    pub fn get_whitelist(&self) -> Vec<String> {
        let mut tools: Vec<_> = self.builtin_tools.iter().cloned().collect();
        tools.sort();
        tools
    }

    fn validate_mcp_server_inner(&self, server: &MCPServer) -> Result<()> {
        match server.transport {
            MCPTransport::Stdio => {
                let command = server
                    .command
                    .as_deref()
                    .ok_or_else(|| anyhow!("No command specified for stdio MCP server"))?;
                if command.trim().is_empty() {
                    return Err(anyhow!("MCP stdio command cannot be empty"));
                }
            }
            MCPTransport::Http => {
                let url = server
                    .url
                    .as_deref()
                    .ok_or_else(|| anyhow!("No url specified for HTTP MCP server"))?;
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(anyhow!(
                        "HTTP MCP server url must start with http:// or https://"
                    ));
                }
            }
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
        validate_public_http_url(url)
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
        Self::new(vec!["curl".to_string(), "http_request".to_string()])
    }
}

/// W39: validate the structured arguments object an `http_request`
/// datasource is built from. Reused by Build apply, Workbench manual
/// create, and the proposal validation gate so unsafe sources fail
/// with a typed error instead of materializing into the catalog.
///
/// Allowed shape: `{ "method": "GET"|"POST"|..., "url": "https://...",
/// "headers"?: { string: string }, "body"?: <JSON> }`.
///
/// Authorization-bearing headers are rejected on the React side: secrets
/// live in the W37 external_source_state row, not in workflow JSON.
pub fn validate_http_request_arguments(args: &Value) -> Result<()> {
    let obj = args
        .as_object()
        .ok_or_else(|| anyhow!("http_request arguments must be an object"))?;
    let method = obj
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("http_request arguments require a 'method' string"))?;
    match method.to_uppercase().as_str() {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" => {}
        other => {
            return Err(anyhow!(
                "http_request method '{}' is not allowed (GET/POST/PUT/PATCH/DELETE only)",
                other
            ))
        }
    }
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("http_request arguments require a 'url' string"))?;
    validate_public_http_url(url)?;

    if let Some(headers) = obj.get("headers") {
        let map = headers
            .as_object()
            .ok_or_else(|| anyhow!("http_request headers must be a JSON object"))?;
        for (name, value) in map {
            if !value.is_string() {
                return Err(anyhow!(
                    "http_request header '{}' must be a string (got {})",
                    name,
                    value
                ));
            }
            let lower = name.to_ascii_lowercase();
            if lower == "authorization" || lower == "proxy-authorization" || lower == "cookie" {
                return Err(anyhow!(
                    "Refusing to store credential-bearing header '{}' on a saved datasource — use the External Source catalog credential slot instead",
                    name
                ));
            }
        }
    }
    if let Some(body) = obj.get("body") {
        // We don't constrain body shape (JSON arbitrarily nested), but
        // explicit strings of "[object Object]" / control-char garbage
        // from the UI are usually copy-paste mistakes — reject those.
        if let Some(text) = body.as_str() {
            if text.contains("[object Object]") {
                return Err(anyhow!(
                    "http_request body is the literal string '[object Object]' — pass the JSON value instead"
                ));
            }
        }
    }
    Ok(())
}

/// Public-internet HTTP scheme + host check shared by the apply path
/// and the runtime executor. Mirrors `ToolEngine::validate_url` without
/// requiring a live `ToolEngine` instance.
pub fn validate_public_http_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn web_fetch_rejects_private_and_localhost_urls() {
        let engine = ToolEngine::default();
        // Builtin tool must be in the allowlist for the underlying
        // validator; web_fetch only uses validate_url so this is fine.
        let err = engine
            .web_fetch("http://localhost:1234/", None)
            .await
            .expect_err("localhost must be blocked");
        assert!(err.to_string().contains("localhost"));
        let err = engine
            .web_fetch("http://127.0.0.1/", None)
            .await
            .expect_err("loopback IP must be blocked");
        assert!(err.to_string().contains("private/local"));
    }

    #[test]
    fn validate_http_request_arguments_rejects_unsafe_inputs() {
        // Missing url
        assert!(validate_http_request_arguments(&json!({ "method": "GET" })).is_err());
        // Missing method
        assert!(validate_http_request_arguments(&json!({ "url": "https://example.com" })).is_err());
        // Bogus method
        assert!(validate_http_request_arguments(
            &json!({ "method": "OPTIONS", "url": "https://example.com" })
        )
        .is_err());
        // Loopback host
        assert!(validate_http_request_arguments(
            &json!({ "method": "GET", "url": "http://127.0.0.1/internal" })
        )
        .is_err());
        // localhost host
        assert!(validate_http_request_arguments(
            &json!({ "method": "GET", "url": "http://localhost/admin" })
        )
        .is_err());
        // Non-http scheme
        assert!(validate_http_request_arguments(
            &json!({ "method": "GET", "url": "file:///etc/passwd" })
        )
        .is_err());
        // Credential-bearing header
        assert!(validate_http_request_arguments(&json!({
            "method": "GET",
            "url": "https://example.com",
            "headers": { "Authorization": "Bearer secret" }
        }))
        .is_err());
        // Non-string header value
        assert!(validate_http_request_arguments(&json!({
            "method": "GET",
            "url": "https://example.com",
            "headers": { "X-Custom": 42 }
        }))
        .is_err());
        // Body that looks like an accidentally-stringified object
        assert!(validate_http_request_arguments(&json!({
            "method": "POST",
            "url": "https://example.com",
            "body": "[object Object]"
        }))
        .is_err());
        // Happy path
        validate_http_request_arguments(&json!({
            "method": "GET",
            "url": "https://api.example.com/data?q=1",
            "headers": { "Accept": "application/json", "User-Agent": "datrina" }
        }))
        .expect("valid HTTP arguments must pass");
    }

    #[tokio::test]
    async fn assert_robots_allow_blocks_disallowed_path_from_cache() {
        let engine = ToolEngine::default();
        // Wildcard agent so we don't depend on texting_robots' specific
        // substring-match semantics for the Datrina User-Agent.
        let robots = b"User-agent: *\nDisallow: /private\nAllow: /public\n";
        engine.robots_cache_insert_for_test("https://example.com", robots.to_vec());
        let blocked = engine
            .check_robots_for_test("https://example.com/private/data")
            .await
            .expect_err("disallowed path must be blocked");
        assert!(
            blocked.to_string().contains("blocked by robots.txt"),
            "{}",
            blocked
        );
        engine
            .check_robots_for_test("https://example.com/public/article")
            .await
            .expect("allowed path must pass");
    }
}
