# W23 Pipeline Debug View

Status: planned

Date: 2026-05-16

## Context

When a widget renders `0` or `null`, there is no way to see where in
the pipeline the data was lost. The user has to ask the agent ("why is
this empty?"), and the agent has no observability into the pipeline
either — only the final value. Debugging a widget today is guessing.

This is also a hard blocker for W18's reflection turn: the agent needs
to see per-step traces to make useful suggestions.

## Goal

Every widget has a one-click **Debug pipeline** view that shows each
pipeline step with its input sample, output sample, duration, and
optional error. Same trace is available programmatically so the agent
can consume it during reflection.

## Approach

### Trace data structure

```rust
pub struct PipelineTrace {
    pub workflow_id: String,
    pub widget_id: WidgetId,
    pub started_at: i64,
    pub finished_at: i64,
    pub source_summary: SourceSummary,     // mcp_tool name+args, http url+method, etc.
    pub steps: Vec<PipelineStepTrace>,
    pub final_value: Option<serde_json::Value>,   // truncated to 8 KB
    pub error: Option<String>,
}

pub struct PipelineStepTrace {
    pub index: u32,
    pub kind: String,                       // PipelineStep variant tag
    pub config_json: serde_json::Value,     // the step config as configured
    pub input_sample: SampleValue,
    pub output_sample: SampleValue,
    pub duration_ms: u32,
    pub error: Option<String>,
}

pub struct SampleValue {
    pub kind: SampleKind,                   // 'value' | 'array_head' | 'object' | 'null' | 'truncated_string'
    pub size_hint: SizeHint,                // { items: Option<usize>, bytes: Option<usize> }
    pub preview: serde_json::Value,         // pruned: max 5 array items at any depth, max 4 KB
}
```

### Executor instrumentation (`modules/workflow_engine.rs`)

Add a parallel API alongside `run_pipeline`:

```rust
pub async fn run_pipeline_with_trace(
    steps: &[PipelineStep],
    initial: serde_json::Value,
    ctx: &PipelineContext,
) -> Result<(serde_json::Value, Vec<PipelineStepTrace>)>;
```

Implementation wraps each `apply_pipeline_step` call with timing +
sample capture (before / after, pruned by `prune_for_trace(v: &Value)`
which truncates strings >256 chars, arrays >5 items, depth >5). Same
function used by `mcp_tool` to capture source.

`run_pipeline` keeps its current zero-overhead signature; debug uses the
new function explicitly so production refreshes are unaffected.

### Per-widget trace storage

Optionally persisted on every successful refresh in a new table:

```sql
CREATE TABLE widget_traces (
  widget_id TEXT NOT NULL,
  captured_at INTEGER NOT NULL,
  trace_json TEXT NOT NULL,
  PRIMARY KEY (widget_id, captured_at)
);
CREATE INDEX widget_traces_widget_idx ON widget_traces (widget_id, captured_at DESC);
```

Ring-buffer 5 traces per widget. Disabled by default for performance;
enabled via a per-widget toggle `debug.capture_traces = true`. The
Debug view auto-enables capture when first opened on a widget, so
clicking it once and refreshing makes the next trace visible.

### Tauri commands

- `trace_widget_pipeline(widget_id)` — runs a refresh and returns the
  trace inline (no persistence). Triggered by the "Run with trace" button
  in the Debug view.
- `list_widget_traces(widget_id)` — returns persisted traces.
- `get_widget_trace(widget_id, captured_at)` — full row.

### UI

#### Entry point

Widget kebab → "Debug pipeline" → opens a modal sized roughly two-thirds
the screen.

#### Layout

```
┌──────────────────────────────────────────────────────────────┐
│ Widget: Active Release  ·  Last trace 2s ago  ·  [Run with trace] │
├──────────────────────────────────────────────────────────────┤
│ Source: mcp_tool <server_id>.<tool_name>(<arg>=…)            │
│ ▾ Source result preview (collapsed)                          │
├──────────────────────────────────────────────────────────────┤
│ Step 1: pluck { path: "data.items" }            ✓ 1.4 ms     │
│   in: { content: [{ text: "..." }] }                         │
│   out: [{ id: "...", name: "...", ... }, … 9 more]           │
├──────────────────────────────────────────────────────────────┤
│ Step 2: filter { path: "status", op: "eq", value: "active" } │
│                                                  ✓ 0.3 ms    │
│   out: [{ id: "...", name: "..." }] (1 of 10)                │
├──────────────────────────────────────────────────────────────┤
│ Step 3: aggregate { group_by: "project", count: true }       │
│                                                  ✗ 12 ms     │
│   error: path 'project' not found in any item                │
└──────────────────────────────────────────────────────────────┘
Final value: <null>
```

Each step row collapses/expands for full JSON inspection. Errors are
red. Successful step with empty output array is yellow with a hint
"output is empty after this step — likely the data loss point".

#### Heuristic hint

Compute the **first step where the output became trivially empty**
(empty array, null, or zero on numeric path). Display it as a banner:

```
⚠ Data became empty at step 3 (filter). Suggested cause: 'status' field is
absent from the upstream tool response.
```

This is the single most useful piece of information for the user.

### Agent integration (for W18 reflection)

`enqueue_reflection_turn` (W18) calls `list_widget_traces` and includes
a compact textual summary of the latest trace per widget in the
reflection prompt. The agent then sees exactly which step lost the data
and can propose a precise pipeline fix-up instead of guessing.

## Files to touch

- `src-tauri/src/models/pipeline.rs` — `PipelineTrace`, `PipelineStepTrace`,
  `SampleValue`, etc.
- `src-tauri/src/modules/workflow_engine.rs` — `run_pipeline_with_trace`,
  `prune_for_trace`.
- `src-tauri/src/modules/storage.rs` — `widget_traces` migration; CRUD.
- `src-tauri/src/commands/dashboard.rs` — `trace_widget_pipeline`,
  `list_widget_traces`, `get_widget_trace`; hook into `refresh_widget`
  when `debug.capture_traces` is true.
- `src-tauri/src/models/widget.rs` — `debug.capture_traces` field on
  widget config.
- `src-tauri/src/lib.rs` — register commands.
- `src/lib/api.ts` — mirror types; `dashboardApi.traceWidget`, etc.
- `src/components/debug/PipelineDebugModal.tsx` (new).
- `src/components/debug/StepRow.tsx` (new).
- `src/components/layout/DashboardGrid.tsx` — kebab → Debug action.

## Validation

- `bun run check:contract`, `cargo check --workspace --all-targets`.
- Unit: `prune_for_trace` deterministically truncates large structures.
- Manual: build a widget with an intentionally bad filter path; open
  Debug → confirm the failing step is highlighted, and the heuristic
  banner names step 3.
- Manual: enable capture, run the widget twice, open Debug → see two
  traces in the list, ring-buffer caps at 5.
- Performance: refresh latency with trace capture disabled — within
  noise of pre-W23 baseline; with capture enabled — overhead < 5 ms per
  pipeline step on typical payloads.

## Out of scope

- Per-row data lineage (we trace samples, not full data).
- Pipeline editing from the Debug view (read-only inspector in v1).
- Replay traces against modified pipeline configs (could be a "what-if"
  follow-up).

## Related

- W18 — primary consumer of the trace for reflection-turn quality.
- W17 — `mcp_tool_observed_shape` can be enriched with hints derived
  from trace failure points.
- W24 — eval assertions can use trace contents as another check surface.
