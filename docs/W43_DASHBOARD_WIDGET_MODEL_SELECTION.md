# W43 Dashboard And Widget Model Selection

Status: shipped

Date: 2026-05-17

## Context

Users need to choose different LLMs for dashboards and for individual
LLM-backed widgets. Today provider/model selection is primarily chat/provider
configuration, while widget runtime paths need explicit model policy,
capability checks, cost visibility, and honest unsupported states.

This task adds dashboard-level defaults and per-widget overrides for eligible
LLM-backed widget paths.

## Goal

- Dashboards can define a default provider/model policy for LLM-backed widgets.
- Eligible widgets can override the dashboard default with a specific provider
  and model.
- Widget details show the effective model and whether it comes from dashboard
  default, widget override, or provider fallback.
- Unsupported model capabilities fail closed with typed remediation, not fake
  success.
- Cost/token metadata integrates with W22 where available.
- Secrets remain Rust-owned and are never stored in widget JSON or React state.

## Approach

1. Define model policy shapes.
   - Add dashboard default model policy and widget override policy to the
     relevant Rust models.
   - Store provider id/model id/capability requirements, not credentials.
   - Include inheritance metadata so the UI can explain the effective selection.

2. Add provider capability validation.
   - Validate streaming, JSON/object output, tool calling, context length, and
     other required capabilities before a widget run starts.
   - Use W33 structured-output capability handling rather than ad hoc provider
     assumptions.
   - Return typed unsupported state with remediation when the selected model
     cannot run the widget path.

3. Build selection UI.
   - Add dashboard settings for default widget LLM policy.
   - Add per-widget model override controls in widget details for LLM-backed
     widgets.
   - Hide or disable model controls for deterministic pipeline-only widgets
     while still showing "No LLM in this widget" from W41.

4. Integrate runtime resolution.
   - Resolve effective provider/model at refresh time from widget override,
     dashboard default, then app provider default.
   - Record effective selection in widget provenance and stream events.
   - Keep model changes versioned with dashboard/widget changes where existing
     versioning supports it.

5. Add cost visibility.
   - Surface estimated/actual token and cost summary for recent widget runs when
     W22 data exists.
   - Do not block model selection on exact cost support for providers that do
     not expose it; render unknown cost explicitly.

## Files

- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/provider.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/provider.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W43_DASHBOARD_WIDGET_MODEL_SELECTION.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - dashboard default model policy persistence,
  - widget override persistence,
  - effective model resolution order,
  - deterministic widget marked ineligible for model override,
  - unsupported capability returns typed error,
  - provider secrets not serialized into dashboard/widget JSON.
- Manual running-app smoke:
  - set a dashboard default model,
  - create two LLM-backed widgets and override one of them,
  - confirm widget details show effective model and inheritance source,
  - choose a model lacking a required capability and confirm honest failure,
  - confirm deterministic widgets show no LLM/model selector.

## Out of scope

- Team-level model policy/RBAC.
- Cloud billing integration beyond local W22 cost summaries.
- Automatic model benchmarking or routing.
- Storing credentials in dashboard/widget config.
- Changing non-widget chat model selection semantics except where shared
  provider capability code is reused.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
- `docs/W41_WIDGET_EXECUTION_OBSERVABILITY.md`
- `docs/W42_WIDGET_STREAMING_REASONING.md`
