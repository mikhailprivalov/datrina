# W30 Datasource And Pipeline Workbench

Status: partial-shipped (v1 surface landed 2026-05-17; pipeline editor and
apply-time signature reuse deferred — see Residuals)

Date: 2026-05-17

## Context

Datrina has a real dashboard runtime: datasource plans, shared source fan-out,
typed pipeline steps, parameter substitution, refresh runs, alert evaluation,
pipeline traces, Playground exploration, and AI-generated Build proposals.

The product still feels more like "agent-generated widgets over hidden
workflows" than a mature AI dashboard tool. The missing layer is a first-class
operator workbench where data sources, pipeline shaping, health, consumers, and
manual fixes are visible and reusable. Without that layer, AI remains useful
but too magical: the user has to ask chat to rewrite JSON or rebuild widgets
for normal dashboard-operations work.

This workstream promotes existing primitives into an inspectable and editable
datasource/pipeline surface. It must not add a second engine.

## Goal

- Datasources become first-class saved product objects over the existing
  MCP/HTTP/provider/workflow primitives.
- A datasource has a name, kind, source config, optional schedule, optional
  parameters, base pipeline, last-run status, sample output, error state, and
  list of consuming widgets.
- Users can create or save a datasource from Playground, Build Chat, or a manual
  "New datasource" path.
- Users can save reusable query/pipeline presets with title, description, tags,
  datasource kind, parameter mappings, and validation status. These are local
  Datrina objects, not a team/RBAC query library.
- Users can bind one datasource to multiple widgets with per-widget tail
  pipelines, field mappings, thresholds, units, and display options.
- The Pipeline Debug view grows into an edit/test/save loop: inspect a trace,
  adjust a typed pipeline step, run against the sample, compare before/after,
  then save.
- Build Chat learns to reuse existing datasources and propose datasource
  updates instead of duplicating hidden workflows.
- TypeScript/Rust parity is repaired for nested runtime shapes that the
  Workbench exposes, especially `PipelineStep`, `ValidationIssue`, datasource
  plans, and parameter references.
- Inspectability matches the useful observability pattern: raw data, transformed
  data, request/config JSON, query statistics, errors, and exportable panel/data
  JSON are available from the same operator surface.

## Approach

1. Define a saved datasource model.
   - Add a narrow `DatasourceDefinition` model that maps onto existing
     `BuildDatasourcePlan`, `SharedDatasource`, workflow source nodes, and
     widget runtime bindings.
   - Persist definitions in SQLite with version/update timestamps and a small
     health snapshot.
   - Avoid adding a new query engine. Execution still flows through
     `ToolEngine`, `MCPManager`, provider prompt nodes, workflow execution,
     parameter substitution, and the pipeline DSL.

2. Add datasource commands and API bindings.
   - CRUD: create, update, delete, list, get.
   - Runtime: test/run once, list consumers, list recent runs/traces, duplicate.
   - Migration/bridge: convert Build proposal shared datasource entries and
     Playground presets into saved definitions when the user chooses to save.
   - Contract checks must cover nested model parity, not only command names.
   - Add local import/export for datasource definitions, query presets, and
     dashboard/widget bindings as versioned JSON. This is for backup,
     reproducible handoff, and debugging; it is not full Git Sync.

3. Build the Workbench UI.
   - Add a datasource route or panel reachable from Sidebar and widget menus.
   - Left side: datasource catalog with kind, health, last run, consumer count.
   - Main pane: source config, parameters, schedule, sample output, consumers.
   - Pipeline editor: typed step rows, reorder, add/remove, JSON fallback only
     for unsupported advanced step config.
   - Query options: max data points/items, min interval, timeout/cache hint,
     relative time override, and per-panel refresh behavior where those concepts
     map onto the existing datasource/workflow runtime.
   - Result/trace pane: sample output, per-step trace, first-empty-step hint,
     before/after diff for edited steps.
   - Inspector tabs: raw source result, transformed result, runtime request,
     timing/statistics, panel/widget JSON, data JSON, and error details.

