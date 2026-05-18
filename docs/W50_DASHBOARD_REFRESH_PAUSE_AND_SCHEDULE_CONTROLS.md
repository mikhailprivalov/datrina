# W50 Dashboard Refresh Pause And Schedule Controls

Status: shipped (v1)

Date: 2026-05-17

Shipped: 2026-05-18

## v1 notes

- Pause flag persists on `workflows.pause_state`; startup scheduler skips
  registration when `pause_state = paused`, so a paused schedule does not
  silently come back as ticking after restart.
- Five Tauri commands cover the contract: `pause_workflow_schedule`,
  `resume_workflow_schedule`, `set_workflow_schedule`, and the
  dashboard-scoped `pause_dashboard_schedules` /
  `resume_dashboard_schedules`. Every mutation returns a typed
  `WorkflowSummary` so React does not re-fetch to learn the new state.
- Cron input is normalized through the existing
  `normalize_cron_expression` helper before persisting. Invalid input
  flows back as a typed error with the same message scheduler health
  surfaces.
- UI surfaces: dashboard-header `DashboardScheduleControl` rolls up the
  per-widget workflows; Operations cockpit + Workbench reuse the same
  `ScheduleEditor` component so labels and validation stay identical.
- Scheduler health treats `paused_by_user` as intentional —
  `EnabledButNotScheduled` is no longer raised for paused rows.
- v1 deferrals: a separate `last_manual_run` timestamp, `next_run`
  introspection from `tokio_cron_scheduler`, and timezone-aware
  scheduling are out of scope and noted under [Out of scope].

## Context

Datrina already has scheduled workflow refreshes, shared datasource fan-out,
Operations visibility, and dashboard widget refresh paths. The remaining UX gap
is control: an operator should be able to pause automatic dashboard refresh and
adjust refresh frequency from the interface without editing raw cron strings,
rebuilding a widget, or asking the agent.

This task adds first-class pause/resume and schedule controls over the existing
workflow scheduler. It must not add a second scheduler, bypass datasource
identity, or make paused data look fresh.

## Goal

- Users can pause and resume automatic refresh for a dashboard, datasource, or
  scheduled workflow from the UI.
- Manual refresh remains available while automatic refresh is paused.
- Users can choose common intervals through a friendly control and can still use
  advanced 6-field cron when needed.
- Cron input is normalized through the existing scheduler rules before saving;
  invalid schedules are rejected with actionable UI errors.
- Dashboard/header, Workbench, and Operations surfaces show whether refresh is
  automatic, paused, manual-only, invalid, or currently running.
- Paused schedules survive app restart and do not silently re-register jobs on
  startup.
- Existing W35 scheduler health and W41 observability surfaces reflect pause
  state rather than reporting paused jobs as broken.

## Approach

1. Define schedule state.
   - Extend the persisted workflow/datasource schedule model with explicit
     paused/enabled/manual-only state if the existing `is_enabled` field is not
     specific enough.
   - Preserve the existing cron trigger shape and 6-field cron normalization.
   - Record enough metadata to distinguish user-paused schedules from invalid or
     failed scheduler registration.

2. Add schedule commands.
   - Add commands to pause/resume automatic refresh and update refresh cadence
     for dashboard-owned, datasource-owned, and direct workflow schedules.
   - Return typed schedule summaries after every mutation so React does not need
     to infer state from stale local data.
   - Unschedule paused jobs immediately and re-register resumed valid jobs
     through the existing `Scheduler` path.

3. Build schedule controls.
   - Add compact controls in the dashboard header for pause/resume and current
     refresh cadence.
   - Add richer schedule editing in Workbench and Operations where owner/source
     context is already visible.
   - Offer presets such as manual only, every 1/5/15/60 minutes, and advanced
     cron.
   - Use stable controls, validation messages, and disabled/loading states; do
     not hide scheduler errors in logs.

4. Keep runtime semantics honest.
   - Manual refresh still runs through the normal `refresh_widget`/workflow
     execution path.
   - Paused automatic refresh does not update snapshots, run alerts, or claim
     current datasource health.
   - Existing cached/stale data remains visible with stale/paused labeling.
   - Build Chat proposals may suggest a refresh cadence, but applying it must
     still go through validation and explicit confirmation.

5. Integrate observability.
   - Operations schedule summaries show paused/manual-only/invalid/registered
     state and owner links.
   - Scheduler health treats user-paused schedules as intentionally inactive, not
     `EnabledButNotScheduled`.
   - Widget details from W41 show last automatic run, last manual run, next run
     when scheduled, and paused status.

## Files

- `src-tauri/src/models/workflow.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/scheduler.rs`
- `src-tauri/src/commands/workflow.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/layout/TopBar.tsx` if it exists, or the current dashboard
  header component.
- `src/components/datasource/Workbench.tsx`
- `src/components/operations/*`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W50_DASHBOARD_REFRESH_PAUSE_AND_SCHEDULE_CONTROLS.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - cron normalization and rejection of invalid schedule input,
  - pause unschedules an existing cron job and persists across restart,
  - resume re-registers only valid schedules,
  - manual refresh still works while automatic refresh is paused,
  - scheduler health does not flag user-paused jobs as broken,
  - schedule summaries round-trip through Rust models and `src/lib/api.ts`.
- Manual running-app smoke:
  - create a datasource-backed dashboard with automatic refresh,
  - pause refresh from the dashboard header and confirm no automatic job runs,
  - run manual refresh and confirm widget data updates,
  - change cadence from a preset and from advanced cron,
  - reload the app and confirm pause/cadence state survives,
  - inspect Operations and Workbench schedule state for the same source.

## Out of scope

- Replacing `tokio_cron_scheduler` or adding a second queue engine.
- Per-user/team schedule permissions or cloud sync.
- Calendar/timezone scheduling beyond the existing cron runtime.
- Background refresh while the desktop app is closed.
- Letting LLM-generated widget positions or proposals bypass schedule
  validation.
- Treating stale snapshots as fresh data while automatic refresh is paused.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W21_ALERTS_AUTONOMOUS_TRIGGERS.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`
- `docs/W40_WIDGET_RUNTIME_PERFORMANCE.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
