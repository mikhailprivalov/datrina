# W49 Chat Context Economy And Cost Accounting Repair

Status: shipped (2026-05-18)

Date: 2026-05-17

## Shipped summary

- `models::pricing::CostSource` (`provider_total` / `pricing_table` /
  `unknown_pricing`) + `TurnCost` are the typed result of pricing a
  single assistant turn. `accumulate_session_usage` returns a `TurnCost`
  and unconditionally counts tokens; turns without pricing bump
  `ChatSession::cost_unknown_turns` so `total_cost_usd = 0` is no longer
  rendered as `$0.000000` for an active session.
- OpenAI/OpenRouter `usage.cost` (and `total_cost`) flows into
  `TokenUsage::provider_cost_usd`. When present, accounting uses the
  upstream figure verbatim; otherwise it falls back to the local pricing
  table; otherwise marks the turn `unknown_pricing`. The OpenRouter
  request payload now sets `usage: { include: true }` to ask for the
  cost line.
- `SessionCostSnapshot` exposes `cost_unknown_turns` and
  `latest_cost_source` so the chat footer / Costs view can render
  `unknown cost`, `≥ $X.XXXX (N turns unpriced)`, or a clear "billed by
  provider" / "local pricing table" hover hint instead of a silent
  zero.
- `modules::context_budget` is the single Rust-owned context-economy
  compactor. `compact_for_provider` rewrites large tool-role `content`
  into compact status + shape + sampled-row summaries (sk-* tokens and
  other secret-looking keys are redacted), truncates assistant content
  /reasoning blobs older than the recent-turn tail, drops the oldest
  non-system turns when the budget is still busted, and inserts a
  single `[context_truncated] N earlier turn(s) were omitted` system
  marker so the model never mistakes the truncation for an empty
  history. The local `session.messages` is unchanged.
- `grounded_messages` runs the compactor for every Build and Context
  provider call. If even compaction can't bring the request under
  ~1.6× the soft char budget, the helper fails closed with a typed
  `context_overflow` error before the provider round-trip is opened.
- Unit coverage in `modules::context_budget::tests`
  (`compact_tool_message_collapses_large_array`,
  `compactor_drops_oldest_turns_when_over_budget`,
  `redacts_secret_keys_inside_tool_results`,
  `compactor_is_idempotent`) plus
  `commands::chat::tests::accumulate_session_*` (pricing-table cost,
  provider-total preference, unknown-pricing path) lock down the
  failure class described in the original report: 500k+ provider-input
  tokens displayed as `$0.000000`.

## Context

Real Build Chat sessions can become expensive quickly. A recent weather
dashboard session persisted more than 500k input tokens, while the session cost
still displayed as `0.000000`. The product already has W22 token/cost tracking,
but the live evidence shows two remaining problems:

- provider-visible context is too large because full message/tool history and
  bulky tool results are repeatedly sent back to the model,
- cost accounting is not reliably converting real provider usage into dollars.

This task repairs the accounting bug and adds a context-economy layer. It must
not weaken W29 no-fake-success behavior, tool visibility, or local auditability:
fuller local traces can remain available locally, but provider-facing context
must be compact, bounded, and explicit.

## Goal

- Real provider sessions show non-zero cost when usage and pricing are known.
- Unknown pricing renders as explicit `unknown cost`, not `$0.000000`.
- Provider-visible chat context has deterministic token/byte budgets.
- Large tool results are stored locally but summarized/pruned before being sent
  back to the provider.
- Build Chat keeps durable state through compact artifacts: current plan,
  proposal summary, datasource/source summaries, validation issues, and recent
  high-signal turns instead of the whole raw transcript.
- Budget checks use the repaired cost calculation and stop additional provider
  calls before runaway loops.
- Regression tests catch the exact failure class: high token usage with zero
  computed cost.

## Approach

1. Audit and fix cost calculation.
   - Trace provider `usage` parsing for streaming and non-streaming
     OpenAI-compatible/OpenRouter responses.
   - Prefer provider-reported `usage.total_cost` when present.
   - Otherwise compute cost from the model pricing table using input, output,
     and reasoning tokens where available.
   - If pricing is missing, store token counts and render cost as unknown
     instead of silently writing zero.
   - Ensure session totals update transactionally after every persisted
     assistant response and after tool-resume turns.

