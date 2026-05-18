# W46 Dashboard Header Resilience

Status: shipped v1

Date: 2026-05-17

## Shipped v1

- `src/components/layout/TopBar.tsx`: title now owns flex-1 with
  `min-w-0 flex-shrink truncate` and an explicit `title={dashboard.name}`
  tooltip; description collapses to `hidden lg:inline` and also carries a
  native tooltip so its content is reachable when hidden or truncated.
- Provider chip is still `hidden md:flex` but its label now collapses
  from `"name · model"` (with `max-w-[11rem]` at `xl+`) to just
  `"name"` (with `max-w-[8rem]` below `xl`); the chip's `title=` keeps the
  full provider+model string reachable. The colored status dot stays
  visible so connection state never disappears even when the label is
  short.
- `src/components/layout/DashboardGrid.tsx` →
  `DashboardModelPolicyControl`: replaced the
  `provider_id.slice(0, 6)` UUID-prefix label with the actual provider
  name resolved from `providers`; the chip caps at `max-w-[14rem]` with
  a static `"Model ·"` prefix and a truncating value, plus a `title=`
  tooltip carrying the full provider+model string. The toolbar row
  itself stays `flex-wrap items-center justify-end` so secondary actions
  wrap onto a second line at narrow widths without overlapping primary
  content.
- `src/components/dashboard/ParameterBar.tsx`: `PARAM_INPUT_CLASS` now
  includes `max-w-[16rem] truncate` so a single long option label (e.g.
  a URL or filepath) can no longer blow out the sticky parameter row
  horizontally. The row was already `flex-wrap`.

## Validation performed

- `node -e "JSON.parse(...)"` on `src-tauri/tauri.conf.json` — OK.
- `bun run typecheck` — clean.
- `bun run build` — clean (only the pre-existing 500KB chunk warning).
- Running-app smoke is not performed by this change agent because the
  Datrina app is a Tauri desktop window and cannot be observed
  visually from this environment. Per `AGENTS.md`, manual smoke
  remains the operator's responsibility for visual changes — open a
  dashboard with a long title, resize the window through desktop /
  medium / narrow, and confirm the header collapses predictably and
  the dashboard runtime behavior is unchanged.

## Out-of-scope confirmed

- No backend changes (`src-tauri/**` untouched).
- No dashboard runtime refresh changes — only CSS-layer adjustments and
  one label-source fix on the policy chip.
- Widget chrome was not modified; widget rendering already isolates its
  header inside `WidgetCell` and does not contribute to dashboard
  header clipping.

## Context

The dashboard header currently clips text too often. Titles, dashboard status,
provider/model controls, parameters, and action buttons compete for horizontal
space, especially when generated dashboard names are long or the viewport is
narrow. This is a focused frontend hardening task.

## Goal

- Dashboard header text does not clip or overlap in normal desktop and narrow
  window sizes.
- Long dashboard titles wrap, truncate with tooltip, or collapse predictably
  according to explicit layout rules.
- Header actions remain reachable without pushing important status text out of
  view.
- Provider/model/status/parameter controls collapse into menus or secondary
  rows when space is constrained.
- The fix preserves existing dashboard actions and does not change runtime
  behavior.

## Approach

1. Audit current header content.
   - Identify all header text/actions across normal dashboard view, Build Chat
     state, parameterized dashboards, provider/model status, version/undo, and
     Workbench links.
   - Define which items are primary, secondary, or overflow actions.

2. Implement resilient layout.
   - Use CSS grid/flex rules with stable min/max widths.
   - Let the title area own available space while action groups keep fixed or
     min-content dimensions.
   - Move secondary status/parameter controls to a second row or overflow menu
     at constrained widths.
   - Add tooltips for truncated titles and compact icon buttons where existing
     icon patterns support them.

3. Cover responsive states.
   - Test desktop, medium, and narrow app window widths.
   - Include long generated dashboard names, long parameter values, and provider
     labels.
   - Ensure text inside buttons does not overflow its button.

4. Add visual regression/manual checks.
   - Prefer component-level stories/tests if the project has an established
     path; otherwise add a narrow manual smoke checklist and keep CSS scoped.

## Files

- `src/App.tsx`
- `src/components/layout/*`
- `src/components/dashboard/*`
- `src/components/widgets/*` only if widget chrome contributes to header
  clipping
- `src/styles/*` or the relevant CSS module/global stylesheet used by the app
- `docs/RECONCILIATION_PLAN.md`
- `docs/W46_DASHBOARD_HEADER_RESILIENCE.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run typecheck`
- `bun run build`
- Manual running-app smoke:
  - open a dashboard with a very long title,
  - resize the app through desktop, medium, and narrow widths,
  - enable visible provider/model/status/parameter controls,
  - confirm no header text overlaps or becomes unusably clipped,
  - confirm primary actions remain reachable,
  - confirm dashboard runtime behavior is unchanged.

## Out of scope

- Backend model changes.
- Dashboard runtime refresh changes.
- Full app redesign.
- Rewriting navigation/sidebar layout unless the header cannot be fixed
  without a narrow adjacent adjustment.
- Changing dashboard title generation semantics.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W27_CYBERPUNK_REDESIGN.md`
- `docs/W28_CHAT_UX_HARDENING.md`
- `docs/W34_PARAMETERIZED_DATASOURCE_OPTIONS.md`
- `docs/W43_DASHBOARD_WIDGET_MODEL_SELECTION.md`
