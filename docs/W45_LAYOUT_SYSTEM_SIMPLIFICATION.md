# W45 Layout System Simplification And Build Layout Playbook

Status: shipped (v1)

Date: 2026-05-17

## Ship notes (v1)

- Added typed `SizePreset` (`kpi`, `half_width`, `wide_chart`, `full_width`,
  `table`, `text_panel`, `gallery`) and `LayoutPattern` (`kpi_row`,
  `trend_chart_row`, `operations_table`, `datasource_overview`,
  `media_board`, `text_panel`) on `BuildWidgetProposal`. Resolver lives in
  `SizePreset::resolve(&BuildWidgetType)` and is the only path from preset
  to concrete `(w, h)`.
- Apply path (`commands::dashboard::build_widget_shell`) ignores explicit
  `x`/`y` for new widgets, prefers `size_preset` over raw `w`/`h`, and
  falls back to the existing per-kind defaults when neither is set.
  Replacement widgets (`replace_widget_id` set) keep inheriting their
  slot's position; the validator's layout gate is skipped for them.
- Validator gates added: `ProposedExplicitCoordinates` and
  `ConflictingLayoutFields`. Both are wired into `validate_layout_fields`,
  surfaced in the synthetic retry feedback, and mirrored in
  `src/lib/api.ts` + `ChatPanel.formatValidationIssue`.
- Build mode system prompt now documents the named presets and patterns
  and tells the agent to drop `x`/`y` and prefer a `size_preset`.
- Eval fixtures: `layout_size_preset_passes` (happy path with `kpi`
  preset + dry-run evidence) and `layout_explicit_coordinates_rejected`
  (negative case that asserts both new validator variants fire).
- Out of scope for v1, still deferred: a size-preset editor in the
  dashboard UI (there is no raw coordinate input today — resize stays
  drag-only), and translating `layout_pattern` into anything stronger
  than a soft prompt hint.

## Context

Build Chat currently produces layouts that feel random. The product invariant is
already clear: the dashboard grid is 12 columns and auto-pack wins for new
widgets. The missing work is to simplify the layout contract exposed to the
LLM, remove ambiguous layout knobs, and give the model a small playbook of
typical dashboard layouts.

This task is about predictable layout generation and simpler layout semantics,
not a full dashboard redesign.

## Goal

- Build Chat proposes widgets using a small set of named layout patterns.
- The apply path remains row-first 12-column auto-pack for new widgets unless a
  later explicit decision changes it.
- LLM-supplied arbitrary `x`/`y` positions stay ignored or rejected for new
  widgets.
- Widget sizes are normalized by type and layout pattern.
- Generated dashboards look intentional instead of random.
- The layout system is easier to validate, preview, and explain.

## Approach

1. Define layout primitives.
   - Keep 12-column grid as the only dashboard grid model.
   - Define a small set of size presets such as `kpi`, `wide_chart`,
     `half_width`, `table`, `text_panel`, `gallery`, and `full_width`.
   - Map widget kinds to default size presets.
   - Remove or hide ambiguous knobs from Build proposal inputs where possible.

2. Add layout playbook.
   - Write concise system prompt instructions for common layouts:
     executive KPI row plus trend chart, operations table plus status cards,
     datasource overview, media/gallery board, text analysis panel with
     supporting metrics.
   - Tell the LLM to choose pattern and size intent, not exact coordinates.
   - Keep instructions focused on runtime dashboard generation, not marketing
     copy.

3. Normalize proposals at validation/apply.
   - Convert layout pattern and size intent into width/height defaults.
   - Ignore or reject new-widget `x`/`y` values according to the existing
     invariant.
   - Preserve explicit replacement semantics for existing widgets without
     wiping untouched widgets.
   - Add validation issues for unsupported layout pattern, impossible size, and
     conflicting layout fields.

4. Simplify UI and previews.
   - Preview should show how auto-pack will place the proposed widgets.
   - Widget edit controls should expose meaningful size presets or simple span
     controls instead of raw coordinate editing.
   - Keep drag/reorder behavior, if present, consistent with the same model.

5. Add eval coverage.
   - Extend replay evals with prompts for typical dashboard shapes.
   - Assert that generated proposals use size intent/pattern and do not depend
     on arbitrary coordinates.

## Files

- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/modules/ai.rs`
- `src/lib/api.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/widgets/*`
- `src-tauri/tests/agent_eval.rs`
- `src-tauri/tests/fixtures/agent_evals/*`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W45_LAYOUT_SYSTEM_SIMPLIFICATION.md`

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
  - proposal coordinates ignored/rejected for new widgets,
  - size presets normalized by widget kind,
  - unsupported layout pattern returns validation issue,
  - replace/remove delta semantics preserved,
  - auto-pack produces stable row-first layout.
- Manual running-app smoke:
  - ask Build Chat for several dashboard archetypes,
  - confirm proposed layouts follow recognizable patterns,
  - confirm preview matches applied auto-pack result,
  - resize/edit a widget and confirm layout controls remain understandable,
  - confirm no dashboard apply wipes untouched widgets.

## Out of scope

- Honoring arbitrary LLM-supplied coordinates.
- Adding a second layout engine.
- Pixel-perfect freeform canvas editing.
- Full visual redesign.
- Drag-and-drop builder overhaul unless required to align with simplified
  layout semantics.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W27_CYBERPUNK_REDESIGN.md`
- `docs/W28_CHAT_UX_HARDENING.md`
- `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
- `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`
