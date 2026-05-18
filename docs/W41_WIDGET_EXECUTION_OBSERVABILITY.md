# W41 Widget Execution Observability And LLM Provenance

Status: shipped (2026-05-17)

Date: 2026-05-17

## Context

When looking at a widget, the user needs to know where it gets data, which tool
or provider it calls, what arguments were used, how the pipeline transformed
the result, and whether an LLM is part of the widget path. Existing pipeline
debugging is useful for step-level data loss, but widget details also need a
product-facing provenance view.

This task turns "what does this widget call?" and "does this widget use LLM?"
into first-class visible facts.

## Goal

- Every widget detail view shows its datasource/workflow/provider/tool
  provenance.
- Widget details answer whether an LLM/provider step participates in the
  widget runtime path.
- Tool/provider call names, target identities, timing, status, and redacted
  arguments are visible.
- Users can jump from a widget to the relevant Workbench datasource,
  Operations workflow, or Pipeline Debug trace.
- Secret values are never exposed to React, logs, traces, or copied JSON.
- Agent/reflection prompts can consume compact provenance summaries without
  guessing.

## Approach

1. Define a widget execution summary.
   - Add a typed summary that includes widget id/kind, datasource definition id,
     workflow id, source kind, server/tool/provider/model identity, latest run
     status, last success/error timestamps, duration, and LLM participation.
   - Represent LLM participation explicitly as `none`, `provider_source`,
     `llm_postprocess`, `widget_text_generation`, or another concrete enum
     rather than inferring from strings in the UI.

2. Capture source and call metadata.
   - Reuse W23 traces and W35 workflow run records where possible.
   - For MCP/tool calls, show server id, tool name, redacted argument preview,
     duration, output shape, and error class.
   - For provider calls, show provider id, model id, capability flags,
     streaming support if known, token/cost summary when W22 data is available,
     and final status.

3. Build widget details UI.
   - Add a details/inspector panel or modal from the widget controls.
   - Show datasource, pipeline, latest run, LLM badge, and links to Workbench,
     Operations, and Pipeline Debug.
   - Keep compact labels in the widget chrome; put detailed JSON/trace views in
     expandable inspector sections.

4. Enforce redaction and provenance honesty.
   - Redact headers, tokens, passwords, API keys, env-derived values, and
     provider secrets at the Rust boundary.
   - Unsupported or unavailable provenance must render as an explicit
     `unknown`/`not captured` state, not as empty success.
   - Do not store full prompt/tool payloads in widget config unless a separate
     approved retention policy exists.

5. Feed agent reflection.
   - Provide a compact provenance summary for Build Chat/reflection so the LLM
     can explain or repair a widget based on real source/call data.
   - Include whether the widget is deterministic pipeline-only or LLM-backed.

## Files

- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/provider.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/tool_engine.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*`
- `src/components/debug/PipelineDebugModal.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - MCP tool provenance summary,
  - provider/LLM provenance summary,
  - pipeline-only widget marked as no-LLM,
  - `llm_postprocess` or provider-backed widget marked as LLM-backed,
  - redaction of sensitive argument/header/provider values,
  - stale/missing provenance rendered as explicit unavailable state.
- Manual running-app smoke:
  - create one deterministic datasource/pipeline widget,
  - create one provider/LLM-backed widget,
  - open widget details and confirm source, calls, timings, and LLM badge are
    correct,
  - jump from details to Workbench/Operations/Pipeline Debug where available,
  - confirm no secrets appear in UI or copied debug JSON.

## Out of scope

- Full distributed tracing infrastructure.
- Persisting complete raw provider prompts or raw tool payloads indefinitely.
- Replacing W23 Pipeline Debug or W35 Operations.
- Making non-LLM widgets call LLM for explanation automatically.
- Changing provider secrets ownership.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`
