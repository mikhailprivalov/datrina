# Datrina Reconciliation Plan

Status: draft for agent execution

## Purpose

This plan reconciles the existing `datrina` Tauri implementation with the root-level planning and research materials. It is an execution plan for agents working inside `datrina`, not a product roadmap or a replacement for public documentation.

Agents executing this plan must read `AGENTS.md` in the `datrina` directory first.

The immediate goal is to turn the current scaffold into a coherent local-first Tauri application baseline:

1. one accepted runtime boundary,
2. one typed frontend/Rust contract,
3. working build and validation gates,
4. no hidden placeholder behavior in the MVP vertical slice.

## Source Of Truth

| Source | Use as | Do not use as |
| --- | --- | --- |
| `datrina/` implementation | Current code reality and path ownership | Proof that all README capabilities are already implemented |
| `plan-v2.md` | Tauri/Rust direction and desktop adaptation intent | Fully executable task queue |
| `research/SPEC.md` and `research/architecture.md` | Product concepts, contracts, risk inventory | Target Node/Hono/Turborepo runtime for `datrina` |
| `plan.md` | Historical pre-research planning context | Current implementation plan |
| `datrina/README.md` | Public product description and broad roadmap | Detailed agent execution checklist |

Locked reconciliation decision: `datrina` continues as a Tauri v2 desktop app with React frontend and Rust backend. Node/Hono REST and SSE concepts from research are translated into Tauri commands and events for MVP.

## Target Runtime Boundary

The target boundary is:

`React UI -> src/lib/api.ts -> Tauri invoke/listen -> Rust commands -> AppState modules -> SQLite/process/network runtimes`

P0 decisions to preserve:

- Hono/REST is out of MVP unless a later external API workstream is explicitly added.
- SSE is replaced by typed Tauri events with stable channel names and payload envelopes.
- Rust owns secrets, provider calls, MCP process lifecycle, workflow execution, scheduler, and persistence.
- React owns rendering, local UI state, widget layout interaction, and event subscription.
- Plugin SDK, remote MCP hardening, arbitrary sandboxed JS, DuckDB analytics, OAuth/team auth, and marketplace behavior are post-MVP.

Accepted P0 decisions are recorded in `docs/DECISIONS.md`:

- Accepted: AI provider calls are Rust-mediated; React never owns provider secrets or direct external LLM calls.
- Accepted: Rust models are the MVP schema source of truth, mirrored manually in TypeScript with W2 static checks.
- Accepted: SQLite through `sqlx` under the Tauri app data directory is the MVP persistence baseline.
- Accepted: secrets and MCP environment values are Rust-owned, with encrypted OS key storage as the target and any local-only fallback documented by W3.
- Accepted: MVP MCP is stdio-only behind one Rust tool policy gateway with command/network allowlists and audit events.
- Accepted: workflow execution is a Rust-owned persisted local DAG runner; typed Tauri events replace research SSE channels.
- Deferred: remote MCP transport, public HTTP API, plugin SDK/marketplace, DuckDB analytics, OAuth/team auth, cloud sync, and advanced workflow queues/retry/dead-letter behavior are post-MVP.

## Current Snapshot

Known implementation facts to preserve while reconciling:

- Frontend/backend boundary already runs through `datrina/src/lib/api.ts`.
- Rust command registration is centralized in `datrina/src-tauri/src/lib.rs`.
- Tauri setup currently initializes storage, MCP manager, and scheduler in `datrina/src-tauri/src/main.rs`.
- `AppState` currently stores `storage`, `mcp_manager`, `scheduler`, `tool_engine`, and `ai_engine`.
- SQLite table creation already exists for dashboards, chat, workflows, providers, MCP servers, config, and workflow runs.
- MCP stdio process lifecycle exists partially in `mcp_manager.rs`.
- Workflow, tool, scheduler, widget refresh, and dashboard data binding now use explicit runtime paths or explicit unavailable/post-MVP errors.
- Chat/provider behavior is Rust-mediated after W6: no provider returns explicit unavailable/error state, and `local_mock` is the only deterministic no-key success path.

Known immediate blockers:

- Workflow MCP/LLM node execution remains unavailable for MVP and returns explicit errors.
- Remaining advanced tool, scheduler, and generated-dashboard behavior must be handled by residual/post-MVP work instead of hidden success paths.

## Reconciliation Gates

### G0 - Source Lock

Exit criteria:

- This document explicitly states Tauri/Rust as the target.
- Research concepts are mapped to Tauri commands/events or marked post-MVP.
- No workstream asks agents to implement Node/Hono/Turborepo inside `datrina`.

### G1 - Build Baseline

Exit criteria:

- Tauri config parses.
- Cargo manifests parse.
- Rust async initialization model is coherent.
- Local validation commands have known expected results.

Suggested checks:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"` from `datrina`.
- `bun run typecheck`.
- `bun run build`.
- `cargo fmt --all --check`.
- `cargo check --workspace --all-targets`.

If Rust toolchain or dependencies are unavailable, record that as environment status instead of treating downstream feature work as complete.

### G2 - Contract Baseline

Exit criteria:

- Every command name used in `src/lib/api.ts` is registered in Rust.
- Request payload casing is intentional and tested.
- `ApiResult<T>`, `ApiResult<Option<T>>`, and void command behavior are defined.
- Shared model ownership is documented.

Suggested checks:

- Static command-name comparison between `src/lib/api.ts` and `src-tauri/src/lib.rs`.
- Targeted typecheck after contract edits.
- Unit tests or static fixtures for nullable success responses and nested request payloads.

### G3 - Persistence Baseline

Exit criteria:

- CRUD commands for dashboards, chat sessions/messages, workflows, providers, MCP servers, and config either work or are explicitly out of scope.
- Delete/test/remove placeholder commands are implemented or moved to post-MVP.
- Workflow runs are persisted when workflows execute.
- Secrets and MCP environment values have an explicit key-storage policy.

### G4 - Runtime Engine Baseline

Exit criteria:

- `AppState` exposes the runtime modules required by commands.
- Tool execution goes through one policy gateway.
- Workflow node execution can call local tool/MCP/provider abstractions through explicit interfaces.
- Scheduler can trigger a persisted workflow run or is explicitly limited to registration-only scope.
- Tauri event envelope is defined and emitted by at least one workflow/widget path.

### G5 - Local MVP Smoke

Exit criteria:

- App can build without real LLM keys or real MCP servers.
- A local dashboard can be created, persisted, loaded, and updated.
- One workflow can execute with a deterministic local/mock tool path and update one widget through the event bridge.
- Chat mode behavior is honest: either wired to an AI provider or clearly marked as unavailable in UI/runtime.

## Agent Workstreams

Each workstream below is scoped for one implementation agent unless marked as review-only. Agents must not edit outside their ownership paths without updating this plan or handing off to the owning workstream.

### W0 - Source Lock And Decision Record

Status: accepted

Depends on: none

Owner role: planning/docs agent

Ownership:

- `datrina/docs/RECONCILIATION_PLAN.md`
- optional `datrina/docs/DECISIONS.md`

Scope:

- Convert the open decisions in this file into short accepted decisions.
- Record the Tauri adaptation map for Hono/REST, SSE, AI Engine, MCP, workflow, dashboard, chat, storage, tool security, scheduler, and plugins.
- Mark post-MVP promises explicitly.

Out of scope:

- Code changes.
- README marketing rewrite.

Acceptance checks:

- All P0 open decisions have `Accepted`, `Rejected`, or `Deferred` status.
- No workstream depends on an unresolved runtime boundary.
- Decision record exists at `docs/DECISIONS.md`.

Parallelism:

- Serial. Do this before functional coding beyond build repair.

### W1 - Build And Config Baseline

Status: accepted

Depends on: W0

Owner role: bootstrap implementation agent

Ownership:

- `datrina/src-tauri/tauri.conf.json`
- `datrina/src-tauri/Cargo.toml`
- `datrina/Cargo.toml`
- `datrina/package.json`
- `datrina/src-tauri/src/main.rs`
- build/bootstrap notes under `datrina/docs/`

Scope:

- Repair invalid Tauri config.
- Repair Cargo manifest issues.
- Make storage/runtime initialization compile-ready.
- Document which validation commands are expected to pass in the current environment.

Out of scope:

- Feature implementation.
- API contract redesign beyond compile blockers.

Acceptance checks:

- Tauri config JSON parse succeeds.
- Cargo manifests parse.
- `bun run typecheck` either passes or fails only on listed dependency/environment constraints.
- `cargo check --workspace --all-targets` passes when Rust toolchain/deps are available, or blockers are recorded with exact errors.

Validation record:

- Accepted baseline is recorded in `docs/W1_BUILD_BASELINE.md`.

Parallelism:

- Serial. Do before W2-W7 implementation.

### W2 - Frontend/Rust Contract Baseline

Status: accepted

Depends on: W1

Owner role: contract implementation agent

Ownership:

- `datrina/src/lib/api.ts`
- `datrina/src-tauri/src/models/*`
- `datrina/src-tauri/src/commands/*`
- `datrina/src-tauri/src/lib.rs`
- contract tests or static check scripts if added

Scope:

