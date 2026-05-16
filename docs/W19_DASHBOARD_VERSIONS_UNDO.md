# W19 Dashboard Versions And Undo

Status: shipped

Date: 2026-05-16

## Context

`apply_build_proposal` (`commands/dashboard.rs:447-548`) mutates the
dashboard in place. There is no snapshot, no version row, no undo. A bad
agent proposal that overwrites a carefully edited dashboard is lost
unless the user happens to have an external backup. This is the
single biggest reason users hesitate to let the agent act boldly.

## Goal

Every state-changing dashboard mutation (Apply, restore, manual edit)
writes a versioned snapshot first. The user can list, diff, and restore
prior versions from the dashboard header. Apply becomes psychologically
free.

## Approach

### Schema (new migration)

```sql
CREATE TABLE dashboard_versions (
  id TEXT PRIMARY KEY,
  dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
  snapshot_json TEXT NOT NULL,            -- full Dashboard struct as of save time
  applied_at INTEGER NOT NULL,
  source TEXT NOT NULL,                   -- 'agent_apply' | 'manual_edit' | 'restore' | 'pre_delete'
  source_session_id TEXT,                 -- nullable; chat session id if source = agent_apply
  summary TEXT NOT NULL,                  -- 1-line description
  widget_count INTEGER NOT NULL,
  parent_version_id TEXT                  -- nullable; previous version
);
CREATE INDEX dashboard_versions_dashboard_idx
  ON dashboard_versions (dashboard_id, applied_at DESC);
```

Ring-buffer prune: keep the last `MAX_VERSIONS_PER_DASHBOARD = 30`.
Older rows deleted in a transaction tail of every snapshot write.

### Write path

Every mutation goes through a single helper:

```rust
async fn snapshot_then_apply<F, T>(
    storage: &Storage,
    dashboard_id: &str,
    source: VersionSource,
    summary: &str,
    op: F,
) -> Result<T>
where F: FnOnce(&mut Dashboard) -> Result<T>
```

`snapshot_then_apply` reads the current dashboard, serialises it as
`snapshot_json`, inserts a `dashboard_versions` row, then runs `op` on
the in-memory clone and persists the result. Both writes share a single
transaction.

Call sites:

- `apply_build_proposal_inner` (source = `agent_apply`,
  source_session_id = chat session id).
- `update_dashboard` (source = `manual_edit`).
- `delete_dashboard` (source = `pre_delete`) — final snapshot before
  cascade delete; lets a user undo an accidental delete by restoring a
  parent dashboard manually (advanced flow, optional in v1).
- `restore_dashboard_version` (source = `restore`, parent_version_id
  set to the version being restored).

### Read path / Tauri commands

New commands registered in `lib.rs`:

- `list_dashboard_versions(dashboard_id: String) -> Vec<DashboardVersionSummary>` —
  summaries only (`id, applied_at, source, summary, widget_count,
  source_session_id`).
- `get_dashboard_version(version_id: String) -> DashboardVersion` —
  includes full `snapshot_json`.
- `diff_dashboard_versions(from_id: String, to_id: String) -> DashboardDiff` —
  returns added/removed/modified widgets, optionally including title /
  layout changes; computed in Rust against parsed snapshots.
- `restore_dashboard_version(version_id: String) -> Dashboard` — wraps the
  restore in `snapshot_then_apply` itself so the act of restoring is also
  a version.

### UI

#### `DashboardGrid.tsx` header

New "History" icon next to existing actions. Opens a side drawer:

- Vertical list of versions, newest first.
- Each row: source badge (Agent / Manual / Restore), relative timestamp,
  1-line summary, widget count delta vs. previous (`+2 / -1`).
- Click a row → modal with:
  - Left: current dashboard preview.
  - Right: selected version preview (read-only, no live data — uses
    cached widget configs only).
  - Buttons: **Restore this version**, **Diff** (opens the structured
    `DashboardDiff` as a readable list).
- For agent-apply versions, link to the originating chat session via
  `source_session_id` (clicking jumps to ChatPanel filtered to that
  session).

#### Toast on Apply

After every Apply, show a transient toast: `Applied. ↩ Undo` for 10 s.
"Undo" calls `restore_dashboard_version` on the parent version. Plain
ergonomic UX win for the most common flow.

### Diff format

`DashboardDiff`:

```rust
pub struct DashboardDiff {
    pub added_widgets: Vec<WidgetId>,
    pub removed_widgets: Vec<WidgetId>,
    pub modified_widgets: Vec<WidgetDiff>,
    pub title_changed: Option<(String, String)>,
    pub layout_changed: bool,
}

pub struct WidgetDiff {
    pub widget_id: WidgetId,
    pub kind_changed: Option<(String, String)>,
    pub config_changes: Vec<JsonPathChange>,    // dotted path + (before, after) JSON values
    pub datasource_plan_changed: bool,
}
```

Computed via `serde_json::Value` walk; not a binary diff.

## Files to touch

- `src-tauri/src/modules/storage.rs` — migration + version CRUD.
- `src-tauri/src/commands/dashboard.rs` — `snapshot_then_apply` helper;
  call sites; version Tauri commands.
- `src-tauri/src/models/dashboard.rs` — `DashboardVersion`,
  `DashboardVersionSummary`, `DashboardDiff`, `WidgetDiff`,
  `VersionSource` enum.
- `src-tauri/src/lib.rs` — register new commands.
- `src/lib/api.ts` — mirror types, `dashboardApi.listVersions`,
  `getVersion`, `diffVersions`, `restoreVersion`.
- `src/components/layout/DashboardGrid.tsx` — History button + drawer.
- `src/components/dashboard/HistoryDrawer.tsx` (new).
- `src/components/dashboard/VersionDiffView.tsx` (new).

## Validation

- `bun run check:contract`.
- `cargo check --workspace --all-targets`.
- Manual: apply a proposal → open History → see 1 row → apply another →
  see 2 rows → Restore the first → see 3 rows (restore-as-version) →
  state matches.
- Manual: spam Apply → verify ring buffer caps at 30.
- Manual: click "Undo" toast after Apply within 10 s → dashboard reverts.
- Edge: dashboard with 200+ widgets — snapshot write under 100 ms (JSON
  size acceptable; if not, add zstd compression in a follow-up).

## Out of scope

- Cross-dashboard versioning / branching.
- Operational transform / live collaborative editing.
- Per-widget version history independent of dashboard.
- Compressed snapshots (json-only in v1; revisit if storage cost matters).

## Related

- W18 reflection can suggest restoring a prior version when its critique
  finds the new state worse than the previous one — link
  `parent_version_id` for the heuristic.
- W22 cost tracking should attribute version creation to the chat session
  that caused it (already wired via `source_session_id`).
