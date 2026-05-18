# W51 Provider Context Compression And Raw Artifact Retention

Status: shipped

Date: 2026-05-17

Shipped: 2026-05-18

## Outcome

- `modules/context_compressor.rs` is the single Rust-owned compression
  boundary. Eight typed profiles (`chat_tool_result`, `mcp_tool_result`,
  `http_response`, `external_source_result`, `datasource_sample`,
  `pipeline_trace`, `provider_intermediate`, `error_or_failure`) produce
  a `CompressedArtifact { compact, preserved_facts, truncation,
  raw_bytes, compact_bytes, raw_artifact_ref, estimated_tokens_saved }`.
- `Storage::raw_artifacts` (SQLite) holds the redacted raw payload per
  `(owner_kind, owner_id)` with retention class
  (`session`/`ephemeral`/`audit`). `Storage::prune_raw_artifacts` caps
  ephemeral rows at 50 per session.
- Every chat tool result flows through
  `commands::chat::maybe_compress_tool_result` before the provider sees
  it. The persisted `ToolResult` carries typed compression metadata
  (`profile`, `raw_bytes`, `compact_bytes`, `estimated_tokens_saved`,
  `raw_artifact_id`, `truncation_paths`).
- `inspect_artifact` tool joins the chat catalog (`AIToolSpec`) and is
  dispatched inside the same Rust loop so it inherits
  `MAX_TOOL_ITERATIONS`, loop-detection, cost budget, and the
  policy/redaction boundary. Returns bounded slices (JSON pointer + row
  window + byte cap), fails closed on unknown/expired/cross-session.
- `commands::ai::tool_result_for_provider` runs the compressor on
  legacy callsites that haven't attached metadata yet, so the provider
  never receives an unbounded blob from this function.
- `ChatMessagePart::ToolResult`, `AgentEvent::ToolResult`, and
  `ToolResultTrace` carry typed `compression` so the chat UI shows
  `-N%` chip + expanded detail block with `view raw locally` (powered
  by `debug::get_raw_artifact`).
- Redaction is applied at two layers (key-based and free-text
  bearer-token heuristic) before either the provider or any local
  preview sees the payload.

## Validation evidence

- `cargo test` — 216 passed (4 suites), incl. 9 compressor unit tests
  + 6 agent_eval scenarios:
  - `compression_eval_large_http_payload_meets_reduction_target`
    (≥85% reduction, preserves status, redacts Authorization)
  - `compression_eval_mcp_envelope_meets_reduction_target`
    (≥90% reduction, envelope fact preserved)
  - `compression_eval_pipeline_trace_preserves_first_empty_step`
    (≥60% reduction, first-empty + step-count survive)
  - `compression_eval_error_profile_keeps_error_verbatim`
    (error message survives 20 KB of chaff)
  - `compression_eval_median_and_p90_reductions_meet_rtk_class_targets`
    (median ≥60%, p90 ≥90% across 5 representative fixtures)
  - `compression_eval_log_with_failure_keeps_failure_visible`
    (≥90% reduction, single failure line preserved)
- `bun run acceptance` — 6/6 gates pass after the wire-up.

## Context

W49 adds the first provider-context economy layer for chat history, tool
results, and cost accounting. That is necessary but not sufficient for the
runtime effect we want: Datrina should get RTK-class context reduction for
large tool, MCP, datasource, pipeline, and provider-intermediate outputs
without degrading answer quality, validation quality, or local debuggability.

The runtime problem is not only "too many tokens". It is that raw outputs and
provider-visible context are currently the same thing too often. Large JSON
payloads, logs, HTTP responses, MCP envelopes, test-like output, and pipeline
traces should stay available locally, while the model normally receives a
compact, typed, high-signal representation.

This workstream adds a loss-aware compression layer under the Rust runtime. It
is inspired by RTK-style command-output compression, but it must remain a
Datrina-native module, not a shell-hook dependency or second execution engine.

## Goal

- Provider-visible runtime context gets RTK-class reduction on representative
  Datrina workloads: 60-90% fewer provider-visible bytes/tokens for bulky
  tool/datasource/log/table/trace outputs, measured by fixtures and recorded
  evals.
