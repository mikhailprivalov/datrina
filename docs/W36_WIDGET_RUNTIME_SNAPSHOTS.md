# W36 Widget Runtime Snapshots

Status: shipped — v1 hydrate-from-cache, fingerprint invalidation, alerts read live only.

Date: 2026-05-17

## Context

After restart, Datrina currently reconstructs visible widget data by refreshing
widgets again. That is correct for live data, but it makes the first dashboard
paint slower and fragile when the datasource/provider/MCP service is temporarily
unavailable. A dashboard that had useful data seconds ago can reopen as empty or
loading until every datasource runs again.

The product needs a local "last known good" widget runtime snapshot layer:
render the most recent successful widget value immediately, mark it as cached,
then refresh through the normal runtime path.

## Goal

- Persist the last successful rendered runtime value for each widget.
- On app/dashboard load, show cached widget data immediately before live refresh
  finishes.
- Cached data is visibly marked as cached/stale and never treated as a fresh
  successful refresh.
- A live refresh replaces the snapshot only on successful runtime data.
- Failed refresh keeps the previous snapshot visible with a clear error/stale
  state.
- Snapshots are invalidated or marked incompatible when widget config,
  datasource binding, pipeline, parameter values, or dashboard version changes
  make the old value unsafe to display as-is.
- Snapshot storage is local-only and bounded.

## Approach

1. Add snapshot model and storage.
   - Store `dashboard_id`, `widget_id`, widget kind, runtime data JSON,
     captured timestamp, source workflow/datasource id, parameter fingerprint,
     and a lightweight config fingerprint.
   - Keep only the latest snapshot per widget unless a small history is needed
     for debugging.
   - Delete snapshots when widgets/dashboards are deleted.

2. Write snapshots from the real runtime path.
   - After `refresh_widget` successfully produces `WidgetRuntimeData`, persist
     the rendered runtime value.
   - Do not persist raw datasource output as the display snapshot.
   - Do not update the snapshot on errors, unsupported states, or validation
     failures.

3. Load snapshots on startup/dashboard switch.
   - Add commands to list snapshots for a dashboard and optionally get one
     widget snapshot.
   - `App` should hydrate `widgetData` from snapshots immediately, then kick
     off the existing refresh path.
   - UI should show cached/stale metadata separately from live run state.

4. Add invalidation rules.
   - Compute a fingerprint from widget kind/config/datasource binding/output
     key/tail pipeline and relevant parameter values.
   - If the stored fingerprint does not match, either hide the snapshot or show
     it as incompatible only in an inspector/debug surface.
   - Restoring a dashboard version should use snapshots only when fingerprints
     match the restored widget state.

5. Keep operational truthfulness.
   - Cached display is a fast paint optimization, not acceptance evidence for
     datasource health.
   - Alerts/autonomous triggers must use live refresh data, not stale cached
     snapshots.
   - Build Chat / reflection may mention last cached value only as stale
     context and should prefer live traces/runs when available.

## Files

- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/dashboard.rs` if snapshot metadata is exposed there
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*` only if per-widget cached badges are needed there
- `src/components/alerts/*` only to prevent stale snapshots from feeding alerts
- `docs/RECONCILIATION_PLAN.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - snapshot write after successful widget refresh,
  - no snapshot update after failed refresh,
  - dashboard snapshot listing,
  - fingerprint match/mismatch behavior,
  - snapshot deletion on widget/dashboard deletion,
  - parameter value fingerprinting.
- Manual smoke:
  - create a datasource-backed dashboard and refresh widgets,
  - restart the app and confirm widgets render immediately from cache,
  - confirm cached/stale state is visible until live refresh completes,
  - break the datasource and restart again: cached value stays visible with a
    live refresh error,
  - edit widget config/pipeline and confirm incompatible cached data is not
    shown as fresh,
  - verify alerts still evaluate only from live refresh data.

## Out of scope

- Offline-first time-series storage.
- Historical chart backfill beyond the latest rendered value.
- Using cached snapshots as proof that a datasource is healthy.
- Cross-device sync.
- Replacing workflow run history or W35 Operations.
- Snapshotting provider secrets, MCP env, or raw tool outputs for display.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W19_DASHBOARD_VERSIONS_UNDO.md`
- `docs/W21_ALERTS_AUTONOMOUS_TRIGGERS.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`
