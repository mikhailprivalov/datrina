# W27 Cyberpunk UI Redesign

Status: shipped

Date: 2026-05-17

## Context

Datrina is now functionally broad enough that the default warm neutral UI
undersells the product. The app should feel like a focused local AI operations
console: dense, technical, dark-first, and visually distinct, without turning
the dashboard into a decorative landing page or breaking the operator workflow.

This workstream is a frontend-only visual redesign. It must not change the Rust
runtime, Tauri command contracts, chat/tool semantics, workflow execution, data
models, storage, or provider behavior.

## Goal

Ship a polished cyberpunk-inspired interface that is good but bounded:

- Dark-first control-room visual language with deep neutral surfaces, electric
  cyan/magenta/lime/amber accents, sharper panel hierarchy, and better
  data-widget affordances.
- Existing dashboard, chat, settings, playground, alerts, memory, costs,
  history, and debug views remain usable and recognizable.
- Widget charts, tables, logs, status grids, proposal previews, validation
  blocks, and tool traces look intentionally themed instead of inheriting the
  old warm palette.
- Text stays readable, dense UI stays scannable, focus states stay obvious,
  error/warning/success states stay semantically distinct.
- Motion and glow effects are subtle and disabled or reduced through
  `prefers-reduced-motion`.

## Approach

1. Refresh the design token layer first.
   - Update `src/index.css` CSS variables for dark and light themes.
   - Keep the existing Tailwind class contract intact.
   - Synchronize `tailwind.config.ts` chart/accent tokens so Recharts and
     utility classes do not keep the old warm palette.
   - Prefer tokenized color and shadow utilities over component-local one-off
     hex values.

2. Redesign the app shell.
   - Update `App`, `Sidebar`, `TopBar`, and `StatusBar` surfaces for a compact
     console feel: darker chrome, neon focus rings, active-route signal,
     readable status/busy/error states, and collapsed-sidebar polish.
   - Keep the existing route structure and controls. Do not add a landing page,
     marketing hero, or onboarding detour.

3. Redesign dashboard and widget surfaces.
   - Update `DashboardGrid`, `ParameterBar`, widget cards, drag/resize handles,
     widget menus, alert badges, empty/loading/error states, and debug/inspect
     affordances.
   - Update `src/components/widgets/*`, especially `ChartWidget`, so charts,
     gauges, bars, heatmaps, logs, status grids, tables, and text widgets share
     the new palette and preserve readable data density.
   - Keep the 12-column grid and existing auto-pack/runtime behavior unchanged.

4. Redesign the chat and agent-observability surface without touching runtime
   semantics.
   - Restyle `ChatPanel` messages, typed message parts, reasoning trace, tool
     call/result blocks, plan artifacts, proposal previews, dry-run evidence,
     and validation issue tiles.
   - Keep Build proposal apply confirmation, streaming state, cancellation,
     validation retry display, and tool trace masking behavior unchanged.

5. Sweep secondary product surfaces.
   - Apply the same style system to `ProviderSettings`, `McpSettings`,
     `MemorySettings`, `CostsView`, `TemplateGallery`, `Playground`,
     `AlertsView`, `AlertEditorModal`, `HistoryDrawer`, and
     `PipelineDebugModal`.
   - Avoid nested-card inflation. Use panels, separators, tabs, badges, and
     compact controls where they fit the operational workflow.

6. Visual accessibility and motion guardrails.
   - Keep normal text at WCAG AA contrast or better against its actual
     background.
   - Preserve visible keyboard focus for buttons, inputs, menus, tabs, and
     draggable controls.
   - Use glow, scanline, shimmer, or animated effects only where they add state
     clarity. Gate non-essential animation behind `prefers-reduced-motion`.
   - Check narrow desktop widths and collapsed sidebar states for text
     clipping, overlap, and controls that shift layout.

## Files

- `src/index.css`
- `tailwind.config.ts`
- `src/App.tsx`
- `src/components/layout/Sidebar.tsx`
- `src/components/layout/TopBar.tsx`
- `src/components/layout/StatusBar.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/dashboard/ParameterBar.tsx`
- `src/components/dashboard/HistoryDrawer.tsx`
- `src/components/dashboard/VersionDiffView.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/ProviderSettings.tsx`
- `src/components/layout/McpSettings.tsx`
- `src/components/layout/MemorySettings.tsx`
- `src/components/layout/CostsView.tsx`
- `src/components/widgets/*`
- `src/components/onboarding/TemplateGallery.tsx`
- `src/components/playground/*`
- `src/components/alerts/*`
- `src/components/debug/PipelineDebugModal.tsx`

Do not edit `src-tauri/**`, `src/lib/api.ts`, `src-tauri/src/models/**`, or
Tauri command request/response/event shapes for this workstream.

## Validation

- `bun run typecheck`
- `bun run build`
- `bun run check:contract` as a safety check; it should remain green because
  this task must not change frontend/Rust command contracts.
- Visual smoke in the running app:
  - dashboard with widgets,
  - empty dashboard/template gallery,
  - chat drawer in Context and Build modes,
  - proposal preview and validation blocks when available,
  - provider, MCP, memory, costs, playground, alerts, history, and debug modal
    surfaces,
  - collapsed sidebar,
  - narrow desktop width.
- Screenshot evidence for at least one normal dashboard, chat drawer, and one
  modal/panel state.
- Rust checks are not required unless Rust files are changed. If any Rust file
  changes, run the relevant Rust validation and explain why the frontend-only
  boundary was crossed.

## Out of scope

- Rust backend, storage, scheduler, workflow, provider, MCP, validation, or chat
  runtime changes.
- `src/lib/api.ts` or shared request/response/model contract changes.
- New frontend dependencies, icon libraries, component frameworks, chart
  libraries, 3D scenes, canvas backgrounds, or generated image assets unless a
  follow-up decision explicitly approves them.
- Full design system extraction or Storybook setup.
- Production packaging, signing, release assets, app icon work, or marketing
  website work.
- Heavy animation, decorative gradient orbs, bokeh backgrounds, or effects that
  reduce data readability.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W23_PIPELINE_DEBUG_VIEW.md`
- `docs/W25_DASHBOARD_PARAMETERS.md`
- `docs/W26_CAO_AUTOPILOT_E2E.md`
- WCAG 2.2 Success Criterion 1.4.3 Contrast (Minimum):
  https://www.w3.org/TR/WCAG22/#contrast-minimum
- MDN `prefers-reduced-motion`:
  https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-reduced-motion