4. Integrate with dashboard widgets.
   - Widget editor exposes datasource binding, output path, per-widget tail
     pipeline, field mapping, thresholds, units, refresh schedule, and display
     options for common widget kinds.
   - One datasource can feed multiple widgets; shared refresh behavior remains
     row-first and scheduler-owned.
   - DashboardGrid actions link to Workbench for "Edit datasource" and
     "Debug pipeline".

5. Integrate with Playground and templates.
   - Playground "Use as widget" offers "Save datasource" before or alongside
     opening Build Chat.
   - Playground can save a reusable query/pipeline preset after a successful
     run; saved presets must carry the sample shape and last validation result.
   - Templates can reference datasource setup requirements instead of relying
     only on prompt text.
   - Saved presets and datasources should not diverge silently; either a preset
     creates a datasource, or it stays clearly labeled as a Playground-only
     preset.

6. Integrate with Build Chat.
   - The Build system prompt includes existing datasource summaries and
     instructs the provider to reuse them when appropriate.
   - The prompt includes saved query/pipeline preset summaries when relevant,
     with parameter mapping hints instead of full large samples.
   - `BuildProposal` can carry datasource create/update/bind intent, or the
     apply path translates compatible `shared_datasources` into saved
     definitions with explicit preview.
   - Validation prevents duplicate hidden sources when a reusable datasource
     already satisfies the request.
   - `dry_run_widget` can run against saved datasource definitions and parameter
     values.

7. Fix nested contract drift first.
   - Mirror all pipeline variants that can be persisted or displayed in the
     Workbench, including Rust-only variants currently missing in TypeScript.
   - Mirror all validation issue variants that can be emitted by W16/W25/W29.
   - Add static or generated checks that catch nested enum drift, not just
     missing Tauri commands.

8. Keep provenance visible.
   - Any widget created from Build Chat, Playground, a saved preset, or manual
     editor records its datasource definition id, query/pipeline preset id when
     applicable, and latest run/trace id.
   - The UI must answer: "where did this value come from?", "what transformed
     it?", "who/what last changed it?", and "which widgets will be affected if
     I edit this source?"

## Files

- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/pipeline.rs`
- `src-tauri/src/models/validation.rs`
- new datasource model file under `src-tauri/src/models/` if cleaner
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/parameter_engine.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/debug.rs`
- new datasource command file under `src-tauri/src/commands/` if cleaner
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/components/debug/PipelineDebugModal.tsx`
- new Workbench components under `src/components/datasource/` or
  `src/components/debug/`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/playground/Playground.tsx`
- `src/lib/templates/index.ts`
- `src/components/dashboard/ParameterBar.tsx` only if option resolution is
  needed for the Workbench acceptance path
- `docs/RECONCILIATION_PLAN.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`

Crossing into chat files is allowed only for Build Chat datasource reuse:

- `src-tauri/src/commands/chat.rs`
- `src/lib/chat/runtime.ts`
- `src/components/layout/ChatPanel.tsx`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - datasource CRUD and consumer lookup,
  - datasource run/test using existing workflow engine,
  - pipeline step edit serialization,
  - query/pipeline preset save/reuse with parameter mapping,
  - local import/export round-trip for a datasource-backed dashboard,
  - nested Rust/TypeScript parity for pipeline and validation shapes,
  - parameter substitution inside datasource config.
- Manual Workbench smoke:
  - create one HTTP or MCP datasource,
  - run it and inspect sample output,
  - add a base pipeline,
  - bind it to two widgets with different tail pipelines,
  - parameterize the source,
  - intentionally break one step and confirm trace names the failing step,
  - fix the step through the Workbench and save,
  - inspect raw result, transformed result, request JSON, timing/statistics,
    panel/widget JSON, data JSON, and error details,
  - refresh widgets and verify both update through the shared datasource,
  - export and re-import the datasource-backed dashboard into a clean local
    profile or test storage,
  - reload the app and confirm datasource, consumers, health, and traces remain
    understandable.
