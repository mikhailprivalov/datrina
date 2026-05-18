# W32 Typed Pipeline Studio

Status: shipped (v1)

Date: 2026-05-17

## v1 shipped

- `src-tauri/src/commands/debug.rs` exposes `replay_pipeline` —
  deterministic pipeline replay against an inline sample or a stored
  W23 trace. Provider / MCP-aware steps are explicitly rejected so the
  Studio never triggers a network call or cost line item.
- `pipeline_step_kind_public` is now exported from
  `modules/workflow_engine.rs` to format Studio-side validation errors.
- TypeScript registry at `src/lib/pipeline/registry.ts` mirrors every
  `PipelineStep` variant (label, description, defaults, validator,
  advanced flag for `llm_postprocess` / `mcp_call`).
- `src/components/pipeline/PipelineStudio.tsx` +
  `src/components/pipeline/StepEditor.tsx` are the typed editor:
  add / remove / reorder / duplicate / disable / advanced JSON / replay
  with first-empty-step highlighting.
- Workbench replaces the JSON pipeline textarea with the Studio. The
  Studio's replay seeds from `lastRun.raw_source` so users can preview
  pipeline edits before saving.
- PipelineDebugModal grows an "Open in Studio" affordance that seeds
  the editor from the active trace and replays via `from_widget_trace`
  when the trace is persisted in the ring buffer, or via the trace's
  first input sample otherwise. Saving back is intentionally gated on
  the owning datasource/Workbench (no implicit persistence from Debug).
- W18 reflection prompt now includes a compact `trace_summary_for_reflection`
  block (`commands::chat::run_reflection_turn`) so the agent can anchor
  a fix-up proposal on a concrete step index instead of guessing from
  the runtime preview.
- Unit tests in `commands::debug::tests` cover first-empty-step
  detection (none, empty array, error-before-empty) and confirm
  deterministic replay matches the live pipeline runner.

## Residuals deferred to v2

- Saving from PipelineDebugModal back to the owning binding without
  jumping to Workbench.
- Reflection / Build Chat emitting a step-level proposal diff rather
  than a full replacement pipeline.
- Replay against a re-fetched live source value (today the trace's
  recorded first input is used; for stale traces the user runs the
  W23 "Run with trace" button before reopening the Studio).
- Drag-and-drop reordering (today the Studio uses up / down buttons).

## Context

W23 made pipeline traces inspectable and W30 exposed datasource pipelines in the
Workbench, but editing remains a JSON textarea. That is honest, but it keeps
normal dashboard operations too close to implementation details. Operators need
to adjust `pick`, `filter`, `sort`, `aggregate`, `map`, `format`, `coerce`, and
similar deterministic steps without hand-writing JSON.

The Pipeline Studio turns the existing DSL into a structured edit/test/save
surface. It must not add a new query language or execute arbitrary code.

## Goal

- Workbench and Pipeline Debug expose a typed step editor for supported
  `PipelineStep` variants.
- Users can add, remove, reorder, duplicate, disable, and edit pipeline steps.
- Each step has a form matched to its variant and validates before save.
- Users can replay a pipeline against a saved sample or W23 trace without
  hitting the live datasource.
- Users can compare before/after output and identify the first step that makes
  data empty or invalid.
- JSON editing remains available only as an advanced escape hatch with explicit
  validation.
- Build Chat and reflection prompts can consume the same trace/replay result
  shape.

## Approach

1. Define editor metadata for the pipeline DSL.
   - Build a TypeScript registry for each supported `PipelineStep` kind:
     label, description, required fields, form controls, example, and validator.
   - Mirror Rust validation for persistable step shapes where possible.
   - Keep `llm_postprocess` visibly advanced and last-resort.

2. Add replay commands.
   - Replay a candidate pipeline against a provided sample value.
   - Replay a candidate pipeline against the source output from a stored W23
     trace.
   - Return per-step samples, errors, duration, and first-empty-step hints.

3. Upgrade Workbench editing.
   - Replace the default pipeline textarea with typed rows.
   - Add step reorder, duplicate, delete, and temporary disable controls.
   - Show before/after output and validation errors before saving.
   - Preserve the JSON editor under an explicit advanced panel.

4. Upgrade Pipeline Debug.
   - Add "Edit from trace" and "Try fix" flows that open the same Studio editor
     with trace source data.
   - Do not persist changes until the user explicitly saves through the owning
     datasource/widget binding.

5. Connect agent feedback.
   - Reflection should include compact replay diagnostics when suggesting a
     pipeline fix.
   - Build Chat should be able to propose a pipeline diff with step-level
     labels, not only a replacement JSON array.

## Files

- `src-tauri/src/models/pipeline.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/debug.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/chat.rs` only for reflection/prompt integration
- `src/lib/api.ts`
- new Studio components under `src/components/pipeline/`
- `src/components/datasource/Workbench.tsx`
- `src/components/debug/PipelineDebugModal.tsx`
- `src/lib/chat/runtime.ts` only if new diagnostic parts are added
- `docs/RECONCILIATION_PLAN.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`

## Validation

- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021`
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - TypeScript step-form serialization for every supported step kind,
  - Rust replay command returning the same final value as live pipeline
    execution on the same sample,
  - first-empty-step detection,
  - invalid step validation,
  - JSON advanced editor round-trip.
- Manual smoke:
  - open a datasource with a non-trivial pipeline,
  - add a filter and sort through typed controls,
  - replay against the last sample,
  - intentionally break a field path and confirm the failing step is named,
  - fix and save,
  - refresh bound widgets and confirm output changed as expected,
  - open W23 Debug and start an edit from a trace.

## Out of scope

- A new data query language.
- Arbitrary JavaScript execution.
- Full visual workflow graph editor.
- Provider-driven automatic fixes without user confirmation.
- Replacing the existing deterministic `PipelineStep` enum.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W18_PLAN_EXECUTE_REFLECT.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
