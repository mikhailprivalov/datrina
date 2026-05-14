# W4 Dashboard Local UX And Data Plumbing

Status: accepted

## Scope Handled

W4 keeps the dashboard UI local-first and wired through the accepted
`src/lib/api.ts -> Tauri invoke -> Rust command` boundary.

Implemented frontend behavior:

- dashboard list/create/get/delete flows expose loading and error states,
- dashboard selection performs an explicit `get_dashboard` load,
- widget drag and resize commits persist layout through `update_dashboard`,
- widget refresh calls the approved `refresh_widget` command and displays an
  explicit unavailable/error state when Rust returns no runtime data,
- widget components render only explicit runtime data props,
- empty dashboards and missing widget runtime data show local empty states.

## Runtime Data Boundary

Widget configuration remains the persisted/presentational shape from Rust
models. Runtime data is separate and lives in frontend state as typed
`WidgetRuntimeData`.

W4 added a narrow `WidgetDataEventEnvelope` TypeScript interface only as a
future integration point. W7 still owns final Tauri event names, subscription
setup, and producer semantics.

## Removed Hidden Demo Paths

The TS/TSX widget components no longer contain built-in demo datasets.
Stale compiled `.js` duplicates under `src/` were removed so Vite resolves the
current TS/TSX implementation rather than old generated files.
`tsconfig.json` now keeps `tsc` typecheck-only, and `vite.config.ts` prefers
TS/TSX extensions before JS for extensionless imports.

## Residuals

- `refresh_widget` still returns no real widget data from Rust. This is shown
  to the user as unavailable instead of fake success; the producer path belongs
  to W7/W8.
- There is still no UI in W4 for adding new widgets. W4 preserves the local
  grid and persistence plumbing for dashboards/widgets created by later build
  or workflow paths.

## Validation Run

Executed on 2026-05-14:

- `bun run typecheck` passed.
- `bun run check:contract` passed: 33 frontend commands match Rust
  registrations.
- `bun run build` passed. Vite reported the existing large chunk warning.
- `cargo test --workspace storage::tests::dashboard_config_provider_mcp_and_workflow_persistence_smoke`
  passed. Rust reported the existing deprecated
  `tauri_plugin_shell::Shell::open` warning in `system.rs`.
