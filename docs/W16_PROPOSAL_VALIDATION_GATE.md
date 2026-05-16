# W16 Proposal Validation Gate

Status: implemented (json_schema strict mode deferred — see "Implementation notes")

Date: 2026-05-16

## Implementation notes

Shipped 2026-05-16:

- New `ValidationIssue` enum in `src-tauri/src/models/validation.rs` covers
  `missing_datasource_plan`, `unknown_replace_widget_id`,
  `unknown_source_key`, `hardcoded_literal_value`,
  `text_widget_contains_raw_json`, `missing_dry_run_evidence`,
  `pipeline_schema_invalid`, `duplicate_shared_key`. Mirrored in
  `src/lib/api.ts`.
- `src-tauri/src/commands/validation.rs::validate_build_proposal` runs
  the structural checks and matches `dry_run_widget` tool calls by
  widget title against the session transcript. Returns
  `Vec<ValidationIssue>`.
- `send_message_stream_inner` (`commands/chat.rs`) runs the validator
  after the natural tool-loop exit. On non-empty issues it emits
  `ChatEventKind::ProposalValidation` with `AgentEvent::ProposalValidationResult { status: Started, issues, retried: false }`,
  injects a synthetic `[validation_failed]` system message into a fresh
  `grounded_messages` build, and re-runs **non-streaming**
  `complete_chat_with_tools_json_object`. Re-parses + re-validates;
  emits `Completed` (issues empty) or `Failed` (issues remain) for the
  retry result.
- `complete_chat_with_tools_json_object` in `src-tauri/src/modules/ai.rs`
  is the new sibling of `complete_chat_with_tools` that adds
  `response_format: {"type": "json_object"}` for OpenAI-compatible
  providers. Strict `json_schema` mode is deferred — `json_object` mode
  already eliminates the wrapping-prose parse failures we hit in
  practice; strict schema would require deriving / hand-writing the full
  recursive `BuildProposal` schema with `additionalProperties: false`
  everywhere, which is a separate carve-out.
- Loop detection lives next to validator wiring. A
  `VecDeque<(tool_name, canonical_json(args))>` of capacity 5 is
  maintained across the tool loop; a third identical key in the window
  short-circuits the call. The short-circuit emits
  `AgentEvent::AgentPhase { phase: LoopDetected { tool_name }, status: Failed, ... }`
  and synthesises a `loop_detected` `ToolResult` with an instructive
  error string so the agent sees the constraint on its next turn.
- New `AgentPhase` variants: `LoopDetected { tool_name }`,
  `ProposalValidation`. New `ChatEventKind::ProposalValidation`.
- Frontend: `runtime.ts` handles the new agent event with a
  `proposal_validation` runtime-only `ChatMessagePart`. `ChatPanel.tsx`
  renders `ProposalValidationTile` (green pass / amber retrying / red
  failed) with a localized list of issues.

## Context

## Context

Today the Build Chat parses a `BuildProposal` out of the LLM's text stream
and hands it straight to the UI preview (`commands/dashboard.rs:189-204`,
`commands/chat.rs` proposal parse path). The agent is not gated on
correctness — broken Apply paths surface only when the user clicks Test or
Apply, by which point the proposal already polluted the chat history. Three
specific holes feed this:

1. **No critic pass** between proposal parse and preview display.
2. **`dry_run_widget` is a recommendation in the prompt** (`chat.rs:1974-1977`),
   not enforced. The agent silently skips it for testable widget kinds.
3. **No loop / convergence detection.** `MAX_TOOL_ITERATIONS = 40`
   (`chat.rs:873`) is the only stop; the rule "NEVER call same tool twice"
   lives only as text in the system prompt (`chat.rs:1826`).
4. **No structured outputs.** `ai.rs:209-231` sends no `response_format`,
   so the final proposal is text-encoded JSON that occasionally fails to
   parse on a stray character.

## Goal

The agent's final proposal is gated by deterministic checks **before** the
user sees a preview. Failures are routed back into the tool loop as
synthetic `tool_result` payloads so the agent must self-correct or fail
honestly; the user never receives a structurally invalid proposal.

## Approach

### Validator (`src-tauri/src/commands/validation.rs`, new module)

Add `fn validate_build_proposal(proposal: &BuildProposal, dashboard: &Dashboard,
context: &ValidationContext) -> Vec<ValidationIssue>`. Issues are typed:

- `MissingDatasourcePlan { widget_id }` — widget has no `datasource_plan`
  and no `source_key` reference.
- `UnknownReplaceWidgetId { widget_id, replace_widget_id }` — references a
  widget id that doesn't exist on the dashboard.
- `UnknownSourceKey { widget_id, source_key }` — references a shared
  datasource key not declared in `proposal.shared_datasources`.
- `HardcodedLiteralValue { widget_id, path }` — heuristic: stat/gauge/bar_gauge
  values that are numeric literals not referencing a pipeline output.
- `TextWidgetContainsRawJson { widget_id }` — text widget markdown body
  matches `^\s*[{\[]` and parses as JSON.
