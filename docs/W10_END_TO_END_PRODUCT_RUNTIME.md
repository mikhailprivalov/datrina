# W10 End-To-End Product Runtime

Status: implemented with recorded residuals

## Scope Handled

W10 promoted selected post-W9 residuals into the local-first product runtime while preserving the Tauri/Rust boundary:

- provider update, re-key, enable, disable, and test flows,
- provider request timeout, provider-specific OpenRouter headers, structured provider error prefixes, token usage capture when available, and latency metadata,
- context chat grounding from selected dashboard, widgets, and workflow run state,
- explicit build-apply commands for creating a local dashboard and adding local text/gauge widgets,
- dashboard widget add UI beyond the built-in `local_mvp` template,
- workflow `llm` nodes through the Rust-mediated AI provider runtime,
- workflow MCP and built-in tool nodes through `ToolEngine`/MCP gateways,
- scheduler cron execution through the same persisted workflow run path as manual runs.

## Files Changed

- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src-tauri/src/lib.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/provider.rs`
- `src-tauri/src/commands/workflow.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/provider.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/scheduler.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `README.md`
- `docs/RESIDUAL_BACKLOG.md`
- `docs/RECONCILIATION_PLAN.md`

## Validation

Run from `datrina/`:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 37 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed; Vite reported only the existing non-failing chunk-size warning.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets`: passed; Rust reported only the existing non-failing deprecation warning for `tauri_plugin_shell::Shell::open`.

## Acceptance Notes

- Credential-free local behavior is available through `local_mock`, explicit build apply controls, local text/gauge widget creation, deterministic datasource workflows, persisted workflow runs, and `workflow:event` emission.
- Real provider runtime is implemented for OpenRouter, Ollama, and custom OpenAI-compatible endpoints, but live external credentials/services were not available in this validation run.
- Agentic workflow tool nodes are policy-gated through `ToolEngine`/MCP. Provider-driven chat tool schema emission and bounded provider tool-call resume loops remain residual.
- Widget post-process execution remains residual; unsupported post-process steps still fail explicitly.
