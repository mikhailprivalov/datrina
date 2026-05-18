# W29 Real Provider Runtime And Honest Failure Gate

Status: shipped (v1, 2026-05-17)

## Outcome (v1)

- `ProviderKind::LocalMock` and `local_mock` TypeScript union arm are
  removed from product code, UI templates, send-error messages, and
  `pricing_for`.
- New `LegacyProviderKind` storage shim parses any non-supported kind
  (including legacy `local_mock`) into `Unsupported(_)`; the provider
  row loader marks the row `is_unsupported`, force-disables it, and
  refuses to let `set_provider_enabled` re-enable it without an edit to
  a supported kind. The UI surfaces an "unsupported" pill + remediation
  copy on those rows.
- Active provider resolution: chat send (streaming + non-streaming),
  reflection, autonomous alerts, dry-run, scheduled workflows, and
  `init_storage` all consult the new `resolve_active_provider` helper.
  No path silently falls back to "first enabled provider" anymore;
  chat sends without a usable provider return a typed
  `ProviderSetupError::*` code parseable on the frontend via
  `parseProviderSetupError`.
- Frontend `App.loadProviders` reads only `active_provider_id` — no
  rewrite-on-first-enabled fallback. ChatPanel banner copy points to
  Provider Settings (no `local_mock` escape hatch).
- Build proposal apply gate: `apply_build_proposal` runs the W16
  validator server-side and returns `proposal_validation_failed: …`
  when any issues remain, even if a stale UI surfaces the Apply
  button. The streaming + non-streaming chat paths now drop the
  `BuildProposal` part and metadata when residual validation issues
  survive the retry budget, so the assistant message renders only the
  typed `ProposalValidation::Failed` diagnostic.
- The W24 eval suite enum branch for `LocalMock` was removed; existing
  fixtures already targeted `openrouter` so no scenario YAML changed.

## v2 deferrals (closed by W33)

- Per-model capability map for structured output: shipped as
  `StructuredOutputCapability` + `supports_structured_output` in
  `src-tauri/src/models/provider.rs`. The validator retry now resolves
  the requested mode against the map and exposes the resolved tag via
  `AIResponse::strict_mode` so a soft fallback is visible.
  Note: `response_format: json_schema` (with a real `BuildProposal`
  schema) is still future work; today the map distinguishes
  `JsonObject` vs `PlainText` only.
- AIProvider trait extraction: shipped. `AIEngine` retains real
  network calls; `RecordedProvider` (test-only) implements the trait
  for the replay harness.
- Live-provider acceptance lane: shipped as
  `scripts/acceptance.mjs` (default lane) plus a credentials-gated
  `agent_evals_live_provider_smoke` test behind
  `--features expensive_evals`. The manual smoke list at the bottom
  of this doc is still the long-form acceptance walkthrough; the
  scripted lane covers the static + replay subset that does not need
  credentials.

See `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md` for the
shipped surfaces.

## Original plan (preserved below)


Date: 2026-05-17

## Context

The product now has a typed chat runtime, provider-backed Build Chat, proposal
validation, tool traces, cost tracking, eval fixtures, and a dashboard runtime.
The remaining runtime truthfulness problem is not architecture breadth; it is
that product paths can still look successful without a real provider or with an
invalid proposal.

Concrete gaps from the W29 research pass:

- `local_mock` is still a persisted product provider kind in Rust and TypeScript,
  appears as the default provider setup draft, and returns OK without network or
  credentials.
- Chat no-provider behavior is still mostly backend-error driven; the UI can let
  the user enter a send path before a usable provider exists.
- A Build proposal can still be visible and applyable after proposal validation
  reports failed issues.
- Active provider selection can recover through fallback behavior without a
  clear operator-facing correction.
- W24 replay evals are useful, but they are not live-provider product evidence.

This workstream removes fake product success paths. It does not replace the
chat architecture, provider runtime, dashboard engine, or eval suite.

## Goal

- Product runtime uses only real provider kinds:
  - OpenRouter,
  - Ollama when a reachable local service passes provider test,
  - Custom OpenAI-compatible providers.
- `LocalMock` is removed from production provider model/API/UI/runtime paths.
  Existing persisted `local_mock` rows are migrated to a disabled or unsupported
  state with a visible onboarding message; they are never silently selected.
- Mock behavior remains allowed only as test-only machinery, behind fixtures or
  test doubles that cannot be created from the product UI or stored as a normal
  provider.
- Chat and Build Chat fail closed before provider execution when no usable real
  provider is configured. The frontend disables Send with a Settings CTA; the
  backend returns typed provider setup errors instead of generic strings.
- Proposal validation failure blocks preview/apply. If issues remain after the
  retry budget, the chat receives a terminal failed state or a non-applyable
  diagnostic part. No failed proposal may emit an applyable preview.
- Active provider selection is explicit and observable. Invalid
  `active_provider_id` config is surfaced and repaired through user-visible
  settings flow, not hidden fallback.
- One real-provider acceptance lane proves Context Chat and Build Chat through
  tool calls, dry-run, validation, visible failure states, and final proposal
  apply behavior.
- Provider tool calls and proposal output use schema-first contracts where the
  selected provider supports them: strict tool schemas for callable actions and
  strict structured output for final Build proposal JSON. Plain JSON mode or
  text parsing is fallback behavior only and must remain visibly non-acceptance
  for the real-provider gate when the provider claims stricter support.

## Approach

