import { useCallback, useEffect, useMemo, useState } from 'react';
import { dashboardApi } from '../../lib/api';
import type { Dashboard, DashboardVersionSource, DashboardVersionSummary } from '../../lib/api';
import { VersionDiffView } from './VersionDiffView';

interface Props {
  dashboardId: string;
  onClose: () => void;
  onRestored: (dashboard: Dashboard) => void;
}

export function HistoryDrawer({ dashboardId, onClose, onRestored }: Props) {
  const [versions, setVersions] = useState<DashboardVersionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await dashboardApi.listVersions(dashboardId);
      setVersions(data);
      if (data.length > 0 && !selectedId) {
        setSelectedId(data[0].id);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load history');
    } finally {
      setLoading(false);
    }
  }, [dashboardId, selectedId]);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  const selected = useMemo(
    () => versions.find(v => v.id === selectedId) ?? null,
    [versions, selectedId],
  );

  const handleRestore = async (versionId: string) => {
    if (!window.confirm('Restore this version? The current state is saved as a new version first.')) {
      return;
    }
    setRestoring(true);
    setError(null);
    try {
      const dashboard = await dashboardApi.restoreVersion(versionId);
      onRestored(dashboard);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Restore failed');
    } finally {
      setRestoring(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex">
      <div className="flex-1 bg-background/60 backdrop-blur-sm" onClick={onClose} />
      <aside className="flex w-[min(95vw,52rem)] flex-col border-l border-border bg-card shadow-2xl">
        <header className="flex items-center justify-between border-b border-border px-4 py-3 bg-muted/20">
          <div>
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// history</p>
            <h2 className="mt-0.5 text-sm font-semibold tracking-tight">Dashboard history</h2>
            <p className="text-[11px] text-muted-foreground">
              Snapshots before each apply, manual edit, restore, or delete.
            </p>
          </div>
          <button onClick={onClose} className="rounded p-1 hover:bg-muted">
            <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </header>

        {error && (
          <div className="border-b border-destructive/30 bg-destructive/5 px-4 py-2 text-xs text-destructive">
            {error}
          </div>
        )}

        <div className="flex min-h-0 flex-1">
          <ul className="w-72 shrink-0 overflow-auto border-r border-border">
            {loading && (
              <li className="px-3 py-2 text-xs text-muted-foreground">Loading…</li>
            )}
            {!loading && versions.length === 0 && (
              <li className="px-3 py-3 text-xs text-muted-foreground">
                No versions recorded yet. Apply a build proposal or edit the dashboard to populate history.
              </li>
            )}
            {versions.map((version, index) => {
              const isSelected = selectedId === version.id;
              const previous = versions[index + 1];
              const delta = previous ? version.widget_count - previous.widget_count : 0;
              return (
                <li key={version.id}>
                  <button
                    onClick={() => setSelectedId(version.id)}
                    className={`w-full px-3 py-2 text-left text-xs transition-colors border-l-2 ${
                      isSelected ? 'bg-primary/10 border-l-primary' : 'border-l-transparent hover:bg-muted/40'
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <SourceBadge source={version.source} />
                      <span className="text-[10px] text-muted-foreground">
                        {relativeTime(now, version.applied_at)}
                      </span>
                    </div>
                    <p className="mt-1 line-clamp-2 text-[12px] font-medium text-foreground">
                      {version.summary}
                    </p>
                    <p className="mt-0.5 text-[10px] text-muted-foreground">
                      {version.widget_count} widget(s)
                      {previous && delta !== 0 && (
                        <span className={delta > 0 ? 'text-neon-lime' : 'text-destructive'}>
                          {' '}
                          ({delta > 0 ? `+${delta}` : delta} vs prior)
                        </span>
                      )}
                    </p>
                  </button>
                </li>
              );
            })}
          </ul>

          <div className="flex min-w-0 flex-1 flex-col">
            {selected ? (
              <SelectedVersionPanel
                selected={selected}
                newestVersionId={versions[0]?.id ?? null}
                onRestore={() => handleRestore(selected.id)}
                restoring={restoring}
              />
            ) : (
              <div className="flex flex-1 items-center justify-center text-xs text-muted-foreground">
                Select a version to inspect.
              </div>
            )}
          </div>
        </div>
      </aside>
    </div>
  );
}

function SelectedVersionPanel({
  selected,
  newestVersionId,
  onRestore,
  restoring,
}: {
  selected: DashboardVersionSummary;
  newestVersionId: string | null;
  onRestore: () => void;
  restoring: boolean;
}) {
  const isNewest = newestVersionId === selected.id;
  return (
    <>
      <div className="border-b border-border px-4 py-3">
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <p className="text-sm font-medium">{selected.summary}</p>
            <p className="text-[11px] text-muted-foreground">
              <SourceLabel source={selected.source} /> · {new Date(selected.applied_at).toLocaleString()}
            </p>
          </div>
          <button
            onClick={onRestore}
            disabled={restoring}
            className="rounded-md border border-border px-3 py-1.5 text-xs font-medium hover:bg-muted disabled:opacity-50"
          >
            {restoring ? 'Restoring…' : 'Restore this version'}
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-auto p-4">
        <p className="mb-2 text-[11px] uppercase tracking-wide text-muted-foreground">
          {isNewest
            ? 'Diff vs current dashboard'
            : 'Diff to newer snapshot (what changed after this point)'}
        </p>
        {isNewest ? (
          <p className="text-xs text-muted-foreground">
            This is the latest snapshot. Diff against current state is not yet wired; restoring it reverts the most recent mutation.
          </p>
        ) : newestVersionId ? (
          <VersionDiffView fromVersionId={selected.id} toVersionId={newestVersionId} />
        ) : null}
        <p className="mt-3 text-[11px] text-muted-foreground">
          Snapshot id: <code>{selected.id.slice(0, 8)}</code>
          {selected.parent_version_id && (
            <>
              {' '}
              · parent <code>{selected.parent_version_id.slice(0, 8)}</code>
            </>
          )}
          {selected.source_session_id && (
            <>
              {' '}
              · session <code>{selected.source_session_id.slice(0, 8)}</code>
            </>
          )}
        </p>
      </div>
    </>
  );
}

function SourceBadge({ source }: { source: DashboardVersionSource }) {
  const styles: Record<DashboardVersionSource, string> = {
    agent_apply: 'bg-primary/15 text-primary border-primary/40',
    manual_edit: 'bg-neon-amber/15 text-neon-amber border-neon-amber/40',
    restore: 'bg-neon-lime/15 text-neon-lime border-neon-lime/40',
    pre_delete: 'bg-destructive/15 text-destructive border-destructive/40',
  };
  const labels: Record<DashboardVersionSource, string> = {
    agent_apply: 'Agent',
    manual_edit: 'Manual',
    restore: 'Restore',
    pre_delete: 'Pre-delete',
  };
  return (
    <span className={`rounded-sm border px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider ${styles[source]}`}>
      {labels[source]}
    </span>
  );
}

function SourceLabel({ source }: { source: DashboardVersionSource }) {
  switch (source) {
    case 'agent_apply':
      return <>Agent apply</>;
    case 'manual_edit':
      return <>Manual edit</>;
    case 'restore':
      return <>Restore</>;
    case 'pre_delete':
      return <>Pre-delete</>;
  }
}

function relativeTime(now: number, ts: number): string {
  const delta = Math.max(0, now - ts);
  if (delta < 5_000) return 'just now';
  if (delta < 60_000) return `${Math.round(delta / 1000)}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}
