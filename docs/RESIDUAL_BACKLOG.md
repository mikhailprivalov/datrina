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
- Add remote MCP transports only with a new hardening decision and validation gate.
- Keep plugin SDK and marketplace behavior deferred until a dedicated product/runtime workstream exists.

## AI And Chat

- Extend chat beyond the current bounded one-resume tool loop only after explicit UX and policy limits are defined.
- Preserve the current no-key behavior: use clearly labeled `local_mock` dev/test mode for deterministic success, otherwise return unavailable/error state.
- Rerun W13-W15 real-provider acceptance with user-provided credentials/service availability before treating external provider behavior as live-validated in this checkout.
- Add live token streaming support for Ollama only if product scope needs it; current Ollama chat events are honest synthetic single-step events.

## Dashboard And Widgets

- Add full manual widget editing forms beyond W12 generated proposal apply.
- Implement widget post-process steps after the runtime contract and failure semantics are explicit.
- Add per-widget cron editing controls in React if scheduled refresh should be configured outside generated datasource plans or raw workflow commands.

## Deferred Research Promises

- Public HTTP/REST API.
- Node/Hono/Turborepo runtime inside `datrina`.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics.
- OAuth, teams, cloud sync, and multi-user auth.
- Mobile companion app.
