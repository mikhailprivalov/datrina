# W48 Build Chat Source Mentions And Multi-Source Composition

Status: shipped

Date: 2026-05-17 (planned) / 2026-05-18 (shipped)

## Context

W38 adds stable widget mentions, and W39 materializes Build-created sources into
saved datasources. That still leaves a product gap for dashboards that need a
new widget composed from more than one existing source.

The weather dashboard exposed the gap clearly: forecast widgets could share one
Open-Meteo forecast workflow, air-quality became a separate workflow, and the
text "weather news" widget could not consume both the forecast and air-quality
sources. The assistant worked around the limitation by telling the user to look
at the neighboring air-quality widget instead of actually composing the two
inputs.

Build Chat needs typed source targeting, not only widget targeting. The user
should be able to say "use this forecast source and this air-quality source in
one text/table/chart widget" and have the backend pass stable datasource or
workflow identities, compact sample shapes, join keys, freshness, and tail
pipeline context to the model.

## Goal

- Build Chat can mention one or more datasources, workflows, or source-backed
  widgets as explicit input sources for a new or replaced widget.
- Source mentions are stable ids, not title-only text or layout-position
  guesses.
- The sent Build turn carries typed source context: datasource definition id
  when available, workflow id for legacy rows, source kind, redacted arguments,
  output shape, recent sample/trace summary, freshness, and consumer count.
- Build proposals can represent multi-input widget runtime plans rather than
  only one `datasource_plan` per widget.
- The apply path creates or reuses saved `DatasourceDefinition` rows where
  possible and preserves per-widget tail pipelines.
- Validation rejects fake multi-source success, missing input mappings, unsafe
  joins, and proposals that silently ignore a mentioned source.
- The weather-style scenario is covered by replay or acceptance fixtures:
  forecast source + air-quality source -> one narrative text widget that uses
  both inputs.

## Approach

1. Define source mention models.
   - Add a `SourceMention` or equivalent typed model with `dashboard_id`,
     `datasource_definition_id`, optional legacy `workflow_id`, optional
     `widget_id`, display label, source kind, and compact runtime summary.
   - Keep labels presentation-only. The ids and resolved source summaries are
     the source of truth.
   - Resolve mentioned sources against the latest dashboard/workbench state
     before invoking Rust chat logic.

2. Extend Build Chat UX.
   - Add a source/datasource mention picker in Build mode, separate from W38
     widget mentions or sharing the same picker with clear tabs.
   - Show enough context to choose correctly: source name, owning widget(s),
     kind, freshness/error state, and short output-shape preview.
   - If a source is legacy workflow-only, show it as upgradeable but still
     usable.

3. Build compact multi-source prompt context.
   - Include only shape summaries, small pruned samples, provenance, and
     redacted source arguments.
   - Do not paste full raw tool results or full workflow run payloads into the
     provider context.
   - Include explicit instructions that every mentioned source must be used or
     the model must return an unsupported/remediation answer.

4. Extend proposal/runtime shape for multi-input widgets.
   - Add a multi-input datasource plan or binding model that can reference
     multiple existing datasource definitions/workflows.
   - Define deterministic input aliases such as `forecast` and `air_quality`
     so pipeline steps can join or compose without relying on array order alone.
   - Keep the existing single-source path as the simple default.
   - For materialized sources, bind each base source by
     `datasource_definition_id` and store widget-specific composition/tail
     pipeline separately.

5. Add validation and apply semantics.
   - Validate that every mentioned source is represented in the proposal.
   - Validate join/composition mappings: key fields, cardinality assumptions,
     missing-value behavior, and output shape.
   - Block proposals that claim a multi-source narrative/table/chart while
     reading only one input.
   - Preserve explicit preview/apply confirmation and W29 no-fake-success
     behavior.

6. Add debug/provenance integration.
   - W41 widget details should show all source inputs, not just one workflow id.
   - W23/Pipeline Debug should show per-input samples and each composition step.
   - Workbench consumers should account for multi-source widgets.

7. Add scenario coverage.
   - Recorded eval: Build a five-city weather dashboard, add air quality, then
     add a text/news widget that must use both forecast and air-quality sources.
   - Assertions: the final widget references both inputs, dry-run output
     contains weather and air-quality facts, no hardcoded values, and no
     "see neighboring widget" workaround.

## Files

- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src/lib/api.ts`
- `src/components/layout/ChatPanel.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/debug/PipelineDebugModal.tsx`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/**`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W48_BUILD_CHAT_SOURCE_MENTIONS_AND_MULTI_SOURCE_COMPOSITION.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- `bun run eval`
- Unit or integration checks for:
  - source mention serialization across TypeScript and Rust,
  - stale/deleted source mention returning a typed error,
  - legacy workflow source mention resolving without data loss,
  - multi-input proposal validation requiring every mentioned source,
  - apply preserving multiple input bindings and per-widget composition pipeline,
  - Workbench consumer lookup for multi-source widgets,
  - redaction of arguments/headers/secrets in source summaries.
- Manual running-app smoke:
  - create or open a weather dashboard with separate forecast and air-quality
    sources,
  - mention both sources in Build Chat,
  - ask for a narrative text widget combining current weather and air quality,
  - confirm preview shows both source inputs,
  - apply and confirm the rendered widget uses facts from both sources,
  - open debug/provenance surfaces and confirm both inputs are visible.

## Out of scope

- Cross-dashboard source composition unless a later task explicitly adds it.
- Auto-applying Build Chat proposals without preview.
- Replacing W38 widget mentions.
- Replacing W39 datasource materialization.
- Arbitrary SQL/JavaScript joins.
- Storing provider/API secrets in React state, prompts, or widget JSON.
- Making multi-source composition a reason to bypass validation or dry-run
  evidence.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
