# W25 Dashboard Parameters (Grafana-style Variables)

Status: shipped (v1)

Date: 2026-05-17

## v1 shipped surface

- `DashboardParameter` / `DashboardParameterKind` / `ParameterValue` /
  `ParameterOption` types extend `Dashboard` and `BuildProposal`.
- `modules::parameter_engine` resolves a topo-ordered map of parameter
  values and substitutes `$name` / `${name}` tokens in workflow node
  configs. Whole-string tokens preserve JSON type; multi-value params
  support `comma_join` / `json_array` / `first` render modes. TimeRange
  params expose `.from` / `.to` / `.duration_ms` accessors.
- Storage: `dashboards.parameters` JSON column + `dashboard_parameter_values`
  table.
- Tauri commands: `list_dashboard_parameters`,
  `get_dashboard_parameter_values`, `set_dashboard_parameter_value`
  (returns affected widget ids by scanning workflow configs for the
  changed name), `resolve_dashboard_parameters`.
- `refresh_widget`, `dry_run_widget` (UI + agent path), and the chat
  agent's `execute_dry_run_widget` substitute parameters before the
  workflow runs.
- W16 validator: `UnknownParameterReference` + `ParameterCycle`.
- UI: `ParameterBar` sticky controls row above `DashboardGrid` with
  Select / MultiSelect / TextInput / TimeRangePicker / IntervalPicker
  building blocks; commits trigger refresh of affected widgets.
- Build chat system prompt explains the parameter contract +
  `dry_run_widget` tool spec exposes `parameters` /
  `parameter_values`.

## v1 deferrals (intentional)

- `mcp_query` / `http_query` option-list resolution: the parameter
  declaration round-trips, but the dropdown is rendered as a plain text
  input in v1 (UI shows the persisted selection). Wire option fetch in
  v2.
- `builtin` parameter source kind.
- URL-hash sync for shareable parameter state across reloads.
- Repeating widgets (Grafana `repeat`) — explicit out-of-scope below.

## Context

Today every dashboard is hard-coded. A typical Build chat ends with a
proposal whose `datasource_plan.arguments` already contain a literal
project / environment / region key. To look at a different value the
user has to either ask the agent to rebuild the dashboard or hand-edit
JSON. Real monitoring tools — Grafana, Datadog, Honeycomb — solve this
with **dashboard variables**: a row of dropdowns / inputs / time pickers
at the top of the dashboard that flow into every widget's query.

Datrina has the pieces to support the same pattern (typed pipeline DSL,
MCP/HTTP tool calls, shared datasources, reactive refresh through the
workflow engine), it just has no parameter layer.

## Goal