1. Remove production `LocalMock`.
   - Delete `ProviderKind::LocalMock` from the production enum and TypeScript
     provider kind union.
   - Remove `local_mock_response`, `local_mock_tokens`, and LocalMock branches
     from production `AIEngine` completion, streaming, and provider test paths.
   - Remove local mock templates, first-run escape buttons, labels, and empty
     state copy from `ProviderSettings`, `ChatPanel`, docs, and prompts.
   - Add a storage migration or load-time normalization for existing
     `kind='local_mock'` provider rows. The migrated provider must be disabled
     or marked unsupported, and cannot become active automatically.
   - If legacy deserialization needs compatibility, keep it in a private
     storage migration type such as `LegacyProviderKind`; do not expose it
     through product API types, UI selectors, or active-provider selection.

2. Introduce test-only provider doubles.
   - Keep deterministic tests, replay evals, and no-network unit checks by
     extracting a test fixture/double layer that is not part of product
     `ProviderKind`.
   - If an `AIProvider` trait extraction is required, keep it narrow: enough for
     tests/evals to replay completions and tool calls, not a new provider
     framework.
   - Rename test fixtures clearly as `test_provider` / `recorded_provider` /
     `fixture_provider`; avoid `local_mock` product terminology.

3. Make provider setup fail closed and typed.
   - Define provider setup/runtime error codes mirrored in Rust and TypeScript:
     no active provider, active provider missing, active provider disabled,
     provider invalid config, provider unavailable, provider unsupported.
   - Chat send checks provider readiness before creating an in-flight run.
   - Frontend send controls and empty states distinguish no provider, failed
     provider test, missing API key, disabled provider, and unreachable Ollama.
   - Settings must test a provider and show the exact model/base URL before it
     can be selected as active, except when editing an already-active provider
     where the previous active state remains visible.

4. Block invalid Build proposals.
   - In the streaming chat path, if validation retry still fails, emit a
     terminal failure or a diagnostic message part and skip `BuildProposalParsed`.
   - In the runtime adapter and `ChatPanel`, proposal parts carry applyability
     explicitly. Validation-failed proposals render as diagnostics only.
   - Remove copy that says a failed proposal can still be applied.
   - Add regression coverage for a known bad proposal that must not show an
     enabled Apply action.
   - For OpenAI-compatible providers that support strict structured outputs,
     use a `json_schema` response format or strict proposal-emitting tool rather
     than relying on prose-wrapped JSON extraction.
   - Record provider/model capability decisions in one Rust-owned map so
     unsupported strict modes fall back explicitly and do not become acceptance
     evidence.

5. Tighten active-provider selection.
   - Read `active_provider_id` as the only normal active selection.
   - If it points to a missing/disabled/unsupported provider, surface a typed
     correction state and require the operator to choose a new active provider.
   - Do not silently fall back to the first enabled provider for chat/build
     execution without a visible status and persisted correction.

6. Add one live acceptance lane.
   - Document the expected local environment variables for the lane, but keep
     the default no-credential local checks focused on typed no-provider failure.
   - With credentials or reachable Ollama, run Context Chat and Build Chat using
     a named provider/model.
   - Exercise: successful provider test, provider unavailable failure, Context
     Chat response, Build Chat tool/dry-run/validation path, failed-validation
     non-applyable path, and one successful apply.

7. Constrain tool exposure per turn.
   - Keep Rust as the only tool executor, but expose only the tools relevant to
     the current Build/Context turn instead of always sending the full tool
     catalog when the prompt and provider capability allow a narrower set.
   - Preserve `call_id` / tool-call identity through request, execution,
     result, retry, persisted trace, and UI rendering.
   - If the provider can emit multiple tool calls in one turn, either handle
     them deterministically through the existing policy gateway or disable
     parallel tool calls for the acceptance lane.

## Files

- `src-tauri/src/models/provider.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/provider.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/**`
- `src/lib/api.ts`
- `src/lib/chat/runtime.ts`
- `src/components/layout/ProviderSettings.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/chat/**` if needed for typed error/diagnostic UI
- `docs/RECONCILIATION_PLAN.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- README/provider setup docs only if user-facing claims change

Do not edit dashboard/widget/pipeline behavior except where needed to block
apply of invalid Build proposals.

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- `bun run eval` or the current replay eval command after test fixture changes.
- Search gates:
  - `rg -n "local_mock|LocalMock|Local mock" src src-tauri/src`
    returns no production runtime/UI/model hits.
  - Any remaining mock/test hits are under tests, fixtures, examples, or docs
    that explicitly describe test-only behavior.
- Manual no-credential smoke:
  - first-run settings show only real provider options,
  - chat send is disabled or fails with typed no-provider remediation,
  - no persisted empty successful assistant turn is created.
- Manual live-provider smoke when credentials/service are available:
  - configure and test a named provider/model,
  - Context Chat returns a provider-backed answer with visible metadata,
  - Build Chat uses tool/dry-run/validation before proposal preview,
  - validation-failed proposal is non-applyable,
  - successful proposal still requires explicit Apply.

## Out of scope

- New provider marketplace or model catalog service.
- New agent framework, remote orchestration service, or cloud account system.
- Replacing the W15 typed message-parts runtime.
- Redesigning dashboards, widgets, or the pipeline DSL.
- Making real external credentials mandatory for default local build/typecheck
  validation.
- Hiding all test doubles. Test-only fixtures are allowed when they cannot be
  selected or persisted by product runtime.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W12_PROVIDER_DRIVEN_AGENTIC_DASHBOARD_BUILDER.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`
- `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W24_AGENT_EVAL_SUITE.md`
- `docs/W28_CHAT_UX_HARDENING.md`
- OpenAI Function Calling:
  https://developers.openai.com/api/docs/guides/function-calling
- OpenAI Structured Outputs:
  https://developers.openai.com/api/docs/guides/structured-outputs
- Tauri v2 command/event architecture:
  https://tauri.app/develop/calling-rust/
