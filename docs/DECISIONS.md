# Datrina Reconciliation Decisions

Status: accepted source lock for W0

This file records the P0 implementation decisions for reconciling root-level
research and planning with the active `datrina` Tauri application. These
decisions are binding for W1-W9 unless a later workstream explicitly reopens
one with a new decision record.

## Accepted Runtime Boundary

`datrina` is a local-first Tauri v2 desktop application:

`React UI -> src/lib/api.ts -> Tauri invoke/listen -> Rust commands -> AppState modules -> SQLite/process/network runtimes`

Accepted:

- React owns rendering, local UI state, widget layout interaction, and event
  subscription.
- Rust owns secrets, provider calls, MCP process lifecycle, workflow execution,
  scheduler, persistence, and external process/network access.
- `src/lib/api.ts` is the frontend boundary. React components must not bypass it
  to call Rust commands directly unless a workstream records the exception.
- Rust command registration in `src-tauri/src/lib.rs` is the backend boundary.

Rejected for MVP:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public REST API as the primary app boundary.
- Browser-side provider SDK calls with API keys in React.

## P0 Decisions

### D0.1 - AI Boundary

Status: Accepted

AI provider calls are Rust-mediated. W6 may add a Rust `ai`/provider module or
extend provider/chat commands, but React must not read provider secrets or call
external LLM APIs directly. Chat may be unavailable without configured local
credentials, but it must not return fake assistant success.

### D0.2 - Shared Schema Source Of Truth

Status: Accepted

For MVP, Rust models are the source of truth for command payloads and persisted
entities. TypeScript types in `src/lib/api.ts` are a manually synchronized
frontend mirror. W2 must add static checks or fixtures that compare command
names and the highest-risk request/response shapes. Generated TypeScript from
Rust may be added by a later workstream, but W1-W9 must not block on it.

### D0.3 - Storage And Key Policy

Status: Accepted

SQLite through `sqlx` is the MVP persistence baseline. Storage lives under the
Tauri app data directory. Migrations may remain Rust-managed during W1-W3, but
W3 must document the migration location and whether table creation is embedded
or file-based.

Secrets and MCP environment values are Rust-owned. The target policy is encrypted
OS keychain/keyring storage when available, with an explicit local-only fallback
recorded if encryption is not implemented in MVP. React may receive only masked
metadata or non-secret configuration values.

JSON import/export is allowed only as an explicit interchange format. It is not
the primary runtime database.

### D0.4 - Tool And MCP Security Scope

Status: Accepted

MVP MCP support is stdio-only. HTTP/SSE remote MCP transport is deferred.

All tool execution must pass through one Rust policy gateway before reaching
built-in tools, MCP tools, process execution, or network access. W5 owns the
gateway shape. The MVP policy must include:

- command allowlist for process-backed tools,
- URL/network allowlist for network-capable tools,
- audit log event shape for accepted and rejected calls,
- explicit unsupported errors for tool kinds outside the MVP scope.

Saved MCP server configs must not become arbitrary command execution without
the accepted allowlist check.

### D0.5 - Workflow Semantics

Status: Accepted

Workflow execution is Rust-owned and persisted. MVP workflow support is a
deterministic local DAG runner with a small expression surface for static
mapping, filtering, and transforms only where needed by the vertical slice.

Tauri events replace research SSE channels. W7 owns the exact event envelope,
but it must include stable event names and typed payloads for workflow progress,
workflow completion/failure, widget data updates, and audit/security events.

Minimum MVP run semantics:

- workflow run state is persisted,
- unsupported node kinds return explicit errors,
- cancellation is accepted at the command/model level if exposed and may be
  implemented as best-effort stop before the next node,
- retry is limited to explicit per-node or per-run retry fields if already
  modeled; otherwise complex retry policy is deferred.

### D0.6 - Scheduler Scope

Status: Accepted

The scheduler is Rust-owned. In MVP it either triggers persisted workflow runs
through the same workflow engine as manual execution, or it is clearly marked
registration-only. It must not report fake execution.

### D0.7 - Dashboard And Widget Runtime

Status: Accepted

Dashboard and widget layout state is React-owned at interaction time and
persisted through Rust commands. Runtime widget data comes from commands,
workflow outputs, or typed Tauri events. Sample/demo data may remain only when
isolated and labeled as sample-only in code or residual docs.

### D0.8 - Chat Runtime

Status: Accepted

Chat uses the Rust-mediated AI boundary from D0.1. Without a configured
supported provider, chat returns a deterministic unavailable/error state. Hidden
placeholder assistant messages are rejected.

## Tauri Adaptation Map

| Research concept | MVP adaptation in `datrina` | Status |
| --- | --- | --- |
| Hono REST API | Tauri commands invoked through `src/lib/api.ts` | Accepted |
| REST request/response DTOs | Rust command structs plus mirrored TypeScript types | Accepted |
| SSE streams | Typed Tauri `listen` events with stable event names and payload envelopes | Accepted |
| AI Engine/provider calls | Rust-mediated provider/chat module; no React-side secrets | Accepted |
| MCP tools | Stdio MCP process lifecycle behind Rust policy gateway | Accepted |
| Remote MCP HTTP/SSE | Not in MVP | Deferred |
| Workflow engine | Rust deterministic local DAG runner with persisted runs | Accepted |
| Dashboard state | React layout state plus Rust persistence and runtime data commands/events | Accepted |
| Chat | Rust-mediated provider path or explicit unavailable state | Accepted |
| Storage | SQLite via `sqlx` in Tauri app data dir | Accepted |
| Secrets | Rust-owned key storage; encrypted target, explicit local fallback if needed | Accepted |
| Tool security | Rust command/network allowlists and audit event shape | Accepted |
| Scheduler | Rust scheduler triggering persisted runs or explicit registration-only scope | Accepted |
| Plugin SDK/marketplace | Not in MVP | Deferred |
| Arbitrary sandboxed JS | Not in MVP | Rejected |
| DuckDB analytics | Not in MVP | Deferred |
| OAuth/team auth | Not in MVP | Deferred |
| Public external API | Not in MVP | Deferred |

## Explicit Post-MVP Promises

These capabilities must not be implied as working MVP behavior until a later
workstream adds and validates them:

- public HTTP/REST API,
- Node/Hono/Turborepo runtime,
- remote MCP transport and production remote MCP hardening,
- plugin SDK and marketplace,
- arbitrary sandboxed JavaScript,
- DuckDB analytics,
- OAuth, teams, cloud sync, or multi-user auth,
- advanced workflow expression language, queues, retries, and dead-letter
  behavior,
- production packaging and distribution.
