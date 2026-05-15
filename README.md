# Datrina The Lenswright

Datrina is a local-first Tauri v2 desktop app for building and refreshing AI-assisted dashboards. The current product runtime is a desktop application with a React UI, Rust command backend, SQLite persistence, Rust-mediated chat providers, provider-generated build proposals, explicit proposal confirmation, policy-gated tool/workflow execution, and deterministic local dev/test dashboard paths.

The active implementation boundary is:

`React UI -> src/lib/api.ts -> Tauri invoke/listen -> Rust commands -> AppState modules -> SQLite/process/network runtimes`

## Current MVP Behavior

Implemented and validated in the reconciliation baseline:

- Local Tauri v2 desktop app with React 19, TypeScript, Tailwind, and Rust.
- SQLite persistence for dashboards, chat sessions, workflows, workflow runs, providers, MCP servers, and app config.
- Typed frontend/Rust API wrapper in `src/lib/api.ts`, with a static command registration check.
- Dashboard list/create/load/update/delete flows.
- Dashboard layout drag and resize persistence.
- Five widget renderers: chart, text, table, image, and gauge.
- Built-in `local_mvp` dashboard template with a deterministic local datasource workflow and gauge widget.
- Widget refresh through Rust, including persisted workflow run state and `workflow:event` emission.
- Rust-mediated provider and chat boundary for OpenRouter, Ollama, OpenAI-compatible custom providers, and deterministic `local_mock` dev/test smoke behavior.
- Provider add/update/re-key/enable/disable/test flows through Rust commands; provider secrets are not returned to React.
- Context chat is grounded with the selected dashboard, widgets, and workflow run state before provider execution.
- Build chat requests provider-generated structured dashboard/widget proposals with explicit executable datasource plans, previews them in the UI, and applies them only after explicit confirmation through Rust commands.
- OpenAI-compatible chat providers stream assistant content through typed Tauri
  `chat:event` events with canonical agent-event payloads and React-side typed
  message parts while Rust remains the only provider caller.
- Provider-supplied public reasoning fields, when present, render in a separated visible reasoning region without requesting or exposing hidden chain-of-thought.
- Proposal apply can create chart, table, text, gauge, and image widgets with persisted datasource workflows backed by `ToolEngine`, stdio MCP, or Rust-mediated provider execution.
- Provider-driven chat tool calling emits OpenAI-compatible tool schemas,
  executes safe built-in `http_request` calls and configured stdio MCP tool
  calls through the Rust policy gateway, streams masked typed tool activity
  parts, persists visible tool results/errors, and resumes once for the final
  assistant response.
- Dashboard widget creation UI for local text and gauge widgets, each backed by a deterministic persisted workflow.
- Workflow `llm` nodes execute through the same Rust AI provider runtime used by chat.
- Workflow MCP/built-in tool nodes reconnect persisted enabled stdio MCP servers before execution, then execute through the Rust `ToolEngine`/MCP gateway or fail with explicit policy/runtime errors.
- Scheduler cron jobs are started, restored from persisted workflows at app startup, updated/unscheduled when workflow commands change jobs, and execute through the same persisted workflow run path as manual execution.
- React surfaces workflow refresh state from `workflow:event` so operators can see running, success, and error outcomes without opening storage.
- Honest no-key chat behavior: without an enabled provider or `local_mock` dev/test provider, chat returns an unavailable/error state instead of fake assistant output.
- Stdio MCP server configuration, persisted server records, process lifecycle plumbing, and validation through the Rust tool policy gateway.
- Stdio MCP enable fails on initialize or `tools/list` timeout/error; enabled persisted stdio servers are reconnected explicitly through the MCP command or automatically before tool listing/calls.
- Tool security baseline with one Rust gateway for built-in tools, stdio MCP process validation, network URL policy, and audit logging.

In-progress or intentionally limited in the MVP:

- Widget post-process steps are unavailable in the current product path.
- Chat supports a bounded one-resume tool loop for safe built-in HTTP requests and configured stdio MCP tools; arbitrary multi-step agent loops are not enabled.
- `local_mock` and Ollama chat paths use synthetic single-step chat events rather than live token streaming.
- Real-provider behavior requires user-provided credentials/service availability; credential-free validation uses `local_mock` dev/test mode and is not live acceptance evidence.
- Provider and MCP secrets are Rust-owned and masked before reaching React, but the MVP fallback stores them as local-only plaintext SQLite data under the Tauri app data directory. Encrypted OS keychain/keyring storage is a production follow-up.
- Production packaging is not restored in the baseline; bundle packaging and final icon sets are deferred.

Planned, not MVP-supported:

- Remote MCP transports and remote MCP hardening.
- Public HTTP/REST API.
- Plugin SDK and marketplace.
- Arbitrary sandboxed JavaScript execution.
- DuckDB analytics.
- OAuth, teams, cloud sync, and multi-user auth.
- Advanced workflow queues, retries, dead-letter behavior, cancellation commands, and a visual workflow editor.
- Production distribution packages.

## Architecture

