# W22 Token And Cost Tracking

Status: shipped

Date: 2026-05-16

## Context

The agent can burn meaningful money in one session (Kimi K2.6 with 40
tool iterations easily reaches 50–200k tokens). The user sees zero
indication of consumption: no token counter, no $ counter, no per-session
budget. Provider `usage` objects from OpenRouter are not parsed or
persisted today.

This is both a usability gap (transparency) and an operational risk
(runaway costs in autonomous runs from W21).

## Goal

- Token usage is parsed from every provider response (streaming and
  non-streaming) and persisted per-message and per-session.
- A per-model rate table converts tokens to $ at message time.
- Footer of the chat panel shows live "12.4k in / 8.2k out · $0.043"
  for the current session, "$0.84 today" globally.
- Per-session optional budget (`max_session_cost_usd`); the agent is
  hard-stopped with an honest error if exceeded.
- Per-day soft budget warning across all sessions.

## Approach

### Usage parsing (`src-tauri/src/modules/ai.rs`)

OpenAI-compatible streaming responses emit a final SSE chunk like:

```json
{"choices":[],"usage":{"prompt_tokens":1234,"completion_tokens":567,"total_tokens":1801}}
```

OpenRouter additionally includes a `usage.total_cost` field on the final
non-streaming response (or you can compute it). For streaming, OpenRouter
requires `stream_options: {"include_usage": true}` in the request. Add
that flag to the streaming request body.

Parse `usage` whenever present. Surface it through a new field on
`AIResponse`:

```rust
pub struct UsageReport {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub reasoning_tokens: Option<u32>, // some providers report this separately
    pub total_tokens: u32,
}
```

For local-only / mock providers, usage is `None`.

### Rate table

`src-tauri/src/models/pricing.rs` (new) — static map keyed by
`(provider_kind, model_id_pattern)`:

```rust
pub struct ModelPricing {
    pub input_usd_per_1m: f64,
    pub output_usd_per_1m: f64,
    pub reasoning_usd_per_1m: Option<f64>,
}

pub fn pricing_for(provider: &Provider, model: &str) -> Option<ModelPricing>;
```

Seeded with the models we actually support (Kimi K2.6, GPT-4o, Claude
3.5 Sonnet, Gemini 1.5 Pro, a few cheap workhorses). Pattern matching
on model id supports versioned aliases.

A user-editable JSON file in app data dir (`pricing_overrides.json`)
takes precedence — lets the user keep up with OpenRouter price changes
without a Datrina release.

### Persistence

#### Per-message

Extend `ChatMessage` with optional `usage: UsageReport` and computed
`cost_usd: f64`. Already partly present (the summary mentions usage); now
formalised and used.

#### Per-session

New columns on `chat_sessions`:

```sql
ALTER TABLE chat_sessions ADD COLUMN total_input_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chat_sessions ADD COLUMN total_output_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chat_sessions ADD COLUMN total_reasoning_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chat_sessions ADD COLUMN total_cost_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE chat_sessions ADD COLUMN max_cost_usd REAL;
```

Updated transactionally with every persisted message.

#### Daily roll-up

`cost_daily(date, total_usd, by_provider_json)` for the cheap aggregate
the footer needs. Recomputed on demand via SQL `SUM`; cached in memory.

### Budget enforcement

If `chat_sessions.max_cost_usd` is set and a streaming response would
push `total_cost_usd` over it (estimated before the request fires using
the input-token cost + a small completion buffer), the request is
denied with a `RunError`:
`budget_exceeded: session limit $X.XX already reached`.

The check runs:

- Pre-flight: before each `complete_chat_with_tools_streaming` call,
  add input cost from the message we are about to send.
- Post-flight: after parsing the `usage` chunk, update session totals;
  if newly-over budget, do not start another tool resume.

### UI

#### Chat footer

`ChatPanel.tsx` footer band:

```
kimi-k2.6 · 12.4k in / 8.2k out / 1.0k think · $0.043 · today $0.84
```

`$0.043` and `today $X` update live during streaming using the
provider's `include_usage` chunk; intermediate ticks come from a
local estimator that uses the streamed token count.

#### Per-session limit picker

In chat session header → settings icon → modal with:

- "Stop this session at $___" input (defaults to global default).
- "Stop at __ tokens" input (alternate cap).
- Daily soft warning at $___ shown as a banner once exceeded.

Defaults configurable in Settings → Costs.

#### Cost view

New Settings → Costs page:

- Last 30 days as a bar chart by day.
- Top 5 most expensive sessions with links.
- Top 3 most expensive models with token mix.
- Editable rate overrides (opens `pricing_overrides.json` in an inline
  editor).

### Wiring autonomous runs

W21's `agent_action` borrows `max_cost_usd` from the trigger config
(defaults to a conservative `0.10`). Prevents an autonomous loop from
burning real money silently.

## Files to touch

- `src-tauri/src/modules/ai.rs` — parse `usage` (streaming and
  non-streaming); enforce `include_usage` flag; return `UsageReport`.
- `src-tauri/src/models/pricing.rs` (new).
- `src-tauri/src/models/chat.rs` — `usage`, `cost_usd` on
  `ChatMessage`; budget fields on `ChatSession`.
- `src-tauri/src/commands/chat.rs` — pre/post flight budget checks;
  update session totals.
- `src-tauri/src/commands/cost.rs` (new) — `get_cost_summary`,
  `get_daily_costs`, `set_pricing_overrides`, `get_pricing_overrides`.
- `src-tauri/src/lib.rs` — register cost commands.
- `src-tauri/src/modules/storage.rs` — migration.
- `src/lib/api.ts` — mirror types; `costApi`.
- `src/components/layout/ChatPanel.tsx` — footer.
- `src/components/settings/CostsView.tsx` (new).
- `src/components/chat/SessionBudgetModal.tsx` (new).

## Validation

- `bun run check:contract`, `cargo check --workspace --all-targets`.
- Unit: given a synthetic stream with `usage` chunk, parser returns
  correct numbers.
- Unit: budget enforcement — set `max_cost_usd = 0.01`, run a request
  that would cost $0.05 — denied with `budget_exceeded`.
- Manual: real OpenRouter chat with Kimi K2.6 — footer counter
  increments live, final values match OpenRouter dashboard within 5%.
- Manual: edit `pricing_overrides.json`, restart — costs recompute.

## Out of scope

- Per-tool cost attribution (would require splitting tool-call vs.
  reasoning input tokens, which providers don't break down).
- Billing integration / invoices.
- Multi-currency.
- Cost forecasting based on plan complexity.

## Related

- W21 — autonomous runs honour the same budget mechanism.
- W18 — plan artifact can show estimated cost per step (Plan v2,
  follow-up).
