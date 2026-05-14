# W5 Tool Security Baseline

Date: 2026-05-14

## Accepted Baseline

MVP tool execution is Rust-owned and goes through `ToolEngine`.

- Built-in tools: `curl` and `http_request`.
- MCP transport: stdio only.
- MCP process command allowlist: `node`, `npx`, `bun`, `bunx`, `uvx`.
- Network policy: only `http` and `https` URLs are accepted; localhost,
  loopback, private IPv4, link-local, unspecified, and unique-local IPv6
  targets are rejected.
- Audit events are logged through tracing target `datrina::tool_audit`.

Audit event shape:

```json
{
  "timestamp": 0,
  "target_kind": "builtin_tool | mcp_server | mcp_tool",
  "target": "curl | server_id | server_id.tool_name",
  "action": "execute | connect | call",
  "decision": "accepted | rejected",
  "reason": "optional rejection reason"
}
```

## Static Security Review

- `src-tauri/tauri.conf.json`: shell plugin is limited to opening HTTP(S)
  links. CSP still allows localhost and HTTPS connection targets for app
  runtime, but tool/network execution is Rust-gated by `ToolEngine`.
- `src-tauri/src/commands/system.rs`: `open_url` is still shell-plugin scoped
  by Tauri config. It does not execute local commands.
- `src-tauri/src/modules/mcp_manager.rs`: process spawning remains isolated in
  the MCP manager, and command entrypoints validate saved MCP configs through
  `ToolEngine` before connecting.
- `src-tauri/src/modules/tool_engine.rs`: built-in command execution, MCP
  command allowlist checks, URL policy, and accepted/rejected audit events are
  centralized here.
- `src-tauri/src/commands/provider.rs`: provider tests remain explicit
  unsupported behavior until W6 implements Rust-mediated provider calls.
- `src/components/widgets/TextWidget.tsx`: markdown and HTML content no longer
  use `dangerouslySetInnerHTML`; HTML-format content is rendered as text until a
  sanitizer-backed renderer is accepted.

## Residuals

- `ToolEngine::http_request` is implemented as the Rust gateway, but no frontend
  command exposes it yet.
- Future workflow tool nodes must call the same `ToolEngine` policy surface
  before invoking MCP or built-in tools.