- `MissingDryRunEvidence { widget_id, kind }` — testable widget kind
  (stat, gauge, bar_gauge, status_grid, table-with-aggregate) lacks a
  successful `dry_run_widget` tool call in the current session's tool
  history for its id (or generated stub id).
- `PipelineSchemaInvalid { widget_id, error }` — `PipelineStep` array
  fails to deserialize against the strict schema.

### Tool-loop integration (`src-tauri/src/commands/chat.rs`)

After parsing the proposal from the final assistant text **but before**
emitting `MessageCompleted`:

1. Run `validate_build_proposal`.
2. If issues are non-empty AND the current `tool_iteration < MAX_TOOL_ITERATIONS`,
   inject a synthetic `tool_call` + `tool_result` pair into the conversation
   describing the failures, emit the same as visible `AgentEvent::ToolResult`
   for UX honesty, and force one more provider turn so the agent retries.
3. If issues persist after the retry budget — emit `MessageFailed` with a
   structured `ValidationFailed { issues }` body. The preview is not shown.

The retry budget is one additional turn per session by default (configurable
const `MAX_VALIDATION_RETRIES = 2`).

### Loop detection (`src-tauri/src/commands/chat.rs`)

Track `recent_tool_calls: VecDeque<(String /*name*/, String /*sha1 of canonical args*/)>`
capped at 5. Before dispatching a tool call, check whether the same `(name,
args_hash)` appears in the last 3 entries. If yes:

- Emit `AgentEvent::AgentPhase { phase: LoopDetected, status: Failed, detail:
  Some(format!("repeated tool {} with identical args", name)) }`.
- Inject a synthetic `tool_result: "loop_detected: tool '{name}' has been
  called with identical arguments. Vary your approach or finalize."`
- Skip the actual tool execution, force a provider turn.

This prevents the 40-iteration budget from being wasted on a stuck loop.

### Structured outputs (`src-tauri/src/modules/ai.rs`)

Add an optional `response_format: Option<ResponseFormat>` parameter to the
streaming and non-streaming OpenAI-compatible request builders. On the
**proposal-emitting turn** (detected by the system-prompt mode), serialize
the JSON Schema of `BuildProposal` (derived once via `schemars` crate or
hand-written) and pass:

```json
"response_format": {
  "type": "json_schema",
  "json_schema": { "name": "build_proposal", "schema": {...}, "strict": true }
}
```

For OpenRouter, gate on a per-model capability map (Kimi K2.6, GPT-4o,
Claude 3.5 Sonnet → supported; older models → fall back to plain text
parsing). Capability map lives in `models/provider.rs` as a static.

Falling back is silent: if a provider rejects `response_format`, log a warn
and re-send without it.

### Enforced dry_run check inside the validator

The validator scans `session.messages` for `tool_call` parts where
`tool_name == "dry_run_widget"` and matches them against each proposal
widget by `widget_id` (or by an LLM-generated stub id captured in the
dry-run arguments). For each testable widget kind without a successful
dry-run trace, raise `MissingDryRunEvidence`.

## Files to touch

- `src-tauri/src/commands/validation.rs` (new) — validator module.
- `src-tauri/src/commands/chat.rs` — call validator after proposal parse;
  loop detection; thread `ResponseFormat` through proposal turn.
- `src-tauri/src/modules/ai.rs` — `response_format` plumbing in streaming +
  non-streaming OpenAI-compatible paths.
- `src-tauri/src/models/dashboard.rs` — `schemars::JsonSchema` derive on
  `BuildProposal` and friends; or hand-rolled static schema constant.
- `src-tauri/src/models/provider.rs` — `supports_json_schema(model_id)` map.
- `src-tauri/src/models/chat.rs` — extend `AgentPhase` with `LoopDetected`,
  `ValidationFailed`.
- `src/lib/api.ts` — mirror the new agent-phase variants and the
  `ValidationIssue` type for the UI failure tile.
- `src/components/layout/ChatPanel.tsx` — render `ValidationFailed` as a
  red-bordered tile with issue list, distinguishable from generic errors.

## Validation

- `bun run check:contract` after the new agent-phase variants ship.
- `cargo check --workspace --all-targets`.
- Manual: paste a known-broken prompt that triggers a hardcoded stat value;
  observe the synthetic `validation_failed` tool result in the visible chat
  trace and the agent's retry attempt.
- Manual: force a model that does not support `response_format` (e.g.
  rename capability map locally) — confirm silent fallback to plain text.
- Manual: send a prompt that goads the agent into calling the same tool
  with the same args three times — confirm `loop_detected` phase and
  injected tool result.

## Out of scope

- Re-running validation against persisted dashboards (this gates new
  proposals only).
- Auto-fixing validation issues without a provider turn.
- Replacing the existing one-resume tool loop semantics — W16 adds gating
  on top of the same loop.

## Related

- Builds directly on the system-prompt anti-hardcode discipline and the
  optional `dry_run_widget` tool from prior work.
- Precondition for W24 eval suite (validator becomes the primary assertion
  surface for golden prompts).
