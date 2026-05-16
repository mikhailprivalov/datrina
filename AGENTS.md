# Agent Instructions

This file is the instruction layer for AI agents working with the `datrina`
project directory directly.

Keep it short and operational. Detailed execution work lives in
`docs/RECONCILIATION_PLAN.md`.

## Project Shape

- `datrina/` is the active product implementation.
- `docs/RECONCILIATION_PLAN.md` is the current execution plan for reconciling
  research, planning, and implementation.
- `../plan.md`, `../plan-v2.md`, and `../research/*` are context sources. They
  are not executable task queues unless a task says so explicitly.
- The active implementation direction is a Tauri v2 desktop app with React UI
  and Rust backend.

Do not port `datrina` back to Node/Hono/Turborepo as part of reconciliation.
Translate applicable research concepts into Tauri commands, Rust modules, and
Tauri events.

## Before Working

For any non-trivial task:

1. Read this file.
2. Read `docs/RECONCILIATION_PLAN.md`.
3. Read only the files owned by the requested workstream or directly needed for
   the bug/fix.
4. Identify the exact workstream, gate, or narrow file scope before editing.

If the user asks to execute `W0`, `W1`, or another reconciliation workstream,
use the matching section in `docs/RECONCILIATION_PLAN.md` as the task contract.

## Execution Policy

- Keep changes inside this `datrina/` directory unless the user explicitly asks
  for root-level planning or research edits.
- Respect workstream ownership. Do not edit another workstream's files unless
  the current task requires it and you call that out in the handoff.
- Do not treat README claims as implementation proof. Verify behavior from code
  and local checks.
- Do not silently broaden a workstream into adjacent feature work.
- Do not leave fake success paths. Placeholder behavior must be removed,
  returned as explicit unsupported behavior, or recorded as a residual task.
- Do not require real LLM credentials, real external MCP servers, cloud
  services, Docker, or production packaging for baseline validation unless the
  workstream explicitly requires them.
- Preserve local-first behavior and keep secrets out of the React side unless a
  recorded decision says otherwise.

## Reconciliation Workstream Order

Default order:

1. `W0` source lock and decision record.
2. `W1` build and config baseline.
3. `W2` frontend/Rust contract baseline.
4. `W3` storage/config/secrets and `W4` dashboard local UX may run in parallel
   after `W2`.
5. `W5` MCP/tool security and `W6` AI/chat boundary may partially overlap after
   `W2`/`W3`, but tool calling waits for `W5`.
6. `W7` workflow/scheduler/events.
7. `W8` MVP vertical slice.
8. `W9` docs and residual closeout.
9. `W10` end-to-end product runtime after W9 when the task is to make the
   product work beyond the accepted reconciliation slice.
10. Later `W11+` product-readiness and runtime streams follow
    `docs/RECONCILIATION_PLAN.md` in order. Active product backlog as of
    2026-05-16: `W16` (Proposal Validation Gate) implemented; `W17`–`W25`
    planned with one dedicated `docs/W<N>_*.md` each. New W tasks must
    ship a doc in `docs/` with the same `Status / Context / Goal /
    Approach / Files / Validation / Out of scope / Related` shape.

When multiple agents are used, split by ownership paths from the plan. Do not
run concurrent agents over `src/lib/api.ts`, `src-tauri/src/models/*`, or
command request/response shapes.

## Validation

Use the checks listed in the active workstream. Common checks from this
directory include:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract` — Rust commands ↔ TypeScript bindings parity.
  Adding or renaming a Tauri command without a matching `api.ts` mirror
  breaks this gate before runtime.
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check`
- `cargo check --workspace --all-targets`

If dependencies or toolchains are missing, record the exact blocker. Do not
claim acceptance checks passed when they were not run.

When `cargo fmt --all --check` reports drift in files outside the current
workstream's scope, prefer `rustfmt --edition 2021 <changed-files...>` over
`cargo fmt --all`. The workspace has pre-existing format drift in
unrelated modules; do not silently restyle them as part of an unrelated
change.

## Hard Invariants (Don't Re-learn These)

These bit a previous session and cost real time. They live here so the
next agent does not repeat the debugging.

- **Tauri v2 events require capabilities.** `src-tauri/capabilities/default.json`
  must grant at least `core:default` and `core:event:default`. An empty
  or missing capabilities file silently breaks every `listen()` call in
  React — the UI looks dead even though the Rust side is working. If
  the chat ever streams nothing past `MessageStarted`, suspect this
  file first.
- **Cron is 6-field.** The scheduler uses `tokio_cron_scheduler` which
  rejects POSIX 5-field expressions and panics on parse. Run every cron
  string through `commands::dashboard::normalize_cron_expression`
  before scheduling. Startup workflow rescheduling already does this.
