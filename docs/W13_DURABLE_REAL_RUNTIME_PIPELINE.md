# W13 Durable Real Runtime Pipeline

Status: implemented with external-provider validation residual on 2026-05-15.

## Scope Handled

W13 replaces the remaining generated-widget static datasource path with one
durable runtime pipeline:

- Build Chat proposal schema now includes an executable `datasource_plan` for
  every generated widget.
- Proposal apply rejects widgets without an executable datasource plan, then
  persists workflow-backed datasource state through Rust commands.
- Supported generated datasource plan kinds are `builtin_tool`, `mcp_tool`, and
  `provider_prompt`.
- Literal proposal `data` is kept only as preview/config inference input; it is
  not the persisted datasource runtime.
- Chat tool calling advertises the safe built-in `http_request` tool and adds a
  generic stdio MCP tool call when enabled MCP tools are connected or
  reconnectable.
- Provider-requested MCP tool calls reconnect persisted enabled stdio servers
  through `MCPManager`, validate through `ToolEngine`, and persist visible tool
  results/errors.
- Workflow manual execution, widget refresh, and scheduled execution reconnect
  enabled persisted stdio MCP servers before MCP workflow nodes run.
- The cron scheduler now starts its runner, reloads enabled persisted cron
  workflows at app startup, replaces duplicate jobs when workflow commands
  recreate jobs, unschedules jobs on delete, and executes scheduled runs through
  the same persisted runner path as manual refresh.
- React listens to `workflow:event` and shows per-widget workflow run state in
  the dashboard shell.

## Files Changed

- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src-tauri/src/lib.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/workflow.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/modules/scheduler.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `README.md`
- `docs/RESIDUAL_BACKLOG.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`

## Validation

Run from `datrina/`:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 39 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed; Vite reported only the existing non-failing chunk-size warning.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `rg -n "TODO|not implemented|placeholder|unsupported|local_mock|unavailable" src src-tauri/src`: reviewed. Remaining hits are explicit dev/test labels, explicit unsupported/post-MVP exclusions, or UI empty/error states outside W13 acceptance.

## Acceptance Notes

- Real provider runtime support is implemented through the existing OpenRouter,
  Ollama, and custom OpenAI-compatible paths, and W13-generated
  `provider_prompt` workflow nodes execute through the same Rust AI runtime.
- No live external provider credentials or reachable local real provider were
  provided in this checkout, so the real-provider acceptance lane is recorded as
  blocked by environment. `local_mock` was not used as W13 acceptance evidence.
- Generated widget runtime now depends on executable datasource plans. Provider
  proposals that only include literal `data` fail apply with an explicit error.
- Scheduler behavior is code-validated by build checks; live wall-clock cron
  execution remains dependent on running the desktop app event loop.
