# W38 Build Chat Widget Mentions

Status: shipped

Date: 2026-05-17

## Context

After a dashboard already exists, Build Chat should support targeted edits to
specific widgets instead of relying only on natural-language descriptions such
as "change the second chart" or "fix the KPI on the right". The product needs a
stable way for the user to mention one or more existing widgets in the Build
chat input and ask the agent to update, replace, explain, debug, or remove only
those widgets.

This belongs after the datasource/provenance streams because a useful widget
mention is more than a title. The agent needs compact typed context: widget id,
title, kind, datasource binding, tail pipeline, current runtime status, recent
trace/snapshot when available, and validation constraints. Mentions must never
be title-only or DOM-position-only references.

## Goal

- Build Chat input supports mentioning widgets from the active dashboard.
- Mention chips are backed by stable `widget_id` values and remain readable even
  when titles duplicate.
- The mention picker is scoped to the current dashboard and shows enough context
  for selection: title, widget kind, datasource/source hint, freshness/error
  state, and optional short preview.
- Sending a Build message includes typed mentioned-widget context in the chat
  request/session state, not just inline `@title` text.
- Build Chat prompt construction receives a compact target-widget bundle for
  every mentioned widget.
- Build proposals respect the mention target set:
  - update/replace uses `replace_widget_id` for mentioned widgets,
  - remove uses `remove_widget_ids` only for mentioned widgets unless the user
    explicitly asks for broader cleanup,
  - adding new widgets is allowed only when the user asks for additions or when
    the task cannot be satisfied as an update.
- Validation flags proposals that modify unmentioned existing widgets during a
  targeted Build turn unless the user explicitly requested broader changes.
- Proposal preview and explicit apply confirmation remain mandatory.
- Regenerate, retry, and session reload preserve the mentioned-widget target
  set for the turn.

## Approach

1. Define the widget mention model.
   - Add a `WidgetMention` or equivalent type with `dashboard_id`,
     `widget_id`, display label, widget kind, title, and optional provenance /
     runtime summary fields.
   - Store mentions as structured message metadata or a typed chat request
     field. Do not parse the raw prompt text as the source of truth.
   - Keep labels presentation-only because titles can be duplicated or changed.

2. Add mention UX to Build Chat.
   - In Build mode with an active dashboard, typing `@` or pressing an explicit
     mention button opens a keyboard- and pointer-friendly widget picker.
   - The picker lists widgets from the current dashboard with type/source/status
     hints and supports filtering by title/kind.
   - Selected widgets render as chips in or near the composer, can be removed
     before send, and are included in the sent turn.
   - If no dashboard is active, the mention affordance is disabled with a clear
     create/select-dashboard state.

3. Build compact target context on send.
   - Resolve mentioned ids against the latest active dashboard before invoking
     Rust so deleted or stale widgets fail closed with a typed UI error.
   - Include only compact summaries in the prompt context: widget config,
     datasource definition id/provenance, tail pipeline step count or summary,
     current runtime value shape/freshness/error, and recent W23/W36 trace or
     snapshot metadata when available.
   - Do not inject full large datasets or unmasked tool/provider secrets into
     chat context.

4. Thread target scope through backend chat/build logic.
   - Extend the Build chat request/session/run state with
     `target_widget_ids` or equivalent typed metadata.
   - Prompt the model to treat mentioned widgets as the allowed edit target set.
   - When the user asks to "fix this" / "change these" / "explain this", anchor
     the task on the mentioned widgets even if the raw text is ambiguous.
   - Preserve the target set across validation retry and regenerate paths.

5. Enforce targeted proposal semantics.
   - Extend proposal validation with a targeted-build rule:
     proposals may replace/remove mentioned widgets; they may not silently
     replace/remove unrelated widgets.
   - If a proposal adds widgets during a targeted edit, require explicit user
     intent or return a validation issue explaining why the add is blocked.
   - Surface validation failures in the same Build preview flow used by W16/W29.

6. Integrate with existing widget tooling.
   - Mentioned widgets can be opened in W23 Pipeline Debug, W32 Studio, W35
     Operations, or W36 Snapshot surfaces from the preview/context UI when those
     streams are present.
   - Build Chat should be able to use dry-run evidence for mentioned stat,
     gauge, bar_gauge, status_grid, and aggregating table widgets before
     previewing changes.

7. Add focused regression coverage.
   - Cover duplicate widget titles, stale/deleted mentioned widget ids, targeted
     replace, targeted remove, blocked unrelated replacement, allowed explicit
     add, validation retry, regenerate, and reload of a session with mention
     metadata.

## Files

- `src/components/layout/ChatPanel.tsx`
- new chat composer/picker components under `src/components/chat/` if cleaner
- `src/components/layout/DashboardGrid.tsx` only if widget selection/focus
  affordances are added
- `src/lib/chat/runtime.ts`
- `src/lib/api.ts`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/modules/ai.rs`
- `src-tauri/src/modules/storage.rs` only if mention metadata needs migration
  or indexed persistence beyond existing chat message JSON
- `docs/RECONCILIATION_PLAN.md`
- `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - mention model serialization across TypeScript and Rust,
  - widget picker list scoped to the active dashboard,
  - duplicate-title mentions preserving distinct ids,
  - stale/deleted mentioned widget id returning a typed error,
  - Build request carrying `target_widget_ids`,
  - validation allowing replacement/removal of mentioned widgets,
  - validation blocking replacement/removal of unmentioned widgets,
  - validation retry preserving target scope,
  - regenerate/retry preserving mention metadata.
- Manual running-app smoke:
  - create or open a dashboard with at least three widgets, including two with
    similar or duplicate titles,
  - open Build Chat and mention one widget,
  - ask to change its visualization without naming it in prose,
  - confirm preview targets the mentioned widget via `replace_widget_id`,
  - mention two widgets and ask to remove them,
  - confirm only those ids appear in `remove_widget_ids`,
  - ask for a targeted edit that the model tries to solve by changing another
    widget and confirm validation blocks it,
  - regenerate the turn and confirm target scope remains unchanged,
  - reload the session and confirm the mention chips/context remain readable.

## Out of scope

- Auto-applying widget changes from chat.
- Cross-dashboard widget mentions.
- Mentioning arbitrary datasources, workflows, providers, or external sources
  unless a later task adds those mention kinds explicitly.
- Title-only or layout-position-only targeting as the source of truth.
- Replacing W16/W29 proposal validation with frontend-only checks.
- A full collaborative editor or multi-user commenting model.
- Direct React-side provider, MCP, or tool calls.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W28_CHAT_UX_HARDENING.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`
- `docs/W35_WORKFLOW_OPERATIONS_COCKPIT.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`