- **Layout grid is 12 columns; auto-pack always wins.** When a Build
  proposal lands, the apply path ignores LLM-supplied `x` / `y` for new
  widgets and packs row-first on a 12-col grid. The Build system prompt
  also tells the agent not to set them. Do not add a config knob to
  honour LLM positions without an explicit decision.
- **MCP tool results are wrapped.** A stdio MCP `tool_call` returns
  `{ "content": [{ "text": "<json>" }] }`. The `McpTool` workflow node
  unwraps this envelope at the source. Downstream pipeline steps operate
  on the parsed root, not on the envelope. If a widget renders the
  envelope shape, the unwrap regressed.
- **Pipeline DSL first; `llm_postprocess` last.** Deterministic
  `PipelineStep` variants (`pluck` / `filter` / `sort` / `aggregate` /
  `map` / `format` / `coerce` / ...) are the default. `llm_postprocess`
  exists for shapes the typed steps cannot produce, not for shapes the
  agent is too lazy to express.
- **Build proposal delta semantics.** `BuildWidgetProposal.replace_widget_id`
  replaces a widget by id; `BuildProposal.remove_widget_ids` removes by
  id; everything untouched stays. Apply never wipes the dashboard.
- **Shared datasources fan-out.** One workflow per shared `key`, with
  `output_<widget_id>` taps per consumer widget. The cron lives on the
  shared workflow, so a single tick refreshes every consumer.
- **Anti-hardcode mandate.** Stat / gauge / bar_gauge values must come
  from the datasource pipeline. A numeric literal baked into widget
  config is a validator-blocking issue (W16). Same family: text widgets
  must contain markdown, never a stringified JSON object.
- **Runtime SQLite DB is read-only when auditing.** It lives at
  `~/Library/Application Support/app.datrina.desktop/app.db`. Inspect
  with `sqlite3 --readonly`. Schema migrations go through
  `Storage::migrate()` at app startup — never run ad-hoc `ALTER` /
  `UPDATE` against this file from a debug session.
- **Chat list uses lightweight summaries.** `list_session_summaries`
  pulls `(id, title, message_count, preview)` via SQLite `json_extract`
  / `json_array_length`; full `ChatSession` only loads on click. Do not
  bring back `list_chat_sessions` for the sidebar.

## Agent Run Discipline (post-W16)

When editing `commands/chat.rs` or `modules/ai.rs`:

- `MAX_TOOL_ITERATIONS = 40` is the hard cap. On top of that, a 5-entry
  repeat-window short-circuits a third identical `(tool_name,
  canonical_json(args))` with a synthetic `loop_detected` tool result.
  Don't remove this guard without an explicit replacement strategy.
- In Build mode, after the natural tool loop exits, the proposal goes
  through `commands::validation::validate_build_proposal` **before** the
  UI sees the preview. Issues trigger one synthetic `[validation_failed]`
  system-message retry via `complete_chat_with_tools_json_object`. New
  failure modes must extend `ValidationIssue` in both
  `src-tauri/src/models/validation.rs` and `src/lib/api.ts`.
- `complete_chat_with_tools_json_object` is the retry-only sibling of
  `complete_chat_with_tools` — it sets
  `response_format: { "type": "json_object" }` for OpenAI-compatible
  providers and falls back silently for Ollama / LocalMock.
- Streaming uses a 60 s first-byte timeout
  (`provider_first_byte_timeout`) plus mid-stream recovery that keeps
  accumulated content as the final answer when the stream errors after
  text already arrived. Both behaviours are intentional, not bugs.
- The spawned chat task is panic-safe via
  `futures::FutureExt::catch_unwind`. A panic emits `MessageFailed`; the
  UI never sticks on `isLoading=true`.
- `dry_run_widget` is recommended (not yet enforced) for stat / gauge /
  bar_gauge / status_grid / aggregating tables. The W16 validator
  produces `MissingDryRunEvidence` when the agent skipped it; the agent
  is then given one chance to retry with the dry-run done.

## Handoff Format

Every implementation or review response should include:

- workstream or scope handled,
- files changed,
- checks run and exact outcome,
- acceptance checks met or still blocked,
- residual TODOs or risks.

If the task changes a workstream status or closes an open decision, update
`docs/RECONCILIATION_PLAN.md` in the same change unless the user asked for
report-only work.

## Review Mode

For review-only requests, do not edit files. Report findings first, ordered by
severity, with concrete file and line references.

## Documentation Policy

- Keep `AGENTS.md` as the concise routing and guardrail document.
- Keep repeatable execution detail in `docs/RECONCILIATION_PLAN.md` or a future
  dedicated workflow document.
- Do not duplicate the full plan into README.
- README should describe implemented, in-progress, and planned behavior
  honestly after the corresponding workstream asks for docs closeout.
