import { useEffect, useState } from 'react';
import { dashboardApi } from '../../lib/api';
import type { DashboardDiff } from '../../lib/api';

interface Props {
  fromVersionId: string;
  toVersionId: string;
}

export function VersionDiffView({ fromVersionId, toVersionId }: Props) {
  const [diff, setDiff] = useState<DashboardDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setError(null);
    dashboardApi
      .diffVersions(fromVersionId, toVersionId)
      .then(result => {
        setDiff(result);
      })
      .catch(err => {
        setError(err instanceof Error ? err.message : 'Failed to compute diff');
      })
      .finally(() => setLoading(false));
  }, [fromVersionId, toVersionId]);

  if (loading) {
    return <p className="text-xs text-muted-foreground">Computing diff…</p>;
  }
  if (error) {
    return <p className="text-xs text-destructive">{error}</p>;
  }
  if (!diff) {
    return null;
  }

  const noChanges =
    diff.added_widgets.length === 0 &&
    diff.removed_widgets.length === 0 &&
    diff.modified_widgets.length === 0 &&
    !diff.name_changed &&
    !diff.description_changed &&
    !diff.layout_changed;

  if (noChanges) {
    return <p className="text-xs text-muted-foreground">No structural differences.</p>;
  }

  return (
    <div className="space-y-3 text-xs">
      {diff.name_changed && (
        <DiffRow label="Dashboard name" before={diff.name_changed[0]} after={diff.name_changed[1]} />
      )}
      {diff.description_changed && (
        <DiffRow
          label="Description"
          before={diff.description_changed[0] ?? '(none)'}
          after={diff.description_changed[1] ?? '(none)'}
        />
      )}
      {diff.layout_changed && (
        <p className="text-amber-600 dark:text-amber-400">Layout positions changed.</p>
      )}

      {diff.added_widgets.length > 0 && (
        <DiffSection title={`Added (${diff.added_widgets.length})`} tone="add">
          {diff.added_widgets.map(w => (
            <li key={w.id} className="font-mono">
              + <span className="font-medium not-italic">{w.title}</span>{' '}
              <span className="text-muted-foreground">[{w.kind}]</span>
            </li>
          ))}
        </DiffSection>
      )}

      {diff.removed_widgets.length > 0 && (
        <DiffSection title={`Removed (${diff.removed_widgets.length})`} tone="remove">
          {diff.removed_widgets.map(w => (
            <li key={w.id} className="font-mono">
              − <span className="font-medium not-italic">{w.title}</span>{' '}
              <span className="text-muted-foreground">[{w.kind}]</span>
            </li>
          ))}
        </DiffSection>
      )}

      {diff.modified_widgets.length > 0 && (
        <DiffSection title={`Modified (${diff.modified_widgets.length})`} tone="modify">
          {diff.modified_widgets.map(w => (
            <li key={w.widget_id} className="space-y-1">
              <div>
                <span className="font-medium">{w.widget_title}</span>{' '}
                <span className="text-muted-foreground">[{w.kind_changed?.[1] ?? '—'}]</span>
              </div>
              {w.title_changed && (
                <p className="ml-3 text-muted-foreground">
                  Title: <code>{w.title_changed[0]}</code> → <code>{w.title_changed[1]}</code>
                </p>
              )}
              {w.kind_changed && (
                <p className="ml-3 text-muted-foreground">
                  Kind: <code>{w.kind_changed[0]}</code> → <code>{w.kind_changed[1]}</code>
                </p>
              )}
              {w.datasource_plan_changed && (
                <p className="ml-3 text-muted-foreground">Datasource plan changed.</p>
              )}
              {w.config_changes.length > 0 && (
                <ul className="ml-3 space-y-0.5 font-mono text-[10px]">
                  {w.config_changes.slice(0, 8).map((change, idx) => (
                    <li key={`${change.path}-${idx}`} className="break-all">
                      <span className="text-muted-foreground">{change.path}:</span>{' '}
                      <span className="text-destructive">{summarize(change.before)}</span>{' '}
                      → <span className="text-emerald-700 dark:text-emerald-400">{summarize(change.after)}</span>
                    </li>
                  ))}
                  {w.config_changes.length > 8 && (
                    <li className="text-muted-foreground">
                      …{w.config_changes.length - 8} more
                    </li>
                  )}
                </ul>
              )}
            </li>
          ))}
        </DiffSection>
      )}
    </div>
  );
}

function DiffSection({
  title,
  tone,
  children,
}: {
  title: string;
  tone: 'add' | 'remove' | 'modify';
  children: React.ReactNode;
}) {
  const heading =
    tone === 'add'
      ? 'text-emerald-700 dark:text-emerald-400'
      : tone === 'remove'
        ? 'text-destructive'
        : 'text-amber-700 dark:text-amber-400';
  return (
    <div>
      <p className={`mb-1 text-[11px] font-medium uppercase tracking-wide ${heading}`}>{title}</p>
      <ul className="space-y-1">{children}</ul>
    </div>
  );
}

function DiffRow({ label, before, after }: { label: string; before: string; after: string }) {
  return (
    <div>
      <p className="text-[11px] uppercase tracking-wide text-muted-foreground">{label}</p>
      <p className="break-all font-mono">
        <span className="text-destructive line-through">{before}</span>{' '}
        → <span className="text-emerald-700 dark:text-emerald-400">{after}</span>
      </p>
    </div>
  );
}

function summarize(value: unknown): string {
  if (value === null || value === undefined) return 'null';
  if (typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  const serialized = JSON.stringify(value);
  if (serialized.length > 60) return `${serialized.slice(0, 57)}…`;
  return serialized;
}