```
┌─────────────────────────────────────────────┐
│  DESKTOP APP (Tauri v2)                     │
│  ┌─────────────────────────────────────┐    │
│  │  Frontend (React 19 + Tailwind)     │    │
│  │  ├── DashboardGrid                  │    │
│  │  ├── Widget renderers               │    │
│  │  ├── ChatPanel                      │    │
│  │  └── src/lib/api.ts                 │    │
│  └─────────────────────────────────────┘    │
│              ↑ Tauri Commands / Events      │
│  ┌─────────────────────────────────────┐    │
│  │  Backend (Rust)                     │    │
│  │  ├── Storage (SQLite + sqlx)        │    │
│  │  ├── Workflow Engine (local DAG)    │    │
│  │  ├── AI Provider Runtime            │    │
│  │  ├── MCP Manager (stdio baseline)   │    │
│  │  ├── Tool Engine (policy gateway)   │    │
│  │  └── Scheduler (persisted runner)   │    │
│  └─────────────────────────────────────┘    │
└─────────────────────────────────────────────┘
```

## Tech Stack

| Layer | Technology |
| --- | --- |
| Desktop | Tauri v2 |
| Frontend | React 19, TypeScript, Tailwind CSS |
| Widgets/charts | Recharts plus local widget renderers |
| Backend | Rust, Tokio async |
| Storage | SQLite through `sqlx` |
| AI | Rust-mediated OpenRouter, Ollama, OpenAI-compatible custom providers, `local_mock` |
| MCP | Stdio baseline with Rust policy validation |
| Scheduling | `tokio-cron-scheduler` with persisted workflow execution |

## Prerequisites

- Rust latest stable.
- Bun, or Node.js 22+ for frontend commands.
- Tauri CLI for desktop runs/builds: `cargo install tauri-cli`.
- Native Tauri platform prerequisites for your OS.
- Network access only when installing dependencies or using real external providers/MCP servers. Baseline validation does not require real LLM credentials, real external MCP servers, Docker, cloud services, or production packaging.

## Local Development

Run from this `datrina/` directory:

```bash
bun install
bun run tauri:dev
```

Useful validation commands:

```bash
node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"
bun run check:contract
bun run typecheck
bun run build
cargo fmt --all --check
cargo check --workspace --all-targets
```

Expected baseline notes:

- `bun run build` may report a non-failing Vite chunk-size warning.
- `src-tauri/tauri.conf.json` has bundle packaging disabled for the baseline.

## Configuration

### Providers

Provider config is persisted by Rust and returned to React with secrets removed or masked. The supported provider kinds are:

- `openrouter`: OpenAI-compatible `/chat/completions`, requires an API key.
- `custom`: OpenAI-compatible `/chat/completions`, API key optional for local compatible endpoints.
- `ollama`: local Ollama `/api/chat` and `/api/tags`.
- `local_mock`: deterministic no-key/no-network smoke provider.

### MCP Servers

MVP MCP transport is stdio-only. Saved server config is validated through the Rust tool policy gateway before connection. Remote MCP transports are deferred.

Allowed stdio process commands in the current policy baseline:

- `node`
- `npx`
- `bun`
- `bunx`
- `uvx`

## Reconciliation Docs

Agent execution and reconciliation history live in `docs/`:

- `docs/RECONCILIATION_PLAN.md`: historical reconciliation execution contract.
- `docs/DECISIONS.md`: accepted P0 runtime decisions and post-MVP exclusions.
- `docs/W*_*.md`: accepted baseline reports from individual workstreams.
- `docs/RESIDUAL_BACKLOG.md`: concise backlog for deferred work after the MVP baseline.
- `docs/W9_DOCS_CLOSEOUT.md`: W9 closeout and validation record.
- `docs/W10_END_TO_END_PRODUCT_RUNTIME.md`: W10 implementation and validation record.
- `docs/W11_OPENER_PLUGIN_MIGRATION.md`: W11 opener migration validation record.
- `docs/W12_PROVIDER_DRIVEN_AGENTIC_DASHBOARD_BUILDER.md`: W12 provider proposal validation record.
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`: W13 durable runtime validation record.
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`: W14 chat streaming and observability validation record.
- `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`: W15 chat runtime and message-parts validation record.

## Project Structure

```
datrina/
├── src/                      # Frontend (React)
│   ├── App.tsx               # Main app
│   ├── lib/api.ts            # Tauri API wrapper
│   ├── components/           # Layout and widgets
│   └── hooks/                # React hooks
├── src-tauri/                # Backend (Rust)
│   ├── src/
│   │   ├── main.rs           # Entry point
│   │   ├── lib.rs            # AppState and command registration
│   │   ├── commands/         # Tauri command handlers
│   │   ├── modules/          # Storage, AI, MCP, workflow, scheduler, tools
│   │   └── models/           # Rust source-of-truth models
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/                     # Reconciliation plan and baseline records
├── package.json
├── Cargo.toml
└── vite.config.ts
```

## License

Functional Source License 1.1 (FSL-1.1).

- Source code is publicly available.
- Free for local use, commercial and non-commercial.
- Converts to Apache 2.0 after 2 years.
- Cannot be offered as a competing SaaS product.
- Cannot be rebranded for commercial distribution.

See `LICENSE.md` when present in this checkout for the full license text.
