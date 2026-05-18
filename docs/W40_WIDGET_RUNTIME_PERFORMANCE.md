# W40 Widget Runtime Performance

Status: shipped

Date: 2026-05-17

## Shipped

- New `refresh_dashboard_widgets` Tauri command dedupes by `workflow_id`
  so widgets that share a workflow execute it once and only their
  per-widget tail pipelines run per consumer. Independent workflows
  run concurrently with a bounded cap
  (`MAX_CONCURRENT_DASHBOARD_REFRESHES = 4`).
- Refactored `refresh_widget_inner` to share helpers
  (`resolve_dashboard_parameter_selections`, `load_substituted_workflow`,
  `run_workflow_via_state`, `finalize_widget_refresh`) with the batched
  path so single-widget refresh behavior is unchanged.
- Frontend `App.tsx` now calls the batched command on dashboard load,
  apply, and restore. A monotonic per-widget supersede counter guarantees
  a slower previous refresh cannot overwrite a fresher result.
- `DashboardGrid` extracts a memoised `WidgetCell` so one widget's
  refresh tick does not re-render every sibling.
- W36 last-known-good snapshot hydration runs before any live refresh
  so cached widgets paint immediately and the `cached` badge stays
  visible until live data lands.
- Unit test `group_consumers_dedupes_shared_workflow_ids` proves the
  dedupe invariant at the grouping helper level.

## Context

Widgets currently feel slow during dashboard load and refresh. Some of that is
expected when a real datasource, MCP tool, or provider is slow, but the product
should not add avoidable latency through duplicate refreshes, sequential work
that could be parallel, heavyweight React rendering, repeated pipeline
evaluation, or missing last-known-good paint.

This task is a performance stream, not a behavior rewrite. It must preserve
local-first execution, W29 no-fake-success behavior, W36 snapshot honesty, and
the existing Rust-owned workflow/provider/tool boundary.

## Goal

- Dashboards with several datasource-backed widgets paint quickly.
- Widget refresh avoids redundant datasource, workflow, pipeline, and provider
  work when bindings already share a source.
- Independent widget refreshes run with bounded concurrency instead of a slow
  all-serial path.
- The UI shows cached/stale values immediately when W36 snapshots exist, then
  replaces them through normal live refresh.
- Slow upstream calls are visible as slow upstream calls, not hidden behind fake
  success or hardcoded fallback values.
- Performance regressions have repeatable measurement and acceptance checks.

## Approach

1. Add baseline instrumentation.
   - Measure dashboard load, first visible widget paint, per-widget refresh
     duration, datasource/tool/provider duration, pipeline duration, and React
     render/update cost where practical.
   - Reuse W23/W35 trace surfaces when possible rather than creating a second
     observability model.
   - Add a local benchmark fixture with mixed fast/slow widgets and shared
     datasources.

2. Remove duplicate refresh work.
   - Ensure shared datasource fan-out and W39 materialized datasource bindings
     execute one source run per shared key/signature.
   - Avoid re-running identical datasource/provider calls for every consumer
     widget when a shared workflow output can feed tail pipelines.
   - Deduplicate initial dashboard refreshes triggered by mount, session load,
     parameter hydration, and explicit user refresh.

3. Add bounded parallelism and backpressure.
   - Run independent widget/source refreshes concurrently with a small
     configurable cap.
   - Keep provider/MCP/tool calls cancellable or ignorable when a newer refresh
     supersedes an older one.
   - Prevent one slow widget from blocking unrelated widgets from painting.

4. Optimize UI rendering.
   - Keep widget components stable across unrelated refresh updates.
   - Batch widgetData/state updates where possible.
   - Avoid re-rendering the whole grid for one widget status tick.
   - Keep loading/skeleton states dimensionally stable so the grid does not
     jump during refresh.

5. Integrate snapshots without lying.
   - Use W36 last successful rendered snapshots for immediate paint.
   - Mark cached/stale data clearly until live refresh succeeds.
   - Do not count stale snapshots as datasource/provider health evidence.

## Files

- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/scheduler.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/models/widget.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*`
- `src/components/debug/PipelineDebugModal.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W40_WIDGET_RUNTIME_PERFORMANCE.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - shared datasource source runs not duplicated for multiple widgets,
  - independent widget refresh bounded concurrency,
  - stale/snapshot paint not treated as fresh refresh success,
  - superseded refresh result ignored when a newer run wins,
  - pipeline tail work executed per consumer without re-running the shared
    source.
- Manual running-app smoke:
  - open a dashboard with at least 8 mixed widgets,
  - confirm cached widgets paint immediately when snapshots exist,
  - refresh the dashboard and confirm independent widgets update without
    waiting for the slowest upstream call,
  - inspect timing output and identify source/pipeline/UI costs,
  - confirm no widget renders hardcoded fallback data on upstream failure.

## Out of scope

- Replacing the workflow engine.
- Cloud caching or cross-device cache sync.
- Using stale snapshots as live datasource health.
- Removing W23/W35 observability in favor of a new debug model.
- Adding fake/mock provider success paths to make benchmarks pass.
- Broad visual redesign beyond stable loading and update states.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`
