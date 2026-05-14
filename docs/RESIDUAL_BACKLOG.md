# Datrina Residual Backlog

Status: active after W9 closeout

This backlog records deferred features from the reconciliation plan. Items here are not MVP-supported unless a later workstream implements and validates them.

## Production Readiness

- Replace local-only plaintext SQLite fallback for provider secrets and MCP environment values with encrypted OS keychain/keyring storage.
- Restore production bundle packaging, production icon sets, and platform distribution checks.

## Workflow And Scheduler

- Add explicit workflow cancellation command/model behavior if needed by product flows.
- Add advanced retry, queue, priority, and dead-letter behavior only after the MVP runner contract is stable.
- Add a visual workflow editor without changing the Rust-owned execution boundary.

## Tools And MCP

- Expose built-in tool execution only through the existing Rust `ToolEngine` policy gateway.
- Add provider-driven chat tool schema emission, parsing, tool-result messages, and bounded resume loops.
- Add remote MCP transports only with a new hardening decision and validation gate.
- Keep plugin SDK and marketplace behavior deferred until a dedicated product/runtime workstream exists.

## AI And Chat

- Add typed Tauri streaming events if streaming chat becomes part of the product scope.
- Expand build chat from deterministic apply controls to provider-generated structured change proposals.
- Preserve the current no-key behavior: use `local_mock` for deterministic success, otherwise return unavailable/error state.

## Dashboard And Widgets

- Add full widget editing forms beyond the W10 local text/gauge add controls.
- Implement widget post-process steps after the runtime contract and failure semantics are explicit.
- Add scheduled widget auto-refresh wiring in React after scheduler-triggered workflow execution is surfaced in UI state.
- Add provider-generated dashboard templates only after chat generation is validated as real behavior.

## Deferred Research Promises

- Public HTTP/REST API.
- Node/Hono/Turborepo runtime inside `datrina`.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics.
- OAuth, teams, cloud sync, and multi-user auth.
- Mobile companion app.
