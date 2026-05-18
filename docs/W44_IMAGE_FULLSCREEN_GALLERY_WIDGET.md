# W44 Image Fullscreen And Gallery Widget

Status: shipped

Date: 2026-05-17

## Implementation notes

- New `Widget::Gallery` variant with typed `GalleryConfig`
  (layout/aspect/max items/caption/source/fullscreen/fit) and matching
  `BuildWidgetType::Gallery` so Build proposals carry it end-to-end.
- Runtime coercion in `widget_runtime_data_strict` accepts a string
  array, an object envelope with `items` / `images`, or a single
  object. Each item normalises to `{ src, title?, caption?, alt?,
  source?, link?, id? }`; items without a usable image source are
  dropped, and `max_visible_items` caps the array.
- `ValidationIssue::HardcodedGalleryItems` rejects literal `data`
  image arrays without a pipeline producing them; gallery widgets also
  require a `dry_run_widget` evidence record like stat/gauge/table.
- Frontend: new `GalleryWidget` (grid/row/masonry) and reusable
  `ImageLightbox` with focus trap, ESC/←→/PgUp/PgDn navigation,
  `f` to cycle fit (contain/cover/fill), broken-image fallback. The
  existing `ImageWidget` now opens the same lightbox on click.
- Build chat system prompt updated with a gallery section + pipeline
  recipe; templates registry seeds a Wikipedia REST gallery template
  for the empty-state Gallery.
- Tests: `gallery_tests` (6 cases) cover string/object coercion,
  envelope unwrap, max cap, dropped sourceless items, and the
  text-fallback when nothing is usable. `validate_build_proposal`
  tests cover both the hardcoded-array reject and the
  pipeline-present allow.

## Context

Image-oriented dashboards need two missing basics: users must be able to open
an image fullscreen, and the dashboard needs a first-class gallery widget for
multiple images. This should be backed by datasource/pipeline output, not by
hardcoded demo image lists.

## Goal

- Image widgets support fullscreen viewing with keyboard and pointer controls.
- A new `gallery` widget type renders a datasource-backed list of image items.
- Gallery items support image URL/path, title/caption, source metadata, and
  optional link/action fields.
- The gallery handles loading, empty, error, broken image, and stale snapshot
  states.
- Build Chat can propose gallery widgets using typed schema and validation.
- Fullscreen viewing works for image widgets and gallery items.

## Approach

1. Define gallery widget schema.
   - Add a `gallery` widget kind with typed config for layout mode, thumbnail
     aspect ratio, max visible items, caption fields, and fullscreen behavior.
   - Define runtime item shape with image source, alt/title/caption, metadata,
     and optional link.
   - Validate that gallery values come from datasource pipeline output rather
     than hardcoded demo image arrays.

2. Add pipeline mapping support.
   - Ensure PipelineStep DSL can map common API/MCP responses into gallery
     item arrays.
   - Provide deterministic templates/examples for image search, media APIs, RSS
     enclosures, GitHub assets, or other enabled sources where applicable.
   - Keep `llm_postprocess` last-resort only for shapes typed steps cannot
     produce.

3. Build fullscreen viewer.
   - Add a reusable fullscreen/lightbox component with close, next/previous,
     keyboard navigation, focus trap, image fit modes, and caption/source
     display.
   - Do not use OS shell open for normal fullscreen viewing.
   - Keep local filesystem and remote URL handling within existing safety
     policy.

4. Integrate widget rendering.
   - Add gallery rendering in dashboard grid.
   - Support snapshots for the latest successful gallery runtime value.
   - Add broken-image fallback and per-item loading state without resizing the
     grid unpredictably.

5. Update Build Chat and validation.
   - Teach Build proposal schema/system prompt about `gallery`.
   - Validate item mappings and reject hardcoded placeholder image lists.
   - Add preview/apply support and widget details/provenance integration.

## Files

- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/modules/storage.rs`
- `src/lib/api.ts`
- `src/lib/templates/index.ts`
- `src/App.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/widgets/*`
- `src/components/layout/ChatPanel.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W44_IMAGE_FULLSCREEN_GALLERY_WIDGET.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - gallery widget schema serialization/parity,
  - validation rejecting hardcoded placeholder image arrays,
  - pipeline output coercion into gallery item array,
  - broken/missing image item state,
  - fullscreen viewer keyboard navigation.
- Manual running-app smoke:
  - create an image widget and open it fullscreen,
  - create a datasource-backed gallery widget,
  - navigate gallery items in fullscreen with keyboard and mouse,
  - reload the app and confirm gallery snapshot/stale behavior is honest,
  - confirm Build Chat can propose a gallery that applies only after preview
    validation passes.

## Out of scope

- Image generation.
- Image editing/cropping tooling.
- Cloud media library management.
- Infinite masonry virtualization for huge media collections.
- Storing remote image binaries in the local database unless a later cache
  policy explicitly allows it.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`
- `docs/W36_WIDGET_RUNTIME_SNAPSHOTS.md`
- `docs/W37_EXTERNAL_OPEN_SOURCE_CATALOG.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`