- Manual AI smoke:
  - Build Chat reuses an existing datasource instead of creating a duplicate,
  - proposal preview shows datasource create/update/bind changes explicitly,
  - apply still requires confirmation and preserves W29 validation gating.

## Out of scope

- A new data query language or replacement workflow engine.
- Arbitrary JavaScript execution inside pipelines.
- Remote datasource marketplace or cloud sharing.
- Full GitOps/provisioning sync. Local import/export is allowed; continuous
  file watching, Git Sync, team RBAC, and conflict resolution are not.
- Team permissions, OAuth, cloud sync, or public HTTP API.
- Full observability-platform parity: alert rule editor redesign, repeating panels, dashboard
  folders, library panels, annotations, and plugins stay out unless a later
  workstream adds them explicitly.
- Replacing AI Build Chat. The Workbench complements the agent by making its
  outputs inspectable, editable, and reusable.

## What landed in v1 (2026-05-17)

- `DatasourceDefinition` model + `datasource_definitions` /
  `datasource_health` SQLite tables (storage CRUD + health upsert + lookup
  by backing workflow id, unit-tested).
- 10 Tauri commands wired through `commands/datasource.rs`:
  list / get / create / update / delete / duplicate /
  run / list-consumers / export / import. All routed through the existing
  `WorkflowEngine` and `datasource_plan_workflow` — no parallel engine.
- `src/lib/api.ts` mirrors all new shapes (`DatasourceDefinition`,
  `DatasourceHealth`, `DatasourceConsumer`, `DatasourceRunResult`,
  `DatasourceExportBundle`, `ImportDatasourcesResult`) and exposes
  `datasourceApi`.
- Workbench panel at `#/workbench` with sidebar nav: catalog (health,
  consumer count, last-run), source/pipeline form, raw + final value
  inspectors, consumer list, duplicate / delete / test-run / save / import
  / export.
- Build Chat reuse: the saved-datasource catalog is injected into the
  Build mode system prompt so the agent prefers re-emitting a matching
  source key instead of inventing a duplicate.
- Contract drift fixes (the prerequisite for nested editors): TS now
  mirrors `PipelineStep::McpCall` and both W25 `ValidationIssue`
  variants. `scripts/check-contract.mjs` grew a generic parity check
  that fails when any future Rust variant of `PipelineStep` /
  `ValidationIssue` ships without a TS twin.

## Residuals (deferred from v1)

- **Typed pipeline step editor.** Today the Workbench edits pipelines as
  raw JSON. A row-based editor (reorder, add/remove, schema-aware
  config) is the natural follow-up.
- **Per-widget tail pipelines via the Workbench.** A definition currently
  produces a single-output workflow; the existing fan-out helper
  (`build_shared_fanout_workflow`) already proves the shape, but binding
  multiple widgets with per-widget tails through the Workbench is not
  wired yet.
- **Apply-time signature reuse.** Build Chat is *told* about saved
  definitions in v1, but the apply path does not yet promote a matching
  `shared_datasources` entry into a binding on the saved definition; it
  still creates a fresh workflow. Bridge needs a canonical
  `(kind, server_id, tool_name, arguments_canonical_json)` signature
  match + a `datasource_id` field on `BuildWidgetProposal`.
- **W23 Debug "edit + save" loop.** Inspecting a trace and editing the
  failing step through the same modal is still split between the Debug
  modal and the Workbench editor.
- **Playground "Save as datasource"** button — Playground still produces
  presets only.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W17_AGENT_MEMORY_RAG.md`
- `docs/W20_DATA_PLAYGROUND_TEMPLATES.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W25_DASHBOARD_PARAMETERS.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
