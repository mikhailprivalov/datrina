# W33 Real Provider Acceptance And Agent Eval V2

Status: shipped (v1, 2026-05-17)

Date: 2026-05-17

## Outcome (v1)

- New `AIProvider` trait in `src-tauri/src/modules/ai.rs`. `AIEngine`
  retains ownership of real network calls; the trait exposes a
  non-streaming `complete()` that the agent eval suite uses to inject a
  `RecordedProvider` without dragging product `LocalMock` back.
- `StructuredOutputCapability` + `supports_structured_output` capability
  map in `src-tauri/src/models/provider.rs`. Build proposal validator
  retry asks for `JsonObject`; if the model is not on the strict
  allowlist the call falls back to plain text and `AIResponse.strict_mode`
  surfaces the resolved tag so callers can record a visible fallback
  instead of accepting it as strict evidence.
- Recorded full-loop replay harness in `src-tauri/tests/agent_eval.rs`:
  - `ScenarioTrace.turns` carries captured assistant turns
    (content + tool_calls + token usage + per-turn structured-output
    request).
  - `RecordedProvider` implements `AIProvider` and feeds those turns
    one per `complete()` call.
  - `run_replay_loop` drives a deterministic loop that executes every
    tool call via an in-test registry, persists the assistant + tool
    transcript, and stops as soon as a turn parses as `BuildProposal`.
  - New `replay_loop_passes` assertion re-asserts the W16 validator,
    optional cost ceiling, and the resolved strict mode against the
    replayed final state.
  - First fixture: `recorded_replay_full_loop.yaml` — captures a
    happy-path stat proposal driven by `submit_plan` + `dry_run_widget`
    and pins `expect_strict_mode: true` for `moonshotai/kimi-k2.6`.
- Live eval lane in the same test file, gated by
  `--features expensive_evals` and `#[ignore]`. Reads
  `DATRINA_LIVE_{BASE_URL,API_KEY,MODEL,KIND,PROMPT}`; the lane never
  silently no-ops — missing env vars panic with the exact name.
- `parse_build_proposal_content` lifted from `commands::chat` into
  `commands::validation` so the eval harness exercises the same
  prose-extraction path the agent loop uses (not a parallel mirror).
- Acceptance runner: `scripts/acceptance.mjs` + `bun run acceptance`.
  Runs the static gates (tauri.conf, contract, typecheck, fmt, cargo
  check) and the replay eval lane; writes
  `docs/acceptance-report.json` with per-gate timing, exit code, and
  stderr tail for the first failure. `--include-live` opts into the
  expensive lane.

## v2 deferrals (still open)

- `response_format: json_schema` with a real `BuildProposal` schema is
  still future work; today the capability map only distinguishes
  `JsonObject` vs `PlainText`. Generating a strict schema for the
  union of `BuildWidgetType` variants is a separate workstream.
- Recorded streaming chunks (SSE replay) are not captured — the harness
  uses non-streaming `complete()` only. The validator and cost gates
  don't read the SSE transport so this is acceptable for v1, but a
  reasoning/visible-tokens regression test would need streaming
  fixtures.
- The harness executes a fixed tool registry (`submit_plan`,
  `dry_run_widget`, generic ok). It does not replay MCP tool results
  captured live; scenarios that exercise specific MCP response shapes
  would need a recorded MCP layer.

## Context

W24 shipped fast replay-mode assertions, and W29 removed product fake-success
provider paths. The remaining trust gap is live/recorded agent execution:
today the eval suite does not drive the full chat loop through a provider
abstraction, and the real-provider acceptance lane is still mostly manual.

This workstream promotes those deferrals into a repeatable release gate.

## Goal

- Extract a narrow provider abstraction that lets tests drive the full chat
  loop without product `LocalMock`.
- Add recorded provider replay for streamed and non-streamed responses.
- Add an explicit live-provider eval lane behind `expensive_evals`.
- Script the W29 acceptance path: provider setup, Context Chat, Build Chat,
  tool call, dry-run, validation-failed non-applyable proposal, successful
  apply, and reload verification.
- Add a provider/model capability map for strict structured output support and
  use it in Build proposal emission where supported.
- Produce a compact eval report with scenario status, model/provider, costs,
  validator issues, tool iterations, and artifacts.

## Approach

1. Extract provider trait.
   - Define the minimal `AIProvider` interface needed by chat, streaming,
     structured output, tool calls, and eval replay.
   - Keep production `AIEngine` as the owner of real network calls and secrets.
   - Add `RecordedProvider` / fixture providers only under tests or explicitly
     test-only modules.

2. Drive full-loop replay evals.
   - Extend W24 fixtures with recorded provider chunks/tool-call sequences.
   - Assert plan, tool calls, dry-run evidence, validation outcome, proposal
     shape, final message, loop count, and cost ceiling.
   - Keep default replay tests no-network and fast.

3. Add live eval lane.
   - Gate under `--features expensive_evals`.
   - Require explicit environment configuration for provider/model.
   - Record provider/model/cost/result without making credentials mandatory for
     normal local validation.

4. Add strict structured output capability handling.
   - Centralize provider/model support for `response_format: json_schema` or
     equivalent strict tool/proposal mode.
   - Use strict mode where supported; fall back visibly where not supported.
   - Ensure fallback is not counted as strict-mode acceptance evidence.

5. Add acceptance runner/report.
   - Provide one command or script that runs static gates plus selected evals.
   - Emit machine-readable JSON and a small human-readable report.
   - Make failures point to the exact scenario, tool call, validation issue, or
     provider capability mismatch.

## Files

- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/models/provider.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/**`
- optional `src-tauri/examples/record_eval.rs`
- `src/lib/api.ts` only if event/diagnostic shape changes
- `package.json` if script aliases are added
- `docs/W24_AGENT_EVAL_SUITE.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`

## Validation

- `bun run check:contract`
- `bun run typecheck`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021`
- `cargo check --workspace --all-targets`
- `bun run eval`
- `cargo test --test agent_eval --manifest-path src-tauri/Cargo.toml`
- Optional live lane, only when credentials/service are intentionally provided:
  `cargo test --features expensive_evals --test agent_eval --manifest-path src-tauri/Cargo.toml -- --ignored`
- Manual acceptance runner:
  - records provider kind/model,
  - runs Context Chat,
  - runs Build Chat with at least one tool call and dry-run,
  - verifies validation-failed proposals are non-applyable,
  - applies one valid proposal,
  - reloads app/test storage and verifies dashboard state.

## Out of scope

- Making real external credentials mandatory for default validation.
- Adding product-visible mock providers.
- Multi-agent orchestration.
- Benchmarking model quality beyond correctness and regression signals.
- Replacing the chat runtime or Workbench.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W24_AGENT_EVAL_SUITE.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