- Align command names, nested request casing, nullable responses, void responses, and error shape.
- Define how TypeScript and Rust models stay synchronized.
- Ensure every frontend API wrapper maps to one registered command with matching request/response semantics.

Out of scope:

- Deep engine implementation.
- UI redesign.
- Secret storage implementation beyond model shape.

Acceptance checks:

- Static command-name check passes.
- Nullable getters such as `get_config` and missing-dashboard/session/workflow paths have explicit success/error behavior.
- Existing frontend API calls compile against the chosen contract.
- Remaining manual model drift is listed as W2 follow-up or moved into a generated-types task.

Validation record:

- Accepted baseline is recorded in `docs/W2_CONTRACT_BASELINE.md`.

Parallelism:

- Serial immediately after W1. Other agents may read outputs but should not edit command/model/API paths concurrently.

### W3 - Storage, Config, And Secrets Baseline

Status: accepted

Depends on: W2

Owner role: storage implementation agent

Ownership:

- `datrina/src-tauri/src/modules/storage.rs`
- storage-related parts of `datrina/src-tauri/src/commands/dashboard.rs`
- storage-related parts of `datrina/src-tauri/src/commands/chat.rs`
- storage-related parts of `datrina/src-tauri/src/commands/provider.rs`
- storage-related parts of `datrina/src-tauri/src/commands/mcp.rs`
- storage-related parts of `datrina/src-tauri/src/commands/workflow.rs`
- storage-related parts of `datrina/src-tauri/src/commands/config.rs`

Scope:

- Make storage initialization async-safe.
- Complete CRUD behavior for MVP entities or mark commands unavailable.
- Persist workflow runs and `last_run` data if workflow execution is exposed.
- Define what is stored in SQLite, what is exported as JSON, and how secrets/API keys/MCP env values are stored.

Out of scope:

- Provider network calls.
- MCP process hardening beyond persistence shape.
- Full import/export UX.

Acceptance checks:

- CRUD smoke tests or targeted command tests for dashboard/config/provider/MCP/workflow persistence.
- Delete/remove commands either work or return explicit unsupported errors.
- Plaintext secret residuals are listed with mitigation task or accepted local-only rationale.

Validation record:

- Accepted baseline is recorded in `docs/W3_STORAGE_BASELINE.md`.

Parallelism:

- Can run in parallel with W4 after W2.
- Coordinate with W5/W6 before changing workflow/provider/MCP model shapes.

### W4 - Dashboard Local UX And Data Plumbing

Status: accepted

Depends on: W2

Owner role: frontend runtime agent

Ownership:

- `datrina/src/App.tsx`
- `datrina/src/components/layout/*`
- `datrina/src/components/widgets/*`
- frontend-only state helpers if added

Scope:

- Separate presentational widget config from runtime widget data.
- Wire dashboard refresh and layout persistence to contract-approved APIs.
- Add loading/error/empty behavior for local MVP flows.
- Subscribe to the chosen Tauri event envelope once W7 defines it, or add a narrow placeholder interface blocked on W7.

Out of scope:

- Rust model edits.
- AI/chat/provider implementation.
- Major visual redesign.

Acceptance checks:

- Dashboard list/create/load/update path works against local commands or documented mocks.
- Widget demo data is either replaced by explicit runtime data props or clearly isolated as sample-only.
- `bun run typecheck` passes after frontend changes when dependencies are available.

Validation record:

- Accepted baseline is recorded in `docs/W4_DASHBOARD_LOCAL_UX.md`.

Parallelism:

- Can run in parallel with W3.
- Event subscription portion waits for W7.

### W5 - MCP And Tool Security Baseline

Status: accepted

Depends on: W2, W3

Owner role: runtime/security implementation agent

Ownership:

