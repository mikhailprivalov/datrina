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

When multiple agents are used, split by ownership paths from the plan. Do not
run concurrent agents over `src/lib/api.ts`, `src-tauri/src/models/*`, or
command request/response shapes.

## Validation

Use the checks listed in the active workstream. Common checks from this
directory include:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check`
- `cargo check --workspace --all-targets`

If dependencies or toolchains are missing, record the exact blocker. Do not
claim acceptance checks passed when they were not run.

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
