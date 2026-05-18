# W34 Parameterized Datasource Options

Status: shipped (v1, 2026-05-17)

Date: 2026-05-17

## Context

W25 added dashboard parameters and substitution, but v1 intentionally left
query-backed option lists as a plain input. W30 added saved datasources, which
creates a better primitive for parameters: option lists should come from the
same local workflow/runtime layer as widgets.

This workstream makes dashboard controls operational rather than static.

## Goal

- `mcp_query` and `http_query` parameter kinds resolve real option lists.
- Datasource-backed parameters can use a saved `DatasourceDefinition` plus a
  pipeline that returns `{ label, value }` options.
- Dependent parameters re-resolve in topological order when an upstream value
  changes.
- Parameter state can sync to the URL hash for local share/reload flows.
- Changing a parameter refreshes only affected widgets and datasources.
- Build Chat can propose query-backed parameters when the user asks for
  project/environment/service selectors.

## Approach

1. Implement option resolution.
   - Execute MCP/HTTP/datasource-backed parameter option queries through the
     existing Rust runtime.
   - Apply parameter substitution before option queries so cascading controls
     work.
   - Normalize outputs into `ParameterOption[]` with typed validation.

2. Update `ParameterBar`.
   - Render query-backed parameters as real selects/multiselects once options
     load.
   - Show loading, empty, and error states inline.
   - Preserve the previous selected value when a refresh temporarily fails.

3. Add dependency handling.
   - Re-resolve downstream parameters when upstream values change.
   - Detect cycles using the existing W25 validation path.
   - Avoid refreshing unaffected widgets.

4. Add datasource-backed options.
   - Allow a parameter to reference a saved datasource definition as the option
     source.
   - Support a parameter-specific pipeline tail that maps source data to
     options.
   - Surface option-source health in the Workbench where relevant.

5. Add URL hash sync.
   - Encode current parameter values in the hash for same-profile reload/share.
   - Do not imply cross-machine sharing or cloud sync.

## Files

- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/modules/parameter_engine.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/commands/chat.rs`
- `src/lib/api.ts`
- `src/components/dashboard/ParameterBar.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `docs/W25_DASHBOARD_PARAMETERS.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W34_PARAMETERIZED_DATASOURCE_OPTIONS.md`

## Validation

- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021`
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - MCP/HTTP option query resolution,
  - datasource-backed option query resolution,
  - dependent parameter topological re-resolution,
  - cycle and empty-option validation,
  - URL hash encode/decode,
  - affected-widget refresh selection.
- Manual smoke:
  - create a dashboard with `project` and dependent `release` controls,
  - resolve project options from an MCP/HTTP/datasource source,
  - change project and confirm release options refresh,
  - confirm only affected widgets refresh,
  - reload with hash state and confirm selections restore,
  - ask Build Chat to create a parameterized dashboard and inspect proposed
    parameter option sources.

## Shipped surface (v1, 2026-05-17)

- New `DashboardParameterKind::DatasourceQuery { datasource_id, pipeline }`
  variant in `src-tauri/src/models/dashboard.rs`; TS mirror at
  `src/lib/api.ts` exposes `kind: 'datasource_query'`.
- `src-tauri/src/modules/parameter_options.rs` runs MCP / HTTP / saved
  datasource queries through the existing `WorkflowEngine` (same path
  widget refresh uses), substitutes `$upstream` tokens into MCP arguments
  and HTTP url/body/headers, and normalizes outputs into
  `ParameterOption[]` (accepts `[{label, value}]`, `[{name, id}]`,
  `[{title, value}]`, plain scalar arrays, and key-only objects).
- `list_dashboard_parameters` resolves options in topological order so
  cascading selectors (env → service → version) just work.
- New `refresh_dashboard_parameter_options` command lets the UI
  re-resolve a single parameter on demand without touching the rest.
- `set_dashboard_parameter_value` now returns `downstream` —
  re-resolved states for every parameter that declared the changed one in
  its `depends_on`, so cascading dropdowns update without an extra
  round-trip.
- `ParameterBar` renders `mcp_query` / `http_query` / `datasource_query`
  as real selects (with loading spinner, inline option error, manual
  refresh button) and preserves the previously selected value when a
  backend refresh temporarily fails (renders as `<value> (stale)` so the
  user can see it is not in the current option set).
- New `src/lib/parameterHash.ts` encodes / decodes selections into the
  URL hash as `#?d=<dashboard>&p.<name>=<json>` and `DashboardGrid`
  wires `initialSelections` from the hash on mount + writes back on
  every commit. Profile-scoped only; no cloud share implication.
- Build Chat prompt teaches `datasource_query`, mentions `depends_on`
  for cascading queries, and clarifies the expected pipeline output
  shape.

## Residuals

- Parameter editor UI (a dedicated dashboard settings panel for
  creating / removing parameters by hand) is still out of scope — for
  now parameters land via Build Chat proposals or direct dashboard JSON
  edits.
- The URL hash encoder uses JSON for typed round-trip and skips URL
  shortening; long parameter lists produce long hashes.
- `depends_on` validation against `$param` token references inside the
  parameter query itself is best-effort; the W16 validator already
  catches dashboard-level cycles.

## Out of scope

- Repeating widgets / panels.
- Cross-dashboard global parameters.
- Multi-user parameter scopes.
- Cloud share links.
- Replacing Workbench datasource execution.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W25_DASHBOARD_PARAMETERS.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`