- `datrina/src-tauri/src/modules/mcp_manager.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- `datrina/src-tauri/src/commands/mcp.rs`
- `datrina/src-tauri/src/commands/tool.rs`
- relevant capability/security config under `datrina/src-tauri/`

Scope:

- Enforce the W0 decision that MVP MCP is stdio-only.
- Add a single tool gateway for MCP and built-in tools.
- Define command allowlist, network/URL allowlist, and audit log behavior.
- Connect `ToolEngine` to `AppState` if it is the accepted gateway.
- Make `test_tool`, MCP tool call, and MCP server remove/test behavior honest.

Out of scope:

- Remote MCP production hardening.
- Dynamic plugin SDK.
- Arbitrary sandboxed JS.

Acceptance checks:

- Built-in tool command goes through `ToolEngine` or returns explicit unsupported status.
- MCP server configs cannot execute arbitrary saved commands without the accepted allowlist/policy check.
- Audit event shape is defined for successful and rejected tool calls.
- Static security review covers `tauri.conf.json`, `system.rs`, `mcp_manager.rs`, `tool_engine.rs`, `provider.rs`, and `TextWidget.tsx`.

Validation record:

- Accepted baseline is recorded in `docs/W5_TOOL_SECURITY_BASELINE.md`.

Parallelism:

- Can overlap with W6 after W3, but interface changes must be coordinated through W2 contract ownership.

### W6 - AI Provider And Chat Boundary

Status: completed

Depends on: W0, W2, W3

Owner role: AI/runtime implementation agent

Ownership:

- `datrina/src-tauri/src/commands/chat.rs`
- `datrina/src-tauri/src/commands/provider.rs`
- provider-related models in `datrina/src-tauri/src/models/provider.rs`
- any new `datrina/src-tauri/src/modules/ai*` module
- chat-facing frontend call sites only when needed

Scope:

- Implement the W0 AI boundary decision.
- Make provider testing honest: real supported providers, local mock, or explicit unsupported state.
- Define streaming behavior through Tauri events if streaming is accepted for MVP.
- Replace placeholder assistant responses with either a real provider path or a truthful unavailable response.

Out of scope:

- Full multi-step agent orchestration.
- Tool calling until W5 is accepted.
- Prompt marketplace or provider UI polish.

Acceptance checks:

- Chat command behavior is no longer a hidden placeholder.
- Provider config validation and test path are explicit.
- No API keys are read by React if Rust-owned secrets is accepted.
- Local no-key smoke returns deterministic unavailable/error state, not fake success.

Parallelism:

- Can overlap with W5 only before tool calling is wired.
- Full Build/Context chat waits for W8.

### W7 - Workflow, Scheduler, And Event Envelope

Status: completed

Depends on: W2, W3, W5

Owner role: workflow implementation agent

Ownership:

- `datrina/src-tauri/src/modules/workflow_engine.rs`
- `datrina/src-tauri/src/modules/scheduler.rs`
- `datrina/src-tauri/src/commands/workflow.rs`
- event envelope definitions under models/modules if added

Scope:

- Define typed Tauri event envelope replacing research SSE channels.
- Persist workflow run state.
- Execute deterministic local nodes without MCP/AI first.
- Connect MCP/tool nodes through W5 interfaces when ready.
- Make scheduler trigger persisted workflow runs or explicitly restrict scheduler scope.
- Define minimum cancellation/retry behavior.

Out of scope:

- LangGraph runtime.
- Full priority/dead-letter queue.
- Complex expression language beyond MVP filters/transforms accepted in W0.

Acceptance checks:

- One local workflow executes and persists a run.
- At least one workflow progress/result event is emitted with the accepted envelope.
- Scheduler registration either triggers a workflow or is marked registration-only with no fake execution.
- Remaining unsupported node kinds return explicit errors.

Completion notes:

- Accepted event channel: `workflow:event`.
- MVP retry policy: no automatic retry; failed nodes end the run with `status: error`.
- MVP cancellation policy: no cancellation command is exposed yet; cancellation is deferred to W8/W9 residual handling if needed for the vertical slice.
- Scheduler is registration-only in this baseline and logs cron matches without pretending to execute workflow runs.

Parallelism:

- Starts after W5 interface is stable.
- Frontend event subscription in W4 can integrate once envelope is accepted.

### W8 - MVP Vertical Slice

Status: accepted

Depends on: W3, W4, W5, W6, W7

Owner role: integration agent

Ownership:

- Cross-cutting integration only.
- Must avoid broad rewrites in owned workstream paths unless coordinating with the owner notes.

Scope:

- Validate the first local product path:
  1. create/load dashboard,
  2. run one deterministic workflow,
  3. update one widget through event/data plumbing,
  4. use chat in an honest mode,
  5. record residual unavailable features.

Out of scope:

- Production packaging.
- Cloud integrations.
- Remote MCP servers.
- UI polish unrelated to the slice.

Acceptance checks:

- Local static/build checks from G1 and G2 pass where toolchain exists.
- No hidden placeholder success in the vertical slice.
- `rg -n "TODO|not implemented|placeholder" src src-tauri/src` from `datrina` returns only lines linked to explicit residual tasks or post-MVP exclusions.
- README or docs no longer imply that incomplete runtime capabilities are already working.

Completion notes:

- `create_dashboard` accepts an optional `local_mvp` template that creates a persisted local dashboard with a deterministic datasource workflow and gauge widget.
- `refresh_widget` executes the widget datasource workflow, persists the run, emits `workflow:event`, and returns typed runtime widget data.
- React subscribes to `workflow:event` for visible workflow status and applies returned widget runtime data after refresh.
- Chat remains honest provider-backed behavior: without an enabled provider or `local_mock`, send returns an unavailable error; dashboard generation and tool calling remain out of MVP.
- Residual unavailable features: MCP/LLM workflow nodes, widget post-process steps, scheduler-triggered execution, generated dashboards, remote MCP, plugin SDK/marketplace, and production packaging.

Parallelism:

- Serial integration gate after W3-W7.

### W9 - Docs And Residual Closeout

Status: accepted

Depends on: W8

Owner role: docs/review agent

Ownership:

- `datrina/README.md`
- `datrina/docs/*`
- optional validation report under `datrina/docs/`

Scope:

- Align public docs with actual MVP behavior.
- Preserve this reconciliation plan as historical execution context.
- Create a concise residual backlog for deferred features.

Out of scope:

- New runtime implementation.
- New product promises.

Acceptance checks:

- README separates implemented, in-progress, and planned behavior.
- Deferred research promises are listed without implying MVP support.
- Validation commands and environment prerequisites are documented.

Completion notes:

- Public README now separates implemented MVP behavior, intentionally limited
  MVP behavior, and planned/non-MVP behavior.
- Concise deferred-feature backlog is recorded in `docs/RESIDUAL_BACKLOG.md`.
- W9 validation and closeout are recorded in `docs/W9_DOCS_CLOSEOUT.md`.

Parallelism:

- Serial closeout after W8.

### W10 - End-To-End Product Runtime

Status: implemented with residuals

Depends on: W9, plus accepted runtime baselines from W3, W4, W5, W6, W7, W8

Owner role: end-to-end integration agent

Ownership:

- `datrina/src/lib/api.ts`
- `datrina/src/App.tsx`
- `datrina/src/components/layout/*`
- `datrina/src/components/widgets/*`
- `datrina/src-tauri/src/commands/*`
- `datrina/src-tauri/src/models/*`
- `datrina/src-tauri/src/modules/ai.rs`
- `datrina/src-tauri/src/modules/mcp_manager.rs`
- `datrina/src-tauri/src/modules/scheduler.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- `datrina/src-tauri/src/modules/workflow_engine.rs`
- `datrina/src-tauri/src/modules/storage.rs`
- targeted validation/docs updates under `datrina/docs/` and `datrina/README.md`
  after behavior is implemented and verified

Scope:

- Keep W0-W9 accepted reconciliation history unchanged while promoting selected
  residuals into a validated local-first product path.
- Make first-run LLM provider setup an explicit prerequisite for LLM-backed
  behavior, with `local_mock` clearly labeled as a deterministic local mode and
  real providers configured/tested through Rust commands.
- Add provider update/re-key/disable behavior so Settings can repair a provider
  without remove-and-recreate.
- Harden real provider execution: request timeout policy, structured provider
  errors, provider-specific headers/options where required, token usage capture
  when available, and typed Tauri streaming events if streaming is exposed.
- Ground context chat in the selected dashboard, widgets, workflow runs, and
  available runtime data before calling the provider.
- Define and implement build-chat outputs that can create or update dashboards,
  widgets, workflows, and datasource bindings only through explicit apply
  commands and visible user confirmation.
- Wire agentic tool calling end to end: provider tool schema emission, tool-call
  parsing, `ToolEngine`/MCP execution behind the policy gateway, tool-result
  messages, and a bounded resume loop.
- Wire workflow `llm` nodes through the Rust-mediated AI provider runtime.
- Wire workflow MCP/tool nodes through `ToolEngine` before invoking MCP or
  built-in tools.
- Wire scheduler cron matches to the same persisted workflow execution path used
  by manual runs.
- Add dashboard/widget creation and editing UI beyond the built-in local
  template.
- Implement widget post-process steps needed by the product path with explicit
  failure semantics.
- Preserve the Tauri/Rust boundary: React owns UI state and event subscription;
  Rust owns secrets, provider calls, tool execution, workflow execution,
  scheduler, and persistence.

Out of scope:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public HTTP API unless a later external API workstream is explicitly added.
- Remote MCP transport unless a separate hardening decision and validation gate
  is added.
- Plugin SDK/marketplace.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics, OAuth/team auth, cloud sync, and mobile companion app.
- Production packaging/distribution unless split into a dedicated production
  readiness workstream.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes.
- First-run provider setup is understandable and cannot leave LLM-backed UI in
  an ambiguous state.
- A credential-free local run works with `local_mock`: create a dashboard from
  the build flow, create or edit a widget, execute the backing workflow, refresh
  the widget, persist the run, emit `workflow:event`, reload the app, and keep
  the dashboard/runtime data.
- A real-provider run works when credentials/service are available: configure
  and test a provider, send context chat grounded in dashboard data, and produce
  a provider-backed assistant response with visible provider/model metadata.
- Build chat can create or update a dashboard only through explicit generated
  changes, user confirmation, Rust command application, persistence, and reload
  verification.
- Agentic tool calls execute through `ToolEngine`/MCP policy gates or fail with
  explicit policy/runtime errors; no direct ungoverned tool execution is added.
- Workflow LLM and MCP/tool nodes execute through the same provider/tool
  gateways as chat or fail honestly with typed errors.
- Scheduler-triggered workflow execution uses the same persisted run path as
  manual execution.
- No new hidden placeholder success paths appear; sample/demo behavior is
  visibly labeled as such.
- README/docs are updated only after behavior is implemented and validated.

Completion notes:

- W10 validation is recorded in `docs/W10_END_TO_END_PRODUCT_RUNTIME.md`.
- Provider settings can add, update, re-key, enable, disable, select, and test
  providers through Rust commands.
- Provider calls use a bounded request timeout, OpenRouter-specific headers,
  structured error prefixes, token usage capture when available, and latency
  metadata.
- Context chat prepends selected dashboard/widget/workflow-run context before
  provider execution.
- Build chat exposes visible apply controls. Dashboard creation and local
  text/gauge widget addition are applied only through Rust commands and persisted
  dashboard/workflow updates.
- Workflow `llm` nodes execute through the Rust AI provider runtime. Workflow
  MCP and built-in tool nodes execute through the `ToolEngine`/MCP gateway or
  fail with explicit policy/runtime errors.
- Scheduler cron jobs registered through workflow creation execute through the
  same persisted workflow engine path used by manual runs.
- Remaining W10 residuals are provider-driven chat tool schema emission/parsing,
  bounded provider tool-call resume loops, typed streaming events, full widget
  editing forms, widget post-process execution, and live real-provider
  verification with user-provided credentials/service availability.

Parallelism:

- Start after W9 so W0-W9 remain accepted history.
- Split implementation into non-overlapping lanes: provider/settings, chat
  grounding/build apply, workflow/scheduler runtime, tool/MCP gateway, and
  dashboard/widget editing.
- Do not run concurrent agents over `src/lib/api.ts`, `src-tauri/src/models/*`,
  or command request/response shapes. Those changes must be serialized through
  the W10 integration owner.

### W11 - Opener Plugin Migration

Status: implemented

Depends on: W10

Owner role: production-readiness cleanup agent

Ownership:

- `datrina/src-tauri/Cargo.toml`
- `datrina/src-tauri/src/main.rs`
- `datrina/src-tauri/src/commands/system.rs`
- `datrina/package.json`
- `datrina/bun.lock`
- `datrina/src-tauri/gen/schemas/*`
- targeted validation/docs updates under `datrina/docs/` and `datrina/README.md`

Scope:

- Replace the deprecated `tauri_plugin_shell::Shell::open` path used by the
  Rust `open_url` command with the accepted Tauri opener plugin path.
- Keep the existing frontend/Rust command contract unchanged:
  `systemApi.openUrl` still invokes `open_url` and expects void success
  semantics.
- Remove the resolved deprecation residual from public validation notes and the
  residual backlog.
- Remove unused frontend shell plugin dependency if no frontend code imports it.

Out of scope:

- New frontend opener APIs.
- Broader production packaging or icon work.
- Public HTTP/API behavior.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes without the previous
  `tauri_plugin_shell::Shell::open` deprecation warning.

Completion notes:

- W11 validation is recorded in `docs/W11_OPENER_PLUGIN_MIGRATION.md`.
- `open_url` now uses `tauri_plugin_opener::OpenerExt`.
- `tauri-plugin-shell` was replaced with `tauri-plugin-opener` because no other
  Rust shell plugin usage remained in the codebase.
- Tauri generated schemas now expose opener permissions instead of shell
  permissions.

Parallelism:

- Serial cleanup after W10. Do not overlap with work that changes system command
  registration or Tauri plugin bootstrap.

### W12 - Provider-Driven Agentic Dashboard Builder

Status: implemented with external-provider validation residual

Depends on: W11

Owner role: agentic product completion agent

Ownership:

- `datrina/src/lib/api.ts`
- `datrina/src/App.tsx`
- `datrina/src/components/layout/*`
- `datrina/src/components/widgets/*`
- `datrina/src-tauri/src/commands/chat.rs`
- `datrina/src-tauri/src/commands/dashboard.rs`
- `datrina/src-tauri/src/commands/mcp.rs`
- `datrina/src-tauri/src/commands/provider.rs`
- `datrina/src-tauri/src/commands/tool.rs`
- `datrina/src-tauri/src/commands/workflow.rs`
- `datrina/src-tauri/src/models/*`
- `datrina/src-tauri/src/modules/ai.rs`
- `datrina/src-tauri/src/modules/mcp_manager.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- `datrina/src-tauri/src/modules/workflow_engine.rs`
- targeted validation/docs updates under `datrina/docs/`, `datrina/README.md`,
  and `datrina/docs/RESIDUAL_BACKLOG.md`

Scope:

- Keep W0-W11 accepted reconciliation history unchanged while closing the
  shortest path to a real no-mock local-first product.
- Make real-provider behavior the primary W12 acceptance lane. `local_mock` may
  remain only as an explicitly labeled dev/test smoke mode, not as acceptance
  evidence for AI-backed behavior.
- Implement provider-driven chat tool calling end to end: tool schema emission,
  tool-call parsing, policy-gated `ToolEngine`/MCP execution, tool-result
  messages, persisted/audited execution state, visible errors, and a bounded
  resume loop.
- Make MCP stdio enable fail on initialize or `tools/list` timeout/error instead
  of storing a fake connected state.
- Add an explicit reconnect or autoconnect path for persisted enabled stdio MCP
  servers needed by chat or workflow execution after app restart.
- Replace fixed Build Chat apply buttons with provider-generated structured
  dashboard/widget/workflow proposals.
- Show a preview or diff for generated proposals, require explicit user
  confirmation, and apply mutations only through Rust commands.
- Persist generated dashboards, widgets, workflows, and datasource bindings;
  verify that reload preserves the applied result and that widgets refresh
  through real runtime data paths.
- Cover the product-critical widget authoring path needed by generated
  proposals: chart, table, text, gauge, and image create/edit behavior should be
  usable enough to render refreshed runtime data without sample-only product
  paths.
- Keep unsupported or post-MVP features explicit, but no key W12 product path may
  end in mock success, hidden placeholder behavior, or a generic unavailable
  state.

Out of scope:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public HTTP/REST API.
- Remote MCP transport.
- Plugin SDK/marketplace.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics.
- OAuth/team auth, cloud sync, and mobile companion app.
- Production packaging, signing, icon sets, and platform distribution checks.
- Advanced workflow queue, priority, retry, and dead-letter behavior beyond the
  bounded resume behavior needed by provider tool calls.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes.
- A real provider can be configured, updated/tested, selected, and used for W12
  acceptance without relying on `local_mock`.
- Context chat with the real provider returns a provider-backed response with
  visible provider/model metadata, latency, and token usage when returned.
- Build Chat produces a provider-generated structured proposal instead of
  hardcoded local apply actions.
- Proposal preview shows dashboard, widget, workflow, and datasource changes
  before apply.
- Proposal apply requires explicit user confirmation, runs through Rust commands,
  persists the result, refreshes runtime data, and survives app reload.
- Chat tool calling works end to end against at least one safe built-in tool or
  configured stdio MCP tool through `ToolEngine` policy gates.
- Denied tool calls return typed policy/runtime errors and are persisted as tool
  results instead of being hidden or reported as success.
- The bounded resume loop works: assistant tool call, Rust tool execution, tool
  result message, follow-up provider call, and final assistant message.
- MCP stdio enable cannot report connected status after initialize or
  `tools/list` timeout/error.
- Persisted enabled stdio MCP servers can be reconnected explicitly or
  automatically for chat/workflow execution after app restart.
- W12-created widgets can be created, edited, persisted, reloaded, refreshed, and
  rendered without sample-only data in the product path.
- `local_mock` remains clearly labeled as local deterministic dev/test behavior
  wherever exposed.
- `rg -n "TODO|not implemented|placeholder|unsupported|local_mock" src src-tauri/src`
  returns only dev/test labels, explicit post-MVP exclusions, or residuals
  outside the W12 product path.
- README/docs/residual backlog are updated after validation and no longer imply
  mock-backed or unsupported core functions are complete.

Completion notes:

- W12 validation is recorded in `docs/W12_PROVIDER_DRIVEN_AGENTIC_DASHBOARD_BUILDER.md`.
- Build Chat now asks the active provider for a strict structured proposal,
  stores the parsed proposal in assistant message metadata, previews proposed
  dashboard/widget changes in React, and applies them only after explicit user
  confirmation through the Rust `apply_build_proposal` command.
- Proposal apply can create or append chart, table, text, gauge, and image
  widgets with persisted datasource workflows. Refresh uses the existing Rust
  workflow runtime rather than sample-only React data.
- Chat provider calls now support OpenAI-compatible tool schema emission,
  tool-call parsing, safe built-in `http_request` execution through
  `ToolEngine`, persisted visible tool results/errors, and one bounded provider
  resume call for the final assistant response.
- MCP stdio connect now fails on initialize and `tools/list` timeout/error
  instead of reporting a connected empty-tool state. Enabled persisted stdio
  servers can be reconnected through `reconnect_enabled_servers` and are
  auto-connected before MCP tool listing or calls.
- `local_mock` remains available only as clearly labeled local deterministic
  dev/test smoke behavior. Live real-provider acceptance still requires
  user-provided credentials/service availability in this checkout.

Parallelism:

- Start after W11.
- Split implementation into non-overlapping lanes only when command/model/API
  shape changes are serialized through the W12 integration owner.
- Do not run concurrent agents over `src/lib/api.ts`, `src-tauri/src/models/*`,
  or command request/response shapes.
- Tool/MCP hardening, provider/tool-call runtime, Build Chat proposal/apply UI,
  and widget authoring can be investigated in parallel but must integrate through
  one final W12 acceptance pass.

### W13 - Durable Real Runtime Pipeline

Status: implemented with external-provider validation residual

Depends on: W12

Owner role: durable runtime integration agent

Ownership:

- `datrina/src/lib/api.ts`
- `datrina/src/App.tsx`
- `datrina/src/components/layout/*`
- `datrina/src/components/widgets/*`
- `datrina/src-tauri/src/commands/chat.rs`
- `datrina/src-tauri/src/commands/dashboard.rs`
- `datrina/src-tauri/src/commands/mcp.rs`
- `datrina/src-tauri/src/commands/provider.rs`
- `datrina/src-tauri/src/commands/tool.rs`
- `datrina/src-tauri/src/commands/workflow.rs`
- `datrina/src-tauri/src/models/*`
- `datrina/src-tauri/src/modules/ai.rs`
- `datrina/src-tauri/src/modules/mcp_manager.rs`
- `datrina/src-tauri/src/modules/scheduler.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- `datrina/src-tauri/src/modules/workflow_engine.rs`
- targeted validation/docs updates under `datrina/docs/`,
  `datrina/README.md`, and `datrina/docs/RESIDUAL_BACKLOG.md`

Scope:

- Keep W0-W12 accepted reconciliation history unchanged while replacing the
  remaining static or only locally simulated product paths with one durable live
  runtime pipeline.
- Make real-provider acceptance the primary W13 gate. `local_mock` may remain as
  a dev/test smoke provider, but it is not acceptance evidence for W13 product
  behavior.
- Define the first supported live dashboard datasource plan shape generated by
  Build Chat. A generated widget must be backed by an executable workflow plan
  through `ToolEngine`, stdio MCP, or Rust-mediated provider calls, not only by a
  literal persisted `data` value.
- Extend Build Chat proposal validation and apply so provider-generated
  datasource/workflow plans are previewed, confirmed, persisted, refreshed, and
  reloaded through Rust commands.
- Expose available safe built-in tools and connected or reconnectable stdio MCP
  tools to provider chat/tool calling through the existing policy gateway.
- Execute provider-requested MCP tool calls through `ToolEngine`/`MCPManager`
  with persisted visible tool results or explicit policy/runtime errors.
- Make workflow MCP nodes use the same reconnect/autoconnect behavior as MCP
  commands before calling a persisted enabled stdio server.
- Make scheduled workflow execution durable: start the cron scheduler runner,
  reload persisted cron workflows at app startup, update or unschedule jobs when
  workflows change or are deleted, and execute through the same persisted runner
  path as manual workflow/widget refresh.
- Surface scheduled run state and widget refresh results in the UI enough for an
  operator to see whether live refresh succeeded, failed by policy, or failed by
  provider/MCP runtime.
- Validate one complete live loop end to end: configure a real provider, create a
  provider-generated dashboard with live datasource workflow, apply after
  confirmation, execute tool/MCP/provider-backed refresh, persist the run, emit
  `workflow:event`, reload the app, and verify the dashboard/runtime data still
  works without `local_mock`.

Out of scope:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public HTTP/REST API.
- Remote MCP transport unless a separate hardening decision is added first.
- Plugin SDK/marketplace.
- Arbitrary sandboxed JavaScript.
- DuckDB analytics.
- OAuth/team auth, cloud sync, and mobile companion app.
- Production packaging, signing, icon sets, and platform distribution checks
  unless needed only as validation notes.
- Full visual workflow editor.
- Arbitrary unbounded multi-step agents. W13 may extend beyond the W12
  one-resume loop only when the limit, UI state, and policy boundary are explicit.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes.
- Real-provider setup is validated live with user-provided credentials or a
  reachable local real provider. The W13 acceptance record must name the provider
  kind, model, and exact success/failure outcome.
- Build Chat produces a provider-generated proposal whose widgets are backed by
  executable datasource/workflow plans, not only literal static `data`.
- Proposal preview shows dashboard, widget, workflow, datasource, and tool/MCP
  effects before apply.
- Proposal apply requires explicit user confirmation, runs only through Rust
  commands, persists all dashboard/widget/workflow/datasource state, refreshes
  runtime data, and survives app reload.
- Chat tool calling advertises at least one safe built-in tool and at least one
  configured stdio MCP tool when available, with policy-gated execution and
  persisted visible tool results/errors.
- Workflow MCP nodes reconnect or autoconnect persisted enabled stdio servers
  before execution and fail with typed errors when reconnect is impossible.
- Scheduler cron jobs are actually running, are restored after app restart, are
  removed when workflows are deleted, and execute through the same persisted
  workflow runner as manual refresh.
- A scheduled live workflow run updates persisted run state and emits
  `workflow:event`; React surfaces the outcome without requiring the operator to
  inspect storage directly.
- `local_mock` remains clearly labeled as local deterministic dev/test behavior
  and is not used as W13 acceptance evidence.
- `rg -n "TODO|not implemented|placeholder|unsupported|local_mock|unavailable" src src-tauri/src`
  returns only dev/test labels, explicit post-MVP exclusions, or residuals
  outside the W13 product path.
- README/docs/residual backlog are updated only after behavior is implemented and
  validated, and they do not imply that mock-backed or unsupported core functions
  are complete.

Validation record:

- W13 validation must be recorded in
  `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`.

Completion notes:

- W13 validation is recorded in
  `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`.
- Build Chat proposals now require an executable `datasource_plan` per widget.
  Apply converts those plans into persisted workflow nodes backed by
  `ToolEngine` built-ins, stdio MCP tools, or Rust-mediated provider prompts;
  literal `data` is only a preview sample.
- Chat tool calling exposes the safe built-in `http_request` tool and, when
  reconnectable enabled stdio MCP servers exist, a policy-gated generic MCP tool
  call with visible persisted results/errors.
- Manual widget refresh, workflow commands, and scheduled jobs reconnect enabled
  persisted stdio MCP servers before workflow MCP nodes run.
- The cron scheduler now starts its runner, restores enabled persisted cron
  workflows at app startup, replaces existing jobs when workflow commands
  recreate a cron workflow, unschedules jobs on delete, and emits the same
  `workflow:event` stream as manual refresh.
- React surfaces per-widget workflow run status from `workflow:event` so
  operators can see idle/running/success/error outcomes in the dashboard shell.
- Live external-provider acceptance still requires user-provided credentials or
  a reachable local real provider in this checkout; `local_mock` remains
  dev/test-only and is not W13 acceptance evidence.

Parallelism:

- Start after W12.
- Split investigation into non-overlapping lanes: scheduler durability,
  workflow MCP reconnect, chat/MCP tool exposure, live datasource proposal/apply,
  and UI runtime state.
- Do not run concurrent agents over `src/lib/api.ts`, `src-tauri/src/models/*`,
  or command request/response shapes.
- All lanes must integrate through one final W13 acceptance pass against the same
  live provider and one durable dashboard/runtime scenario.

### W14 - Chat Streaming, Reasoning Trace, And Tool Visibility

Status: implemented with external-provider validation residual

Depends on: W13

Owner role: chat runtime and observability agent

Ownership:

- `datrina/src/lib/api.ts`
- `datrina/src/App.tsx`
- `datrina/src/components/layout/ChatPanel.tsx`
- `datrina/src/components/layout/*` only if shared shell state is required
- `datrina/src-tauri/src/commands/chat.rs`
- `datrina/src-tauri/src/models/chat.rs`
- `datrina/src-tauri/src/modules/ai.rs`
- `datrina/src-tauri/src/modules/mcp_manager.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- `datrina/src-tauri/src/modules/workflow_engine.rs` only if workflow/chat
  event envelope reuse is required
- targeted validation/docs updates under `datrina/docs/`,
  `datrina/README.md`, and `datrina/docs/RESIDUAL_BACKLOG.md`

Scope:

- Audit the current chat request/response path before implementation:
  `ChatPanel -> chatApi.sendMessage -> send_message -> AIEngine -> provider
  response -> persisted ChatMessage`.
- Define one typed chat streaming event envelope over Tauri events. At minimum it
  must represent: message started, assistant content delta, provider reasoning
  summary delta or snapshot when the provider exposes one, tool call requested,
  tool execution started, tool result/error, build proposal parsed, message
  completed, and message failed.
- Implement streaming for providers that support streamed OpenAI-compatible chat
  completions. Non-streaming providers such as `local_mock` may use the same
  event envelope with synthetic single-step events, but the UI must not pretend
  they are live provider streams.
- Preserve Rust ownership of provider calls, secrets, tool execution, MCP
  lifecycle, and persistence. React may render stream state but must not call
  providers or tools directly.
- Display provider-supplied reasoning only as an explicit model/provider output
  field such as reasoning summary, reasoning text, annotation, or equivalent
  public trace. Do not request, fabricate, persist, or display hidden chain of
  thought.
- Show tool activity as first-class chat state: requested tool name, arguments
  preview with secret masking, policy decision, running/success/error status,
  result preview, and final assistant resume.
- Keep the bounded tool loop explicit. If W14 expands beyond the W12/W13
  one-resume loop, define the max iterations, cancellation behavior, timeout,
  and UI state before changing runtime behavior.
- Add cancellation/abort behavior for an in-flight streamed chat response if the
  current provider/runtime path can support it safely. If not, record the exact
  limitation and make the UI state honest.
- Persist the final assistant message, tool calls, tool results, provider/model
  metadata, token usage when available, and visible reasoning summaries when
  available. Do not persist partial stream noise unless a deliberate resume
  contract is added.
- Keep build proposal parsing/apply semantics unchanged: streamed text may
  preview progress, but dashboard changes still require a parsed proposal and
  explicit user confirmation.

Out of scope:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public HTTP/REST or SSE server APIs.
- React-owned direct LLM, MCP, or tool calls.
- Remote MCP transport unless a separate hardening workstream accepts it first.
- Arbitrary unbounded autonomous agents.
- Exposing hidden chain-of-thought or prompting providers to reveal private
  reasoning.
- Replacing the existing workflow event system unless the new chat event
  envelope can reuse it with a small typed extension.
- Production packaging, signing, and distribution.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes.
- The chat event envelope is typed on the Rust side and mirrored in TypeScript.
- A real streaming-capable provider run shows assistant text incrementally in
  React while the Rust command remains the only provider caller.
- Tool calls are visible while running and after completion, including policy
  denials and MCP/runtime failures.
- Provider-supplied visible reasoning summaries/traces, when present, are
  rendered in a clearly separated UI region. Hidden chain-of-thought is neither
  requested nor displayed.
- Build Chat can stream a provider-generated proposal and still requires
  explicit confirmation before applying dashboard changes.
- Non-streaming provider behavior remains honest and does not show fake token
  streaming.
- A failed provider stream leaves the chat session in a recoverable state with a
  visible error and no fake assistant success message.
- Secrets in provider config, MCP args/env, tool arguments, headers, and results
  are masked before display.
- README/docs/residual backlog are updated after validation so streaming,
  reasoning trace, and tool visibility claims match implemented behavior.

Validation record:

- W14 validation must be recorded in
  `docs/W14_CHAT_STREAMING_TRACE_UI.md`.

Completion notes:

- W14 validation is recorded in
  `docs/W14_CHAT_STREAMING_TRACE_UI.md`.
- Chat now has a typed Rust `chat:event` envelope mirrored in TypeScript for
  message start, assistant content delta, visible reasoning delta/snapshot,
  tool call requested, tool execution started, tool result/error, build proposal
  parsed, message completed, and message failed.
- OpenAI-compatible providers use Rust-owned SSE streaming through
  `AIEngine`; `local_mock` and Ollama use honest synthetic single-step events.
- React renders incremental assistant content, separated visible reasoning,
  masked live tool activity, parsed proposal previews, completion, failure, and
  cancellation state from Tauri events.
- The W12/W13 bounded one-resume tool loop and explicit Build Chat apply
  confirmation boundary are unchanged.
- Live real streaming-provider and tool-calling Build Chat acceptance still
  requires user-provided credentials or a reachable local real provider in this
  checkout; `local_mock` remains dev/test-only evidence.

Parallelism:

- Start after W13.
- Split only into non-overlapping lanes: provider streaming parser/runtime,
  chat event contract, React stream rendering, and tool trace rendering.
- Serialize all edits to `src/lib/api.ts`, `src-tauri/src/models/chat.rs`, and
  command request/response/event shapes through one integration owner.
- All lanes must integrate through one final W14 acceptance pass against one
  streaming-capable provider and one tool-calling Build Chat scenario.

### W15 - Chat Runtime Replacement And Message Parts Model

Status: implemented with external-provider validation residual

Depends on: W14

Owner role: chat runtime architecture and frontend integration agent

Ownership:

- `datrina/package.json`
- `datrina/bun.lockb` or the active package lock if dependency changes are made
- `datrina/src/lib/api.ts`
- `datrina/src/App.tsx`
- `datrina/src/components/layout/ChatPanel.tsx`
- new chat runtime/components under `datrina/src/components/chat/` or
  `datrina/src/lib/chat/` if introduced
- `datrina/src-tauri/src/commands/chat.rs`
- `datrina/src-tauri/src/models/chat.rs`
- `datrina/src-tauri/src/modules/ai.rs`
- `datrina/src-tauri/src/modules/tool_engine.rs`
- targeted validation/docs updates under `datrina/docs/`,
  `datrina/README.md`, and `datrina/docs/RESIDUAL_BACKLOG.md`

Scope:

- Replace the current custom chat render/state assembly with a message-parts
  model that treats assistant text, visible reasoning, provider-opaque reasoning
  state, tool calls, tool results, errors, and Build Chat proposals as typed
  parts of one assistant run instead of scattered `messages`, metadata, and
  `streamTraces` state.
- Evaluate and adopt a production chat UI/runtime layer such as `assistant-ui`
  with a custom Tauri/Rust adapter, or document why a local minimal runtime is
  required. The chosen layer must support typed message parts, reasoning display,
  tool-call lifecycle rendering, cancellation/error states, and custom proposal
  parts.
- Map the existing Rust-owned `chat:event` stream to a canonical agent event
  protocol shape inspired by AG-UI or AI SDK UI streams: run start/finish/error,
  text start/delta/end, reasoning start/delta/end, tool call start/args/end,
  tool result, custom Build Chat proposal, abort/cancel, and recoverable
  failure.
- Preserve the accepted runtime boundary: React renders and submits user input;
  Rust remains the only owner of provider calls, provider secrets, MCP process
  lifecycle, tool execution, policy decisions, persistence, and proposal apply.
- Store provider-visible reasoning summaries separately from provider-opaque
  reasoning continuation data. Opaque reasoning blobs or provider-specific
  reasoning details may be persisted/round-tripped only as backend-owned state
  and must not be displayed as hidden chain-of-thought.
- Keep tool activity first-class in the UI: requested tool name, argument
  preview with masking, policy decision, running/success/error state, result
  preview, and assistant resume after the bounded tool loop.
- Keep Build Chat apply semantics unchanged. Generated dashboard/workflow
  changes must remain preview-only until explicit user confirmation, even if the
  proposal is carried as a typed message part.
- Remove or quarantine obsolete chat state helpers once the replacement runtime
  owns message-part assembly. Do not leave two active chat state machines.
- Update README/docs/residual backlog after implementation so chat, reasoning,
  tool visibility, and external-provider validation claims match the new
  runtime.

Out of scope:

- Node/Hono/Turborepo runtime inside `datrina`.
- Public HTTP/REST or SSE server APIs.
- React-owned direct LLM, MCP, or tool calls.
- Remote MCP transport unless a separate hardening workstream accepts it first.
- Unbounded autonomous tool loops or new agent orchestration semantics.
- Exposing hidden chain-of-thought or prompting providers to reveal private
  reasoning.
- Rewriting dashboard, workflow, scheduler, provider settings, or MCP manager
  behavior except where required to preserve existing chat contracts.
- Production packaging, signing, and distribution.

Acceptance checks:

- Tauri config JSON parse succeeds.
- `bun run check:contract` passes.
- `bun run typecheck` passes.
- `bun run build` passes.
- `cargo fmt --all --check` passes.
- `cargo check --workspace --all-targets` passes.
- The chosen chat runtime/dependency decision is documented in the W15
  validation record with the reason it fits Tauri/Rust ownership.
- `ChatPanel` no longer manually assembles independent reasoning/tool trace
  state from every event kind; rendering is driven by typed message parts or a
  runtime adapter with equivalent typed state.
- The Rust `ChatMessage`/event model can represent text, visible reasoning,
  provider-opaque reasoning state, tool calls, tool results, Build Chat
  proposals, cancellation, and failures without lossy metadata-only fields.
- A local no-key path such as `local_mock` still works honestly without fake live
  token streaming.
- A real streaming-capable provider run, when credentials/service are available,
  renders incremental assistant text, visible provider-supplied reasoning when
  present, tool lifecycle, final assistant resume, and recoverable errors.
- Tool arguments/results/secrets remain masked before display.
- Build Chat proposal preview and explicit apply confirmation still work after
  the runtime replacement.
- No React code path calls providers, MCP, or tools directly.

Validation record:

- W15 validation must be recorded in
  `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`.

Completion notes:

- W15 validation is recorded in
  `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`.
- The chat UI now uses a local typed message-parts runtime adapter under
  `src/lib/chat/runtime.ts`; `ChatPanel` renders parts and no longer owns a
  second `messages`/metadata/`streamTraces` state machine.
- The Rust `ChatMessage` model now carries typed parts for assistant text,
  visible reasoning, backend-owned opaque reasoning state references, tool
  calls, tool results, Build Chat proposals, recoverable errors, and
  cancellation.
- `chat:event` now carries a canonical `agent_event` protocol shape for run,
  text, reasoning, tool, proposal, cancellation, and failure events while
  preserving the W14 event names for compatibility.
- No new React-side provider, MCP, or tool runtime was added; Rust remains the
  owner of provider calls, secrets, MCP lifecycle, tool policy/execution,
  persistence, and proposal apply.
- Live real streaming-provider and tool-calling Build Chat acceptance still
  requires user-provided credentials or a reachable local real provider in this
  checkout; `local_mock` remains dev/test-only evidence.

Parallelism:

- Start after W14.
- Split only into non-overlapping lanes: chat runtime/dependency evaluation,
  Rust message-part/event model, frontend runtime adapter/components, and final
  validation/docs.
- Serialize all edits to `src/lib/api.ts`, `src-tauri/src/models/chat.rs`, and
  command request/response/event shapes through one integration owner.
- All lanes must integrate through one final W15 acceptance pass against one
  no-key path and, when available, one streaming-capable provider with a
  tool-calling Build Chat scenario.

## Dedicated Product Workstreams After W15

The product-readiness backlog after W15 is tracked as one dedicated
`docs/W<N>_*.md` file per workstream. Agents executing these streams must use
the matching file as the task contract and preserve the same
`Status / Context / Goal / Approach / Files / Validation / Out of scope /
Related` shape for new tasks.

Current dedicated workstream index:

- `W16` - Proposal Validation Gate:
  `docs/W16_PROPOSAL_VALIDATION_GATE.md`.
- `W17` - Agent Memory And Local RAG:
  `docs/W17_AGENT_MEMORY_RAG.md`.
- `W18` - Plan / Execute / Reflect Agent Loop:
  `docs/W18_PLAN_EXECUTE_REFLECT.md`.
- `W19` - Dashboard Versions And Undo:
  `docs/W19_DASHBOARD_VERSIONS_UNDO.md`.
- `W20` - Data Playground And Templates:
  `docs/W20_DATA_PLAYGROUND_TEMPLATES.md`.
- `W21` - Alerts And Autonomous Triggers:
  `docs/W21_ALERTS_AUTONOMOUS_TRIGGERS.md`.
- `W22` - Token And Cost Tracking:
  `docs/W22_TOKEN_COST_TRACKING.md`.
- `W23` - Pipeline Debug View:
  `docs/W23_PIPELINE_DEBUG_VIEW.md`.
- `W24` - Agent Eval Suite:
  `docs/W24_AGENT_EVAL_SUITE.md`.
- `W25` - Dashboard Parameters:
  `docs/W25_DASHBOARD_PARAMETERS.md`.
- `W26` - CAO Autopilot E2E:
  `docs/W26_CAO_AUTOPILOT_E2E.md`.
- `W27` - Cyberpunk UI Redesign:
  `docs/W27_CYBERPUNK_REDESIGN.md`.
- `W28` - Chat UX Hardening And Regression Pass:
  `docs/W28_CHAT_UX_HARDENING.md`.
- `W29` - Real Provider Runtime And Honest Failure Gate:
  `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`.
- `W30` - Datasource And Pipeline Workbench:
  `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`.
- `W31` - Datasource Identity, Binding, And Provenance:
  `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`.
- `W32` - Typed Pipeline Studio:
  `docs/W32_TYPED_PIPELINE_STUDIO.md`.
- `W33` - Real Provider Acceptance And Agent Eval V2:
  `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`.
- `W34` - Parameterized Datasource Options:
  `docs/W34_PARAMETERIZED_DATASOURCE_OPTIONS.md`.
- `W35` - Workflow Operations Cockpit (shipped — Operations route, run
  history with filters, retry, honest "cancel unsupported" outcome,
  scheduler health warnings):
  `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`.
- `W36` - Widget Runtime Snapshots:
  `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`.
- `W37` - External Open Source And Free-Use Source Catalog:
  `docs/W37_EXTERNAL_OPEN_SOURCE_CATALOG.md`.
- `W38` - Build Chat Widget Mentions (shipped — typed `WidgetMention`
  bundle on `SendMessageRequest`, `@`-picker in Build composer scoped to
  the active dashboard, targeted-edit system prompt block, validator
  `OffTargetWidgetReplace` / `OffTargetWidgetRemove` gates, mention chips
  persisted as a `widget_mentions` part on the user message):
  `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`.
- `W39` - Automatic Datasource Materialization (shipped — Build apply
  auto-creates / reuses `DatasourceDefinition` rows via canonical
  signature, `http_request` validated as a first-class kind,
  `preview_proposal_materialization` exposes the per-source plan in the
  proposal preview; compose/output_path/inputs still fall back to
  per-widget workflows): `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`.
- `W40` - Widget Runtime Performance (shipped — batched
  `refresh_dashboard_widgets` command dedupes by `workflow_id`, runs
  independent workflows with a bounded concurrency cap, frontend
  supersede tokens drop stale results, memoised `WidgetCell` keeps
  unrelated widgets stable across refresh ticks):
  `docs/W40_WIDGET_RUNTIME_PERFORMANCE.md`.
- `W41` - Widget Execution Observability And LLM Provenance (shipped —
  `get_widget_provenance` command, typed `WidgetProvenance` envelope with
  redacted source/argument previews, `llm_participation` enum, inspector
  panel with Workbench/Operations/Pipeline Debug deep links, and
  reflection-prompt provenance feed). Details in
  `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`.
- `W42` - Text Widget Streaming And Reasoning State (shipped v1 —
  `widget:stream` Tauri channel with typed `WidgetStreamEnvelope`,
  per-refresh `WidgetStreamContext` + supersede registry on
  `AppState`, streaming-aware tail-pipeline runner that taps the
  terminal `llm_postprocess` text step, and a TextWidget that paints
  partial provider output behind a "streaming"/"reasoning" badge while
  the committed runtime value remains the source of truth):
  `docs/W42_WIDGET_STREAMING_REASONING.md`.
- `W43` - Dashboard And Widget Model Selection:
  `docs/W43_DASHBOARD_WIDGET_MODEL_SELECTION.md`.
- `W44` - Image Fullscreen And Gallery Widget:
  `docs/W44_IMAGE_FULLSCREEN_GALLERY_WIDGET.md`.
- `W45` - Layout System Simplification And Build Layout Playbook:
  `docs/W45_LAYOUT_SYSTEM_SIMPLIFICATION.md`.
- `W46` - Dashboard Header Resilience:
  `docs/W46_DASHBOARD_HEADER_RESILIENCE.md`.
- `W47` - LLM Conversation Language Settings:
  `docs/W47_LLM_CONVERSATION_LANGUAGE_SETTINGS.md`.
- `W48` - Build Chat Source Mentions And Multi-Source Composition:
  `docs/W48_BUILD_CHAT_SOURCE_MENTIONS_AND_MULTI_SOURCE_COMPOSITION.md`.
- `W49` - Chat Context Economy And Cost Accounting Repair:
  `docs/W49_CHAT_CONTEXT_ECONOMY_AND_COST_ACCOUNTING_REPAIR.md`.
- `W50` - Dashboard Refresh Pause And Schedule Controls:
  `docs/W50_DASHBOARD_REFRESH_PAUSE_AND_SCHEDULE_CONTROLS.md`.
- `W51` - Provider Context Compression And Raw Artifact Retention:
  `docs/W51_PROVIDER_CONTEXT_COMPRESSION_AND_RAW_ARTIFACT_RETENTION.md`.

## Parallelization Model

Recommended agent queue:

1. Run W0, W1, W2 serially.
2. Run W3 and W4 in parallel after W2.
3. Run W5 and W6 in parallel only if W6 does not wire tool calling yet.
4. Run W7 after W5 interfaces are stable.
5. Run W8 as one integration task.
6. Run W9 as final docs/review.
7. Run W10 as the product-completion stream after W9, promoting selected
   residuals into real end-to-end runtime behavior.
8. Run W11 as a narrow production-readiness cleanup for the opener plugin
   migration.
9. Run W12 as the no-mock agentic product completion stream: provider-driven
   tool calls, generated dashboard proposals, persisted apply, MCP runtime
   honesty, and usable widget authoring.
10. Run W13 as the durable live runtime stream: real-provider acceptance,
    executable datasource/workflow plans, MCP tool exposure, workflow reconnect,
    scheduler durability, reload verification, and one no-mock product loop from
    AI proposal to live widget refresh.
11. Run W14 as the chat observability stream: typed Tauri streaming events,
    visible provider-supplied reasoning summaries, live tool-call status/results,
    cancellation/failure honesty, and Build Chat proposal streaming without
    bypassing explicit apply confirmation.
12. Run W15 as the chat runtime replacement stream: replace the custom chat
    state machine with typed message parts and a production UI/runtime adapter
    while preserving the Rust-owned provider/tool/MCP boundary.
13. Run W16-W26 from their dedicated `docs/W<N>_*.md` records when product
    readiness work resumes after W15.
14. Run W27 as a bounded frontend-only redesign stream after W26: refresh the
    app's visual language into a cyberpunk operations console while preserving
    Rust/API contracts, chat runtime semantics, dashboard runtime behavior, and
    local-first validation boundaries.
15. Run W28 as a chat UX lifecycle hardening pass after W27: make Build Chat
    entrypoints deterministic, keep seeded prompts out of stale sessions, clean
    up copy/edit/regenerate affordances, add visible cancellation/error/retry
    states, preserve drafts/scroll behavior, and only cross into backend/API
    scope for explicitly accepted durable lifecycle fixes.
16. Run W29 as the no-fake-success runtime gate after W28: remove LocalMock from
    product provider paths, migrate existing local mock rows to visible
    unsupported state, make no-provider chat fail closed with typed remediation,
    block apply of validation-failed Build proposals, and prove one real-provider
    Context/Build acceptance lane when credentials or a reachable local provider
    are available.
17. Run W30 as the operations-grade datasource/pipeline workbench after W29:
    promote existing datasource plans, shared fan-out, Playground samples,
    parameters, traces, and pipeline DSL into first-class reusable datasource
    definitions with inspect/edit/test/save UI, while keeping the existing Rust
    workflow engine as the only execution engine. **2026-05-17 status:** v1 of
    the Workbench shipped — saved `DatasourceDefinition` model + SQLite tables,
    10 Tauri commands (CRUD/test-run/list-consumers/import/export), TypeScript
    bindings, `#/workbench` panel reachable from the sidebar, and Build Chat
    catalog injection so the agent reuses saved sources. Contract checker also
    grew a generic `PipelineStep` / `ValidationIssue` parity gate. Residuals:
    typed pipeline step editor, apply-time signature reuse (promote a matching
    `shared_datasources` entry into a binding on the saved definition rather
    than minting a fresh workflow), per-widget tail pipelines through the
    Workbench, W23 edit-test-save loop, and Playground "Save as datasource".
    See `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`.
18. Run W31 after W30 closeout as the datasource identity stream: make saved
    datasource definitions explicit widget/proposal/version/export identities,
    bind Build Chat proposals to existing definitions instead of duplicating
    hidden workflows, and surface provenance plus impact preview in the
    Workbench.
19. Run W32 after W31 as the typed pipeline studio stream: replace default JSON
    pipeline editing with typed step forms, replay traces/samples before save,
    and keep JSON as an advanced validated escape hatch.
    **2026-05-17 status:** v1 shipped — `replay_pipeline` Tauri command
    (deterministic; refuses `llm_postprocess` / `mcp_call`), TS step
    registry at `src/lib/pipeline/registry.ts`, Studio components at
    `src/components/pipeline/{PipelineStudio,StepEditor}.tsx`,
    Workbench's JSON textarea replaced with the Studio (seeded from
    last test-run's raw source), PipelineDebugModal grew an
    "Open in Studio" mode that replays via `from_widget_trace` (or
    inline sample fallback), and W18 reflection now embeds
    `trace_summary_for_reflection` so the agent can anchor fix-ups on
    a concrete step. Residuals: save-from-Debug, step-level proposal
    diffs from reflection/Build Chat, drag-and-drop reorder.
    See `docs/W32_TYPED_PIPELINE_STUDIO.md`.
20. Run W33 after W31 or in parallel with W32 only if command/model ownership is
    serialized: extract the test-only provider abstraction, add recorded/full
    loop evals, add the live-provider acceptance lane, and centralize strict
    structured-output capability handling.
    Shipped 2026-05-17: `AIProvider` trait + `RecordedProvider`,
    `replay_loop_passes` assertion, `StructuredOutputCapability`
    capability map, `bun run acceptance` runner, and
    `--features expensive_evals` live lane behind explicit env vars.
    See `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`.
21. Run W34 after W31 and preferably after W32: make query-backed dashboard
    parameters resolve real MCP/HTTP/datasource option lists, support dependent
    controls, and sync same-profile parameter state through the URL hash.
22. Run W35 after W31: add the workflow operations cockpit over existing
    scheduler/workflow runs so operators can inspect health, errors, schedules,
    retries, and owner links without adding a second workflow engine.
23. Run W36 after W31 and preferably near W35: persist last successful rendered
    widget runtime snapshots so dashboards can paint immediately after app
    restart, mark cached data as stale, then replace it through the normal live
    refresh path without treating cache as datasource health evidence.
24. Run W37 after W31 and preferably after W35: add the researched MCP/source
    shortlist from `docs/W37_EXTERNAL_OPEN_SOURCE_CATALOG.md` as a curated
    catalog of license/terms-reviewed open-source or free-use external sources:
    Brave Search, Fetch, CoinGecko, DefiLlama, GitHub, arXiv, MediaWiki,
    RSS/Atom, Hacker News, and SearXNG candidates. Users can enable/disable
    sources in UX, save them as datasources, and expose enabled sources as
    dashboard-scoped chat tools so chat can analyze dashboard data and call
    external sources for clarification without bypassing validation, provenance,
    or explicit apply confirmation.
25. Run W38 after W31 and preferably after W32/W36: add Build Chat widget
    mentions for existing dashboards. Users can mention one or more current
    widgets from the composer, send stable `widget_id` targets with compact
    typed widget context, and have Build proposals update/replace/remove only
    those mentioned widgets unless broader edits were explicitly requested.
    Proposal validation, preview, and explicit apply confirmation remain the
    enforcement boundary. See `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`.
26. Run W39 after W31 and preferably after W32/W37: remove the manual
    datasource setup requirement from the normal Build/Playground happy path.
    Build Chat and Playground-created HTTP/MCP/provider sources should
    automatically materialize into saved `DatasourceDefinition` objects on
    confirmed apply/save, reuse existing matching definitions by canonical
    signature, bind widgets through `datasource_definition_id`, and keep
    Workbench as the inspect/edit/debug surface rather than a required setup
    step. Explicit preview/apply confirmation, validation, provenance,
    Rust-owned secrets, and W29 no-fake-success behavior remain mandatory. See
    `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`.
27. Run W40 after W36 and preferably after W39: make widgets feel immediate by
    profiling refresh/render latency, eliminating redundant datasource and
    pipeline work, adding bounded concurrency/cache rules, and proving the
    dashboard paints quickly without fake/stale success. See
    `docs/W40_WIDGET_RUNTIME_PERFORMANCE.md`.
28. Run W41 after W31/W35 and preferably after W23/W39: expose each widget's
    execution provenance in details, including datasource/workflow identity,
    tool/provider calls, arguments with secret redaction, timings, latest run
    status, and whether an LLM/provider step participates in the widget. See
    `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`.
29. Run W42 after W41 and after the chat streaming substrate from W14/W15:
    support streaming updates for text-like widget refreshes, surface visible
    "LLM reasoning in progress" state when a provider step is active, and keep
    partial output honest until the final runtime value is committed.
    **2026-05-17 status:** v1 shipped — new `widget:stream` Tauri event
    channel; typed `WidgetStreamEnvelope` with `refresh_run_id` +
    `sequence`; per-widget `WidgetStreamContext` on `AppState`
    (`widget_refresh_runs`) drops emissions from superseded refreshes
    server-side and the React reducer drops stale deltas client-side;
    `run_pipeline_with_streaming` taps the terminal `llm_postprocess Text`
    step on Text widgets and fan-outs provider reasoning/text deltas;
    `TextWidget` paints partial output (animated caret) and a
    "thinking…" placeholder while reasoning is the only signal so far,
    with a destructive ring for partial-on-failure. Residuals: streaming
    for chart/table data, persisting reasoning summaries, applying
    streamed proposals without confirmation.
    See `docs/W42_WIDGET_STREAMING_REASONING.md`.
30. Run W43 after W29/W33 and before broad widget streaming rollout where
    possible: let users choose default LLM/model policy at dashboard scope and
    override it per eligible widget, while preserving provider capability,
    cost, no-fake-success, and Rust-owned secret boundaries. See
    `docs/W43_DASHBOARD_WIDGET_MODEL_SELECTION.md`.
31. Run W44 after the widget runtime data contracts are stable: add a
    fullscreen image viewer and a first-class gallery widget with datasource
    pipeline-backed image items, keyboard navigation, loading/error states, and
    no hardcoded demo images. Shipped v1: `Widget::Gallery` +
    `BuildWidgetType::Gallery`, runtime coercion accepting string/object/
    envelope shapes with broken-image fallback, `HardcodedGalleryItems`
    validation gate, reusable `ImageLightbox` (focus trap, keyboard nav,
    cycle fit), Build Chat prompt update + Wikipedia gallery template.
    See `docs/W44_IMAGE_FULLSCREEN_GALLERY_WIDGET.md`.
32. Run W45 after W38/W39 and preferably before more Build Chat layout features:
    simplify dashboard layout semantics, keep row-first 12-column auto-pack as
    the default, and teach the LLM a small set of reusable layout patterns
    instead of letting it generate arbitrary random placements. Shipped v1:
    typed `SizePreset` (kpi/half_width/wide_chart/full_width/table/text_panel/
    gallery) and `LayoutPattern` on `BuildWidgetProposal`; apply path drops
    explicit `x`/`y` for new widgets and resolves presets to (w, h); validator
    gates `ProposedExplicitCoordinates` and `ConflictingLayoutFields`; Build
    system prompt now documents the playbook; eval fixtures cover both happy
    path and rejection. See `docs/W45_LAYOUT_SYSTEM_SIMPLIFICATION.md`.
33. Run W46 as a narrow frontend hardening pass after W27 and before another
    visual redesign: fix dashboard header layout so titles, actions, provider
    status, parameters, and responsive controls wrap or collapse predictably
    instead of clipping important text. Shipped v1: title area owns flex
    space with truncate + native tooltip; provider chip collapses model
    suffix below `xl` and name max-width capped; dashboard Model policy
    chip now shows real provider name (not a UUID prefix) with truncation;
    ParameterBar selects cap at `max-w-[16rem]` so a long option label
    cannot blow out the sticky row. See
    `docs/W46_DASHBOARD_HEADER_RESILIENCE.md`.
34. Run W47 after W43 and preferably after W46: add explicit assistant language
    settings for GPT/Claude/Kimi-backed chat, Build Chat, and LLM-backed widget
    text paths. The task should provide an auto/follow-user mode plus a curated
    major-language catalog, persist the selected BCP-47 language policy without
    storing secrets, inject the language requirement into Rust-owned provider
    prompts, and prove that unsupported or low-confidence provider behavior is
    shown honestly rather than faked. See
    `docs/W47_LLM_CONVERSATION_LANGUAGE_SETTINGS.md`.
35. Run W48 after W31/W39 and preferably after W38/W41: add typed Build Chat
    source mentions and multi-source composition. Users should be able to
    mention one or more datasource/workflow/source-backed widget inputs, send
    compact typed source context, and create or replace widgets that genuinely
    use every mentioned input. Validation must reject proposals that ignore a
    mentioned source or fake multi-source output. See
    `docs/W48_BUILD_CHAT_SOURCE_MENTIONS_AND_MULTI_SOURCE_COMPOSITION.md`.
36. Run W49 after W22/W29/W33 and preferably before more live-provider
    acceptance expansion: repair cost accounting so real provider usage does
    not show as zero cost, add explicit unknown-cost states when pricing is
    missing, and reduce provider-visible context by compacting older turns and
    bulky tool results while preserving local traces. See
    `docs/W49_CHAT_CONTEXT_ECONOMY_AND_COST_ACCOUNTING_REPAIR.md`.
37. Run W50 after W35 and preferably after W40/W41: add first-class pause/resume
    and schedule controls for dashboard refresh. Users should be able to pause
    automatic refresh without disabling manual refresh, choose common intervals
    from the UI, edit advanced cron with validation, persist pause/cadence across
    restart, and see honest automatic/manual/paused/invalid/running state in the
    dashboard header, Workbench, and Operations surfaces. See
    `docs/W50_DASHBOARD_REFRESH_PAUSE_AND_SCHEDULE_CONTROLS.md`.
38. Run W51 after W49 and preferably before broad multi-source/streaming
    provider rollout: add a Rust-owned, RTK-class provider-context compression
    layer for bulky tool, MCP, HTTP, datasource, pipeline, and intermediate
    provider outputs. The goal is 60-90% provider-visible reduction on
    representative large-output scenarios without losing answer quality,
    validation quality, provenance, errors, or local raw-artifact
    inspectability. See
    `docs/W51_PROVIDER_CONTEXT_COMPRESSION_AND_RAW_ARTIFACT_RETENTION.md`.

Do not give two agents simultaneous ownership of `src/lib/api.ts`, `src-tauri/src/models/*`, or command request/response shapes. Contract drift is already the main risk.

## Task Prompt Template

Use this template for future implementation agents:

```md
You are working in /Users/prvlv/Kimi_Agent_Локальный AI-дэшборд/datrina.

Task: Execute Wn from docs/RECONCILIATION_PLAN.md.

Read first:
- AGENTS.md
- docs/RECONCILIATION_PLAN.md
- the files listed under Wn Ownership

Do not edit:
- files outside Wn Ownership unless the plan explicitly allows it
- unrelated public roadmap or research files

Required output:
1. files changed,
2. commands/checks run and exact result,
3. remaining blockers or residual TODOs,
4. whether Wn acceptance checks are met.
```

## Non-Goals For Reconciliation

- Do not port `datrina` back to Node/Hono/Turborepo.
- Do not implement a public HTTP API in MVP.
- Do not implement full plugin marketplace or dynamic plugin SDK.
- Do not add arbitrary sandboxed JS execution.
- Do not require real OpenRouter/Ollama/OpenAI credentials for local smoke.
- Do not require real external MCP servers for baseline validation.
- Do not treat README claims as implementation proof.
