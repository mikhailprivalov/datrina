# W42 Text Widget Streaming And Reasoning State

Status: shipped v1

Date: 2026-05-17

## Context

Chat already has a streaming substrate, but widgets still behave like
refresh-only boxes: the user waits for a final value and cannot tell whether an
LLM-backed text widget is actively reasoning, calling tools, streaming text, or
stuck. Textual widgets should be able to stream partial output when their
runtime path includes a provider call, while deterministic widgets should keep
their current final-value behavior.

This task adds widget-level streaming for text-like widgets and visible LLM
reasoning state without weakening validation or committing partial data as final
runtime data.

## Goal

- Text-like widgets can display streaming provider output during refresh.
- LLM-backed widgets show an explicit reasoning/thinking/in-progress state
  while provider reasoning is active.
- Partial streamed text is visually distinct from committed final widget data.
- Final widget data is committed only after the runtime path succeeds.
- Stream failures after partial text are honest: accumulated partial content may
  remain visible as failed/partial, but it is not marked as a successful refresh.
- Non-text widgets keep final-value refresh behavior unless a later task defines
  typed streaming for them.

## Approach

1. Define widget stream event types.
   - Add Rust/Tauri events for widget refresh started, provider reasoning
     started/updated, text delta, tool/status update, final committed value,
     failed, and cancelled/superseded.
   - Include dashboard id, widget id, refresh run id, sequence number, and
     status metadata so stale events can be ignored.
   - Reuse W14/W15 provider streaming concepts where possible.

2. Add provider streaming path for text widgets.
   - Route eligible text/markdown/summary widgets through a streaming provider
     call when their source path includes LLM generation or `llm_postprocess`.
   - Preserve non-streaming fallback for providers that do not support
     streaming, but show a clear non-streaming in-progress state.
   - Keep first-byte and mid-stream timeout behavior explicit and consistent
     with chat where applicable.

3. Keep partial output separate from committed data.
   - Store partial stream state in UI runtime state, not as the persisted
     widget config or last successful snapshot.
   - Commit to `WidgetRuntimeData` and W36 snapshots only after successful final
     completion.
   - On failure, show partial content with failed state and retry affordance.

4. Show reasoning and call status in widget chrome/details.
   - Add compact in-widget indicators for LLM reasoning, tool call, streaming,
     failed, and final states.
   - Feed W41 details with the active provider/tool state.
   - Avoid verbose instructional copy inside the widget; use icons, badges, and
     tooltips.

5. Add ordering and cancellation safety.
   - Ignore stream events whose run id is older than the current widget refresh.
   - Cancelling or superseding a refresh must clear the active stream state
     without overwriting a newer final result.

## Files

- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/provider.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*`
- `src/components/debug/PipelineDebugModal.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W42_WIDGET_STREAMING_REASONING.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - widget stream event ordering by run id and sequence,
  - text delta accumulation,
  - final commit only after success,
  - failure after partial text not counted as successful refresh,
  - non-streaming provider fallback state,
  - superseded stream events ignored.
- Manual running-app smoke:
  - configure an LLM-backed text widget,
  - refresh it and confirm reasoning/streaming state appears before final text,
  - interrupt or supersede a refresh and confirm stale deltas do not overwrite
    the newer result,
  - simulate stream failure after partial text and confirm the widget is marked
    failed/partial,
  - confirm deterministic stat/table widgets are not forced into text streaming.

## Out of scope

- Streaming chart/table binary data.
- Persisting provider reasoning traces beyond the approved summary/provenance
  fields.
- Auto-applying Build Chat changes from streamed widget output.
- Replacing chat streaming implementation.
- Making providers expose hidden chain-of-thought beyond safe reasoning
  summaries/status.

## v1 shipped — 2026-05-17

- New Tauri event channel `widget:stream` with `WidgetStreamEnvelope`
  carrying `dashboard_id`, `widget_id`, `refresh_run_id`, monotonic
  `sequence`, and a typed `kind` (`refresh_started`, `reasoning_delta`,
  `text_delta`, `status`, `final`, `failed`, `superseded`).
- `WidgetStreamContext` registers a fresh `refresh_run_id` per refresh
  in `AppState::widget_refresh_runs`. Mid-flight events from a
  superseded refresh are dropped server-side; the client also drops
  deltas whose run id is older than the one it is tracking.
- `run_pipeline_with_streaming` runs the tail pipeline and routes the
  terminal `LlmPostprocess { expect: text }` step through
  `complete_chat_with_tools_streaming`, fan-outing provider deltas as
  typed pipeline stream events.
- `finalize_widget_refresh` opt-in streams when the widget is `Text`
  and the tail terminates in `llm_postprocess Text`; non-streaming
  providers (Ollama) emit a `status` hint so the UI shows a "waiting"
  badge instead of a deceptive idle widget.
- Front-end `TextWidget` paints partial text with an animated caret +
  dimmed style, surfaces an "LLM is thinking…" placeholder when only
  reasoning has arrived, and shows a destructive ring for partial text
  retained after a streamed failure. The committed runtime value still
  lands via the normal refresh return path and overwrites the partial.
- Unit coverage: `run_pipeline_with_streaming_emits_step_started_for_each_step`,
  `run_pipeline_with_streaming_records_error_and_does_not_finalise`,
  and the `widget_stream_tests` module that pins the streaming
  eligibility rule for Text widgets with terminal `LlmPostprocess Text`.

Out-of-scope for v1 (still): streaming chart/table data, persisting
reasoning summaries to `WidgetRuntimeSnapshot`, and Build Chat auto-
apply from streamed widget output.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`
- `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
