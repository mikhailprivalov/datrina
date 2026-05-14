# W3 Storage, Config, And Secrets Baseline

Status: accepted

## Storage Boundary

SQLite remains the MVP persistence baseline through `sqlx`.

- Runtime database path: Tauri app data directory plus `app.db`.
- Migration mechanism: embedded Rust table creation in `Storage::migrate()`.
- Runtime JSON export/import: not implemented in W3. JSON values are used only
  inside SQLite columns for structured fields such as dashboard layout,
  workflow nodes/edges, MCP args/env, provider models, and workflow results.

Persisted tables now cover the W3 baseline:

- `dashboards`
- `chat_sessions`
- `workflows`
- `workflow_runs`
- `providers`
- `mcp_servers`
- `app_config`

## Command Behavior

Implemented storage-backed behavior:

- dashboard list/get/create/update/delete,
- chat session list/get/create/update/delete,
- config get/set,
- workflow list/get/create/delete,
- workflow execution run persistence through `workflow_runs`,
- workflow `last_run` persistence after execution,
- provider list/add/remove,
- MCP server list/add/remove.

Explicitly unavailable in W3:

- `test_provider` returns an error explaining that W6 owns Rust-mediated
  provider calls. It no longer returns fake success.

Still owned by later workstreams:

- workflow node runtime honesty for MCP/LLM nodes belongs to W7/W6,
- widget refresh runtime data belongs to W4/W7,
- MCP/tool execution security gateway belongs to W5.

## Secrets Policy

Secrets and MCP environment values are Rust-owned.

W3 does not add OS keychain/keyring encryption. The accepted MVP fallback is
local-only plaintext storage in SQLite under the Tauri app data directory.
This keeps baseline validation local and credential-free, but it is not a
production-grade secret store.

Mitigation applied in W3:

- provider API keys are persisted in SQLite but stripped from provider command
  responses before data is returned to React,
- MCP environment values are persisted in SQLite but masked as `********` in
  `list_servers` responses,
- commands that need secrets internally still read full values from Rust
  storage.

Residual mitigation task for a later workstream:

- replace the plaintext fallback with encrypted OS keychain/keyring storage
  before production packaging or real credential usage is documented as
  supported.

## Validation Run

Executed on 2026-05-13:

- `cargo test --workspace storage::tests::dashboard_config_provider_mcp_and_workflow_persistence_smoke`
  passed: dashboard/config/provider/MCP/workflow persistence smoke succeeded.
- `bun run check:contract` passed: 33 frontend commands match Rust
  registrations.
- `bun run typecheck` passed.
- `cargo fmt --all --check` passed.
- `cargo check --workspace --all-targets` passed. Rust reported the existing
  deprecated `tauri_plugin_shell::Shell::open` warning in `system.rs`.