2. Add pricing and usage diagnostics.
   - Persist enough metadata to explain each cost line: provider id, model id,
     usage source (`provider_total_cost`, `pricing_table`, `unknown_pricing`),
     and strict/non-strict response mode when relevant.
   - Surface a compact cost diagnostic in chat footer/details so operators can
     tell whether the number came from provider usage or local pricing.
   - Keep pricing overrides from W22 as the operator repair path for new
     OpenRouter model prices.

3. Build provider-context budgeting.
   - Add one Rust-owned context builder for Build and Context chat.
   - Set per-turn budgets for system/developer instructions, recent messages,
     tool summaries, dashboard/widget context, datasource/source summaries, and
     validation feedback.
   - Budget by an approximate token counter or a conservative byte heuristic
     when tokenizer support is unavailable.
   - When context is truncated, include an explicit compact note in the prompt
     state rather than hiding the truncation.

4. Compact tool results.
   - Store full local tool results where current retention policy allows.
   - Send provider-facing tool result summaries with status, shape, selected
     sample rows, counts, error class, and redacted argument/source metadata.
   - Reuse or extend W23 pruning helpers so arrays and nested JSON are capped
     deterministically.
   - Never send secrets, headers, provider keys, or raw credential-bearing MCP
     env values to React or provider-visible context.

5. Add rolling Build session state.
   - Maintain a compact `current_plan`, applied proposal summary, datasource
     bindings, validation history, and last useful trace summaries as structured
     state.
   - Summarize older assistant/tool turns into state artifacts once the session
     exceeds the context budget.
   - Preserve raw transcript for local UI/history where practical, but do not
     require every raw turn to be provider-visible.

6. Enforce budgets before tool resume loops.
   - Estimate the next provider call size before resuming after tool execution.
   - If the context budget would be exceeded, compact first; if compaction
     cannot make the request safe, fail closed with a typed remediation.
   - Tie cost budget checks to the repaired cost calculation.

7. Add eval and runtime regression coverage.
   - Recorded fixture with large HTTP/MCP tool output verifies provider-facing
     result is compact while local trace remains inspectable.
   - Cost fixture with known token usage and pricing verifies non-zero session
     totals.
   - Missing-pricing fixture verifies `unknown cost` instead of zero.
   - Weather-style Build Chat fixture verifies repeated corrections do not
     resend the full raw Open-Meteo payload every turn.

## Files

- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/pricing.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/tool_engine.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/cost.rs` if it exists or is added by W22 follow-up
- `src-tauri/src/lib.rs`
- `src/lib/api.ts`
- `src/lib/chat/runtime.ts`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/**`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W24_AGENT_EVAL_SUITE.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W49_CHAT_CONTEXT_ECONOMY_AND_COST_ACCOUNTING_REPAIR.md`

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
  - provider-reported `total_cost` persisted into message/session totals,
  - pricing-table fallback producing non-zero cost,
  - missing pricing stored/rendered as unknown cost, not zero,
  - streaming `include_usage` final chunk updates totals,
  - provider-facing tool result pruning with full local trace retention,
  - context budget truncation/compaction with an explicit prompt note,
  - budget denial before the next provider call when session max cost is hit.
- Manual running-app smoke:
  - run a real-provider Build Chat session with tool calls,
  - confirm token and cost footer updates after the response,
  - inspect stored session totals and verify cost is not zero when pricing is
    known,
  - create a large HTTP result and confirm the next provider turn receives a
    compact summary,
  - confirm UI can still inspect local trace/debug details.

## Out of scope

- Cloud billing integration.
- Exact tokenizer parity for every provider if a conservative estimate is
  enough for safe budgeting.
- Per-tool cost attribution finer than provider usage allows.
- Deleting raw local chat history unless a separate retention policy is
  approved.
- Hiding tool failures or validation failures to save tokens.
- Reintroducing product mock providers.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W17_AGENT_MEMORY_RAG.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W24_AGENT_EVAL_SUITE.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
