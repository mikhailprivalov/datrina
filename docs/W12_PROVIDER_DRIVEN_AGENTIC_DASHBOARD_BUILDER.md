# W12 Provider-Driven Agentic Dashboard Builder

Status: implemented with external-provider validation residual on 2026-05-15.

## Scope Handled

W12 replaces fixed Build Chat apply buttons with provider-generated structured
proposals and closes the no-mock product path as far as local validation can
prove without user credentials:

- Build Chat system prompting now asks the active provider for strict JSON
  dashboard/widget proposals.
- Assistant messages persist parsed proposal metadata for preview and apply.
- React previews generated dashboard/widget changes and requires explicit user
  confirmation before apply.
- Proposal apply runs through the Rust `apply_build_proposal` command and can
  create or append chart, table, text, gauge, and image widgets.
- Generated widgets are backed by persisted datasource workflows and refresh
  through the existing Rust workflow runtime.
- Chat provider calls emit OpenAI-compatible tool schemas, parse tool calls,
  execute safe built-in `http_request` calls through `ToolEngine`, persist
  visible tool results/errors, and perform one bounded provider resume call.
- MCP stdio enable now fails on initialize or `tools/list` timeout/error rather
  than storing a fake connected state.
- Persisted enabled stdio MCP servers can be reconnected explicitly and are
  auto-connected before tool listing or tool calls.
- `local_mock` remains clearly labeled as deterministic local dev/test smoke
  behavior.

## Files Changed

- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src-tauri/src/lib.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/mcp.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/mcp_manager.rs`
- `README.md`
- `docs/RESIDUAL_BACKLOG.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W12_PROVIDER_DRIVEN_AGENTIC_DASHBOARD_BUILDER.md`

## Validation

Run from `datrina/`:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 39 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed; Vite reported only the existing non-failing chunk-size warning.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets`: passed.

## Acceptance Notes

- Real provider support is implemented through the existing OpenRouter,
  Ollama, and custom OpenAI-compatible runtime paths, but no live external
  provider credentials/service were available in this validation run.
- Build Chat no longer exposes hardcoded local apply buttons; it applies only
  provider-generated structured proposals through Rust after confirmation.
- Denied `http_request` tool calls are returned as persisted tool results with
  explicit policy/runtime errors.
- MCP stdio connection cannot succeed silently after initialize or `tools/list`
  failures.
- Full manual widget editing forms, widget post-process execution, streaming
  chat events, remote MCP transport, and multi-step arbitrary agent loops remain
  residual/post-MVP work.
