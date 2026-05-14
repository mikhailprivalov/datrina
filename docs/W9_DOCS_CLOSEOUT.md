# W9 Docs And Residual Closeout

Status: accepted on 2026-05-14.

## Scope Handled

W9 was documentation-only. It aligned the public README with the implemented MVP baseline and preserved the reconciliation plan as historical execution context.

Changed documentation:

- `README.md` now separates current MVP behavior, intentionally limited behavior, and planned/non-MVP behavior.
- `docs/RESIDUAL_BACKLOG.md` records concise deferred work grouped by production readiness, workflow/scheduler, tools/MCP, AI/chat, dashboards/widgets, and research promises.
- `docs/RECONCILIATION_PLAN.md` marks W9 accepted and points to this closeout record.

## Documented Baseline

The public docs now state:

- The active runtime is Tauri v2 with React UI and Rust backend.
- The validated local path is the `local_mvp` dashboard template, deterministic local workflow, persisted run, `workflow:event`, and gauge widget refresh.
- Chat is Rust-mediated and provider-backed, with `local_mock` as the deterministic no-key success path.
- MCP is stdio-only in MVP and guarded by the Rust tool policy gateway.
- Scheduler is registration-only in MVP.
- Dashboard generation, tool-calling chat, MCP/LLM workflow nodes, widget post-process steps, remote MCP, plugin marketplace, cloud/team features, and production packaging are not claimed as working MVP behavior.

## Validation Run

Executed from `datrina/` on 2026-05-14:

- `rg -n "TODO|not implemented|placeholder" src src-tauri/src --glob '!src-tauri/gen/**' || true`: no matches.
- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 33 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed. Vite reported the existing non-failing chunk-size warning.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets`: passed. Rust reported the existing non-failing deprecation warning for `tauri_plugin_shell::Shell::open`.

Environment note: `git status --short` could not be used because this directory is not inside a git repository.

## Acceptance

- README separates implemented, intentionally limited, and planned behavior.
- Deferred research promises are listed without implying MVP support.
- Validation commands and prerequisites are documented.
- Residual deferred work is recorded in `docs/RESIDUAL_BACKLOG.md`.