- Dashboards own a typed list of **parameters** (Grafana's "template
  variables").
- Parameters appear as a controls row at the top of the dashboard:
  dropdowns, text inputs, time-range picker, interval picker.
- Widget configs reference `$param` / `${param}` in
  `datasource_plan.arguments`, MCP `arguments`, HTTP query strings, and
  pipeline step configs. The runtime substitutes before execution.
- Changing a parameter value re-executes **only the widgets that
  reference it**, and persists the selection so a reload comes back to
  the same state.
- The Build Chat agent can propose parameters as part of a
  `BuildProposal` and reference them in the widgets it emits.

## Approach

### Data model (`src-tauri/src/models/dashboard.rs`)

Extend `Dashboard`:

```rust
pub struct Dashboard {
    /* existing fields ... */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<DashboardParameter>,
}

pub struct DashboardParameter {
    pub id: String,                       // stable id (slug)
    pub name: String,                     // referenced as `$name` in configs
    pub label: String,                    // user-facing label
    pub kind: DashboardParameterKind,
    #[serde(default)]
    pub multi: bool,                      // multi-select
    #[serde(default)]
    pub include_all: bool,                // adds an "All" sentinel option
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<ParameterValue>,
    #[serde(default)]
    pub depends_on: Vec<String>,          // other param names; triggers re-resolve when they change
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashboardParameterKind {
    /// Fixed options the user types in.
    StaticList { options: Vec<ParameterOption> },
    /// Free-form string input.
    TextInput { placeholder: Option<String> },
    /// `from` / `to` timestamps with presets (last 5m, 1h, 24h, custom).
    TimeRange { default_preset: Option<String> },
    /// Single duration picker.
    Interval { presets: Vec<String> },     // ["1m","5m","1h","6h","1d"]
    /// Query an MCP tool; pipeline reduces the result to a list of options.
    McpQuery {
        server_id: String,
        tool_name: String,
        arguments: Option<Value>,          // may contain `$other_param`
        #[serde(default)]
        pipeline: Vec<PipelineStep>,       // last step must produce array of {label, value}
    },
    /// Same shape but HTTP backend.
    HttpQuery {
        method: String,
        url: String,
        headers: Option<Value>,
        body: Option<Value>,
        #[serde(default)]
        pipeline: Vec<PipelineStep>,
    },
    /// Built-in special: list of currently-enabled MCP servers, or list of
    /// dashboards, etc. — names registered in `resolve_builtin_parameter`.
    Builtin { source: String },
    /// Constant scalar — useful for environment switching where the same
    /// value is referenced many times but never user-edited.
    Constant { value: ParameterValue },
}

#[serde(untagged)]
pub enum ParameterValue {
    String(String),
    Number(f64),
    Bool(bool),
    Array(Vec<ParameterValue>),
    TimeRange { from: i64, to: i64 },
}

pub struct ParameterOption {
    pub label: String,
    pub value: ParameterValue,
}
```

Per-user selections live in a new table:

```sql
CREATE TABLE dashboard_parameter_values (
  dashboard_id TEXT NOT NULL,
  param_name TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (dashboard_id, param_name)
);
```

### Substitution engine (`src-tauri/src/modules/parameter_engine.rs`, new)

Public surface:

```rust
pub struct ResolvedParameters(BTreeMap<String, ParameterValue>);

impl ResolvedParameters {
    pub async fn resolve_all(
        params: &[DashboardParameter],
        selected: &BTreeMap<String, ParameterValue>,
        ctx: &ResolveContext<'_>,                  // mcp_manager, ai_engine, tool_engine, storage
    ) -> Result<Self>;

    pub fn substitute_value(&self, value: &Value, options: SubstituteOptions) -> Value;
    pub fn substitute_string(&self, raw: &str) -> String;
    pub fn referenced_names(value: &Value) -> BTreeSet<String>;
}

pub struct SubstituteOptions {
    /// How to render a multi-select parameter when it is referenced in
    /// a scalar context: comma-join, JSON array, or first-only.
    pub multi_render: MultiRender,
}
```

`substitute_value` walks the JSON tree:

- `Value::String("...$foo...")` → replaces `$foo` and `${foo}` tokens
  with the parameter's resolved value (string-coerced for arrays via
  `multi_render`).
- `Value::String("$foo")` exactly (the whole string is one token):
  preserves the original type. For example, a numeric param substituted
  into `arguments.count` stays a `Value::Number`, not a stringified
  number. This is the same behavior Grafana exposes with `${var:raw}`.
- `Value::Array` / `Value::Object`: recursive descent.

Cyclic-dependency detection: `resolve_all` does a topological pass over
`depends_on` and fails with `ParameterCycle { cycle: Vec<String> }` if
the graph is not a DAG.

Time-range and interval parameters expose useful helpers — e.g.
`$range.from`, `$range.to`, `$range.duration_ms`, `$step` — surfaced as
extra slots in `ResolvedParameters`.

### Runtime integration

Three call sites use the substitution:

1. **`workflow_engine::execute_workflow`** — load
   `ResolvedParameters` for the workflow's owning dashboard before
   executing each node; substitute `node.config` and node-kind-specific
   arg fields. For widgets without a dashboard (one-off chat dry runs),
   pass an empty `ResolvedParameters` and let `$param` tokens fall
   through as a render warning (`MissingParameter` in W16's validator).
2. **`refresh_widget` command (`commands/dashboard.rs`)** — same path,
   re-resolves on every refresh so newly-selected param values flow in.
3. **`commands/dashboard::dry_run_widget`** — accepts an optional
   `parameter_values: Option<BTreeMap<String, ParameterValue>>` so the
   agent can dry-run with concrete values during proposal-time.

### Refresh semantics

When the user changes a parameter value (UI control commits):

1. `set_dashboard_parameter_value(dashboard_id, param_name, value)`
   persists the value, returns the affected widget IDs (computed by
   walking each widget for `$param` references via
   `ResolvedParameters::referenced_names`).
2. UI calls `refresh_widget(widget_id)` for each affected widget. The
   scheduler does the same on its next tick automatically.

For `depends_on` cascades: changing `env` also re-resolves `service`
(which has `depends_on: ["env"]`). Affected widget set is the union.

### UI

`src/components/dashboard/ParameterBar.tsx` (new):

- Renders one control per parameter, in declared order, in a sticky row
  above `DashboardGrid`.
- Control map:
  - `static_list` (single) → `Select`.
  - `static_list` (multi) → `MultiSelect` with chips.
  - `text_input` → `Input` with debounced commit (500 ms).
  - `time_range` → `TimeRangePicker` (presets + custom).
  - `interval` → `Select` of presets.
  - `mcp_query` / `http_query` → `Select` populated from the resolved
    options. Re-resolves on dependency change with a small spinner.
  - `builtin` → `Select` populated by the matching backend command.
- "Apply" button (or commit on change) → calls
  `set_dashboard_parameter_value` then queues widget refreshes.

State syncs to URL hash (`#params=base64-encoded-json`) for shareable
links.

### Build Chat agent integration

`BuildProposal` already supports `widgets`, `shared_datasources`, etc.
Extend with:

```rust
pub struct BuildProposal {
    /* existing ... */
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<DashboardParameter>,
}
```

`apply_build_proposal_inner` writes parameters into
`dashboard.parameters` (merging with existing — `id` collision means
replace, otherwise append).

System-prompt guidance (`commands/chat.rs` build prompt):

- "If the user's request implies switching between several values
  (project names, environments, time ranges), declare a
  `parameters` entry and reference it as `$paramname` in your
  widget arguments instead of hardcoding."
- Concrete recipe examples (mcp_query parameter producing a project
  list, then a stat widget referencing `$project`).

### W16 validator additions

New `ValidationIssue` variants surfaced by the W16 gate:

- `UnknownParameterReference { widget_index, widget_title, param_name }`:
  widget references `$foo` but `foo` is not declared in
  `proposal.parameters` or the existing dashboard's parameters.
- `ParameterCycle { cycle: Vec<String> }`: `depends_on` declares a
  cycle.
- `EmptyOptionList { param_name }`: a query-backed parameter pipeline
  returned zero options during dry-run (warning, not blocker).

### Backwards compatibility

Existing dashboards without `parameters` continue to work — the
substitution engine is a no-op on configs with no `$` tokens. The
runtime simply skips loading values for dashboards with an empty
parameter list.

## Files to touch

- `src-tauri/src/models/dashboard.rs` — `DashboardParameter`,
  `DashboardParameterKind`, `ParameterValue`, `ParameterOption`; extend
  `Dashboard` and `BuildProposal`.
- `src-tauri/src/modules/parameter_engine.rs` (new) — resolution,
  substitution, cycle detection.
- `src-tauri/src/modules/workflow_engine.rs` — substitute before each
  node execution.
- `src-tauri/src/commands/dashboard.rs` — `list_dashboard_parameters`,
  `get_dashboard_parameter_values`, `set_dashboard_parameter_value`,
  `resolve_dashboard_parameters` Tauri commands; integrate into
  `refresh_widget` and `dry_run_widget`.
- `src-tauri/src/modules/storage.rs` — `dashboard_parameter_values`
  migration; CRUD; persist `parameters` JSON column on dashboards.
- `src-tauri/src/commands/validation.rs` — new validator variants from
  W16 family.
- `src-tauri/src/commands/chat.rs` — extend Build system prompt; pass
  parameter values into the agent's `dry_run_widget` invocations.
- `src/lib/api.ts` — mirror types + `dashboardApi.setParameterValue`,
  `resolveParameters`.
- `src/components/dashboard/ParameterBar.tsx` (new) — top-of-dashboard
  controls.
- `src/components/dashboard/ParameterControls/*.tsx` (new) — Select,
  MultiSelect, TimeRangePicker, IntervalPicker building blocks.
- `src/components/layout/DashboardGrid.tsx` — render `ParameterBar`
  above the grid; pipe value changes through.
- `docs/RECONCILIATION_PLAN.md` — note parameter layer addition.

## Validation

- `bun run check:contract`, `bun run typecheck`, `bun run build`.
- `cargo fmt --all --check`, `cargo check --workspace --all-targets`.
- Unit (`tests/parameter_engine.rs`):
  - Substitution preserves type when whole-string token: number, bool,
    array.
  - Multi-select `multi_render: comma_join` produces
    `"a,b,c"`; `json_array` produces `["a","b","c"]`.
  - Cycle detection on `a → b → a`.
  - Cascading resolve: `service` depends on `env`; changing `env`
    re-resolves `service`.
- Manual: create a dashboard, add a `project` static_list parameter
  with three options; reference `$project` inside an MCP arguments
  block; switch the dropdown — confirm only that widget refreshes and
  the new value is in the MCP call payload (visible in chat trace if
  the same source is reused).
- Manual: ask the agent "build me a release-status dashboard for any of
  these three projects: A, B, C" → expect a proposal with a `project`
  parameter and a stat widget referencing `$project`.
- Manual: cycle test — submit a malformed proposal with `a depends_on
  b`, `b depends_on a` → W16 validator fires `ParameterCycle`.

## Out of scope

- Repeating widgets: one widget instance per parameter value (Grafana
  `repeat`). Useful but doubles the layout complexity; defer.
- Cross-dashboard parameter linking / global parameters.
- Per-user vs per-dashboard parameter scopes — single-user local-first,
  values are dashboard-scoped only.
- URL-based parameter sharing across machines (URL-hash sync works
  within one Datrina instance only).
- Time-range parameter as a global control affecting all queries
  automatically — caller widgets must opt in by referencing
  `$range.from` / `$range.to`. Auto-apply is a follow-up.

## Related

- **W12** Build proposal apply — `parameters` arrives via the same
  `BuildProposal` shape.
- **W13** Durable runtime pipeline — substitution sits in front of every
  pipeline execution.
- **W16** Proposal validation gate — new `ValidationIssue` variants land
  here; the gate verifies parameter references resolve.
- **W17** Agent memory — once shipped, the agent can remember per-user
  preferred parameter defaults across sessions (e.g., "this user always
  picks env=prod").
- **W18** Plan / Execute / Reflect — the plan can include a "declare
  parameters" step explicitly; reflection can spot rendered widgets
  whose values look constant and suggest extracting a parameter.
- **W19** Dashboard versions — versions snapshot
  `parameters` and `parameter_values` so restore brings back the same
  configurable state.
- **W20** Data Playground — Playground can let the user define a
  parameter while exploring, and "Use as widget" carries it through.
- **W23** Pipeline debug view — trace samples should show the
  substituted-in arg values, not the raw `$param` tokens, so debugging
  matches the actual execution.
