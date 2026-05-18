# W35 Workflow Operations Cockpit

Status: shipped — v1 cockpit, scheduler health, retry, honest cancel.

Date: 2026-05-17

## Context

Datrina now has persisted workflows, scheduled refresh, widget runtime status,
pipeline traces, alerts, and saved datasources. What is still missing is an
operator cockpit for the runtime itself: which workflows are scheduled, which
runs failed, which jobs are stuck, and what can be retried or cancelled without
opening raw storage or asking the agent.

This workstream adds operational control over the existing scheduler/workflow
runtime. It must not become a second workflow editor or queue engine.

## Goal

- Add a Workflows/Operations view for run history, schedules, health, errors,
  and manual actions.
- Operators can inspect a workflow's source, owner dashboard/widget/datasource,
  last runs, next schedule, and latest trace/error.
- Operators can retry a failed run and refresh affected widgets.
- Add explicit cancellation command/model behavior for in-flight workflows when
  the runtime can support it honestly.
- Detect and surface stuck runs, disabled schedules, invalid cron, reconnect
  failures, and provider/setup errors.
- Alerts and Workbench link to the relevant workflow/run details.

## Approach

1. Define workflow operation models.
   - Add run summaries, schedule summaries, owner references, and health
     statuses without changing the persisted workflow execution contract more
     than necessary.
   - Keep dashboard/widget/datasource ownership visible.

2. Add commands.
   - List workflow summaries.
   - List runs for workflow/dashboard/datasource/widget.
   - Get run details.
   - Retry a run or rerun workflow once.
   - Cancel in-flight run if supported; otherwise return explicit unsupported
     status and record the residual.

3. Build Operations UI.
   - Sidebar entry or Workbench tab for workflows.
   - Filters: failed, running, scheduled, datasource-owned, dashboard-owned,
     provider/MCP failures.
   - Run detail pane: node results, timing, error, trace link, owner links.
   - Actions: retry, open datasource, open widget, open pipeline debug, disable
     schedule where supported.

4. Integrate existing surfaces.
   - Workbench datasource health links to workflow runs.
   - Dashboard widget runtime status links to run details.
   - Alerts show the run that triggered the event where available.
   - Scheduler startup errors are visible without inspecting logs.

5. Keep advanced queue behavior explicit.
   - Do not add priority/dead-letter/retry policy unless the current runtime
     needs it for truthful operations.
   - If automatic retry is added, keep it bounded and visible.

## Files

- `src-tauri/src/models/workflow.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/scheduler.rs`
- `src-tauri/src/commands/workflow.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- new operations components under `src/components/operations/`
- `src/components/layout/Sidebar.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/alerts/AlertsView.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`

## Validation

- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021`
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - workflow summary listing,
  - run detail retrieval,
  - retry/rerun command,
  - cancellation status behavior,
  - schedule health/invalid cron reporting,
  - owner reference resolution for dashboard/widget/datasource.
- Manual smoke:
  - create a datasource-backed widget,
  - run it successfully and inspect the run in Operations,
  - break the datasource and confirm failure appears with actionable error,
  - retry after fixing it,
  - inspect a scheduled workflow's next/last run state,
  - navigate from Workbench and DashboardGrid into the run detail.

## Out of scope

- Full visual workflow graph editor.
- New queue engine, priority scheduling, or dead-letter system unless explicitly
  required by a discovered runtime blocker.
- Public HTTP operations API.
- Team operations/RBAC/cloud sync.
- Auto-repair by agent without user confirmation.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W21_ALERTS_AUTONOMOUS_TRIGGERS.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