- Compression is quality-preserving by contract:
  - errors, failed assertions, validation issues, policy denials, status codes,
    schema shape, counts, units, timestamps, and provenance survive compaction,
  - truncation is explicit and machine-readable,
  - the model can request bounded additional detail when the compact summary is
    insufficient.
- Raw outputs are retained locally under an explicit retention policy and are
  linkable from chat traces, widget provenance, Pipeline Debug, Workbench, and
  eval reports.
- Secrets and credentials are redacted before any provider-visible summary,
  React-visible trace, copied debug JSON, or local artifact preview.
- Compression metrics are observable: raw size, compact size, estimated token
  reduction, profile used, truncation reason, and artifact reference.
- Build Chat and Context Chat use one Rust-owned compression/context boundary
  instead of ad hoc truncation in `commands/chat.rs`, `modules/ai.rs`,
  datasource code, or workflow code.
- Quality gates prove the same product outcome before and after compression:
  Build proposals still validate, dry-run evidence still works, widget values
  still come from real datasource pipelines, and recorded eval answers do not
  lose required facts.

## Approach

1. Define runtime compression profiles.
   - Add a Rust module such as `modules/context_compressor.rs`.
   - Profiles should include at least:
     - `chat_tool_result`,
     - `mcp_tool_result`,
     - `http_response`,
     - `external_source_result`,
     - `datasource_sample`,
     - `pipeline_trace`,
     - `provider_intermediate`,
     - `error_or_failure`.
   - Each profile returns a typed `CompressedArtifact` with compact value,
     preserved facts, loss/truncation metadata, raw/compact byte counts, and
     optional `raw_artifact_ref`.

2. Preserve high-signal facts instead of blindly truncating.
   - JSON/object compression should preserve keys/types, item counts, selected
     sample rows, schema paths, numeric ranges where cheap, and fields used by
     downstream pipeline/output paths.
   - Array/table compression should preserve columns, row count, first useful
     rows, failure rows, aggregate hints, and detected empty/null-heavy fields.
   - Log/text compression should preserve errors, stack traces, warnings,
     failed test/assertion lines, final summaries, repeated-line counts, and
     explicit omitted-line ranges.
   - HTTP/MCP compression should preserve source identity, effective URL or
     server/tool identity, status, duration, attribution, output envelope
     unwrapping status, and parsed root shape.
   - Pipeline compression should preserve step order, input/output shape
     changes, first-empty step, errors, durations, and the exact step config
     needed to reproduce the trace.

3. Store raw artifacts locally.
   - Add a bounded local artifact store in SQLite or app-data files, depending
     on size and retention needs.
   - Persist artifact metadata in SQLite: id, owner kind/id, profile, raw size,
     compact size, checksum, redaction version, created_at, and retention class.
   - Store raw bytes/value only after redaction policy has a clear answer.
     Credential-bearing data should either be redacted before storage or marked
     non-retained with an explicit reason.
   - Keep runtime SQLite audit rules: normal debugging reads the app DB
     read-only; schema changes still go through `Storage::migrate()`.

4. Replace ad hoc provider-facing compaction.
   - Move `compact_json_for_provider` out of `modules/ai.rs` into the shared
     compressor and make provider message construction consume
     `CompressedArtifact`.
   - Update chat tool results, MCP tool results, external source tool results,
     datasource test runs, pipeline traces, reflection context, and Build Chat
     source/widget mentions to use the same compressor where they send context
     to a provider.
   - Do not change the underlying `ToolEngine`, `MCPManager`,
     `WorkflowEngine`, scheduler, or datasource execution semantics.

5. Add bounded detail recovery.
   - Expose a provider-callable tool such as `inspect_artifact` only inside the
     existing Rust tool loop, with strict limits on path/range/row count and
     with the same policy/redaction boundary as every other tool.
   - The compact summary should tell the model what can be requested next:
     JSON path, row window, line range, or error block id.
   - Detail recovery must obey `MAX_TOOL_ITERATIONS`, repeat-loop detection,
     context budget, and cost budget. It must fail closed when the artifact is
     unavailable, expired, too large, or unsafe to expose.

