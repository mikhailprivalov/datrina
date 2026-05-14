# Datrina Residual Backlog

Status: active after W9 closeout

This backlog records deferred features from the reconciliation plan. Items here are not MVP-supported unless a later workstream implements and validates them.

## Production Readiness

- Replace local-only plaintext SQLite fallback for provider secrets and MCP environment values with encrypted OS keychain/keyring storage.
- Restore production bundle packaging, production icon sets, and platform distribution checks.
- Replace the deprecated `tauri_plugin_shell::Shell::open` usage with the accepted opener path.

## Workflow And Scheduler

- Wire scheduler cron matches to persisted workflow execution through the same workflow engine used by manual runs.
- Add explicit workflow cancellation command/model behavior if needed by product flows.
- Add advanced retry, queue, priority, and dead-letter behavior only after the MVP runner contract is stable.
- Add a visual workflow editor without changing the Rust-owned execution boundary.
- Keep unsupported node kinds honest: MCP and LLM workflow nodes must stay explicit errors until they are policy-gated and validated.

## Tools And MCP

- Expose built-in tool execution only through the existing Rust `ToolEngine` policy gateway.
- Route future workflow tool nodes through `ToolEngine` before invoking MCP or built-in tools.
- Add remote MCP transports only with a new hardening decision and validation gate.
- Keep plugin SDK and marketplace behavior deferred until a dedicated product/runtime workstream exists.

## AI And Chat

- Add typed Tauri streaming events if streaming chat becomes part of the product scope.
- Add dashboard generation and tool-calling chat flows only after W5 tool policy and workflow boundaries are integrated.
- Preserve the current no-key behavior: use `local_mock` for deterministic success, otherwise return unavailable/error state.

## Dashboard And Widgets

- Add a UI for creating and editing widgets beyond the built-in `local_mvp` template.
- Implement widget post-process steps after the runtime contract and failure semantics are explicit.
- Add scheduled widget auto-refresh after scheduler-triggered execution is implemented.
- Add generated dashboard templates only after chat generation is validated as real behavior.

## Deferred Research Promises

- Public HTTP/REST API.
- Node/Hono/Turborepo runtime inside `datrina`.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics.
- OAuth, teams, cloud sync, and multi-user auth.
- Mobile companion app.
