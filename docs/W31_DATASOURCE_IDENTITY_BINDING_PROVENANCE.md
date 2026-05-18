# W31 Datasource Identity, Binding, And Provenance

Status: shipped

Date: 2026-05-17

## Context

W30 made saved datasources visible and runnable through the Workbench, but the
runtime identity is still mostly inferred from backing `workflow_id` and
matching `(kind, server_id, tool_name)` signatures. Build Chat can see saved
datasources in its prompt, yet an apply path can still mint a fresh shared
workflow that only resembles an existing datasource.

The next product gap is identity: when a widget uses a saved datasource, that
relationship must be explicit, persisted, versioned, exportable, and visible in
the Workbench. Operators need to know which datasource owns a value, which
widgets will be affected by edits, and which agent/manual action last changed
the binding.

## Goal

- Saved `DatasourceDefinition` is the source-of-truth identity for reusable
  data sources.
- Widgets can bind explicitly to `datasource_definition_id`, with a stable
  base datasource plus per-widget tail pipeline/output mapping.
- Build Chat can propose "bind to existing datasource" and "update existing
  datasource" changes, not only create hidden `shared_datasources`.
- Apply-time reuse promotes compatible shared datasource proposals into saved
  datasource bindings instead of creating duplicate workflows.
- Dashboard versions, diffs, import/export, Workbench consumers, and widget
  menus all show datasource provenance.
- Editing a datasource shows an impact preview before saving when consumers
  exist.

## Approach

1. Extend datasource/widget models.
   - Add an optional `datasource_definition_id` to the widget datasource binding
     model while preserving `workflow_id` for execution.
   - Add explicit tail pipeline/output mapping fields for per-widget consumers.
   - Keep the existing workflow engine as the only executor.

2. Add binding commands.
   - Bind/unbind a widget to a datasource definition.
   - List consumers by explicit datasource id, falling back to workflow scan only
     for legacy rows.
   - Preview affected widgets before datasource save/delete.

3. Integrate Build proposal apply.
   - Allow `BuildProposal` / `BuildWidgetProposal` to reference an existing
     datasource definition.
   - When a proposed `shared_datasources` entry matches a saved definition,
     bind consumers to the saved definition unless the proposal explicitly asks
     to create a new datasource.
   - Validation should flag duplicate hidden sources when an existing
     datasource satisfies the signature and sample shape.

4. Preserve provenance.
   - Dashboard version diffs should show datasource binding changed, datasource
     definition changed, and per-widget tail pipeline changed separately.
   - Workbench consumer rows should link to dashboard/widget and show the last
     binding source: Build Chat, Playground, Workbench, import, or manual edit.
   - Export/import must round-trip datasource-backed dashboards without losing
     identity.

5. Add migration compatibility.
   - Existing W30 datasources and widgets that share a `workflow_id` are
     discoverable and can be upgraded to explicit bindings without data loss.
   - Legacy dashboards still refresh even before upgrade.

## Files

- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/commands/chat.rs`
- `src/lib/api.ts`
- `src/components/datasource/Workbench.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/dashboard/HistoryDrawer.tsx`
- `src/components/dashboard/VersionDiffView.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - binding/unbinding a widget to a datasource definition,
  - consumer lookup by explicit datasource id,
  - apply-time reuse of a matching saved datasource,
  - duplicate-source validation,
  - export/import round-trip preserving datasource identity,
  - version diff separating datasource identity from widget config changes.
- Manual smoke:
  - create a datasource in Workbench,
  - bind two widgets with different tail pipelines,
  - ask Build Chat for a widget that should reuse the datasource,
  - confirm preview shows reuse/bind rather than duplicate source creation,
  - edit the datasource and inspect impact preview,
  - export/import the dashboard plus datasource into a clean profile,
  - reload and confirm Workbench provenance and consumers remain correct.

## W31.1 follow-up

Shipped as part of the same stream:

- `DatasourceConfig.tail_pipeline: Vec<PipelineStep>` — per-widget typed
  pipeline applied after the saved-datasource workflow output. Empty by
  default; non-empty tails run through
  `workflow_engine::run_pipeline_with_trace` so deterministic primitives,
  `mcp_call`, and `llm_postprocess` all work consistently.
- Build proposal apply: when a `shared_datasources` entry is reused
  against a saved definition, the consumer's `datasource_plan.pipeline`
  is copied into the widget's `tail_pipeline` instead of being baked into
  a separate fan-out workflow. Net effect: no shared workflow is built
  on reuse and each consumer keeps its own deterministic tail.
- `DatasourceConsumer.tail_step_count` surfaces in the Workbench rows.
- `bind_widget_to_datasource` accepts `tail_pipeline` so manual rebinds
  can capture the same shape.

## Out of scope

- Replacing the workflow engine.
- Full GitOps/provisioning sync.
- Team ownership, RBAC, or cloud sharing.
- Visual pipeline editor improvements beyond fields required for binding and
  provenance.
- New datasource marketplace or remote catalog.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W19_DASHBOARD_VERSIONS_UNDO.md`
- `docs/W20_DATA_PLAYGROUND_TEMPLATES.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