6. Make quality measurable.
   - Add before/after fixtures with bulky but realistic outputs:
     - large HTTP JSON payload,
     - MCP envelope with nested JSON text,
     - repeated log/test output with one failure,
     - large table datasource,
     - multi-step pipeline trace with one emptying step,
     - Build Chat run that must use a specific field hidden deep in the raw
       payload.
   - For every fixture, assert both reduction and fact retention. The exact
     answer/proposal must still pass W16/W29 validation where applicable.
   - Track median and p90 provider-visible reduction in the eval report. The
     target is RTK-class effect, not cosmetic truncation.

7. Surface compression honestly.
   - Chat/tool traces and widget provenance should show when a result was
     compressed, how much was omitted, and whether raw local detail is
     available.
   - Workbench and Pipeline Debug should offer "compact sent to model" and
     "raw local artifact" views when both exist.
   - If compression cannot preserve required facts, the runtime should send a
     safe explicit failure/needs-detail state rather than pretending the compact
     summary is complete.

## Files

- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/context_compressor.rs` (new)
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/tool_engine.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/debug.rs`
- `src-tauri/src/commands/external_source.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/pipeline.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/mod.rs`
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/lib/chat/runtime.ts`
- `src/components/layout/ChatPanel.tsx`
- `src/components/debug/PipelineDebugModal.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/**`
- `scripts/acceptance.mjs`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W49_CHAT_CONTEXT_ECONOMY_AND_COST_ACCOUNTING_REPAIR.md`
- `docs/W51_PROVIDER_CONTEXT_COMPRESSION_AND_RAW_ARTIFACT_RETENTION.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- `bun run eval`
- `bun run acceptance`
- Unit or integration checks for:
  - JSON/object compression preserves required paths and reports omitted keys,
  - array/table compression preserves shape, count, and sample rows,
  - log/test compression preserves the failure while removing repetitive pass
    noise,
  - MCP envelope compression unwraps the parsed root and records envelope
    metadata,
  - raw artifact retention writes metadata, checksum, size, and retention class,
  - `inspect_artifact` returns bounded slices and denies unsafe/expired access,
  - provider-facing tool results use compressed artifacts rather than raw bulky
    payloads,
  - no secret/header/API-key value appears in compressed summaries, local
    previews, or copied debug JSON,
  - reduction metrics are recorded for each compressed provider-visible
    artifact.
- Recorded evals:
  - compare pre-compression and post-compression proposal quality for the same
    captured large-output scenarios,
  - assert W16/W29 validation still passes,
  - assert the agent can request bounded detail when the first compact summary
    intentionally omits a needed row/path,
  - assert median reduction is at least 60% and p90 reduction reaches 90% on
    repetitive/log/table fixtures.
- Manual running-app smoke:
  - run a real-provider Build Chat session that calls an external source or MCP
    tool with a large payload,
  - confirm the next provider turn receives the compact artifact summary, not
    the raw payload,
  - inspect the raw artifact locally through the appropriate debug surface,
  - confirm the final widget uses the correct datasource field and validation
    did not degrade.

## Out of scope

- Installing or shelling out to the external `rtk` binary inside the product
  runtime.
- Shell hook integration for developer tools such as Codex, Claude Code,
  Cursor, or terminal sessions.
- Replacing W49 cost accounting, W41 provenance, W23 Pipeline Debug, W30
  Workbench, or the Rust workflow engine.
- Hiding errors, validation failures, provenance, or policy denials to reduce
  token usage.
- Sending secrets, raw credentials, provider keys, or unredacted headers to the
  provider or React.
- Exact tokenizer parity for every provider when conservative byte/token
  estimates are enough for gating.
- Cloud/team artifact retention policy.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W17_AGENT_MEMORY_RAG.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
- `docs/W49_CHAT_CONTEXT_ECONOMY_AND_COST_ACCOUNTING_REPAIR.md`
- `https://www.rtk-ai.app/`
- `https://github.com/rtk-ai/rtk`
