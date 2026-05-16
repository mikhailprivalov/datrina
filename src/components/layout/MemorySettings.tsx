import { useEffect, useMemo, useState } from 'react';
import { memoryApi } from '../../lib/api';
import type { MemoryKind, MemoryRecord, MemoryScope } from '../../lib/api';

interface Props {
  onClose: () => void;
}

function describeScope(scope: MemoryScope): string {
  switch (scope.kind) {
    case 'global':
      return 'global';
    case 'dashboard':
      return `dashboard:${shortId(scope.id)}`;
    case 'mcp_server':
      return `mcp:${scope.id}`;
    case 'session':
      return `session:${shortId(scope.id)}`;
  }
}

function shortId(id: string): string {
  return id.length <= 8 ? id : `${id.slice(0, 8)}...`;
}

function kindTone(kind: MemoryKind): string {
  switch (kind) {
    case 'lesson':
      return 'text-purple-600 dark:text-purple-400 bg-purple-500/15';
    case 'preference':
      return 'text-emerald-600 dark:text-emerald-400 bg-emerald-500/15';
    case 'tool_shape':
      return 'text-blue-700 dark:text-blue-400 bg-blue-500/15';
    default:
      return 'text-amber-700 dark:text-amber-400 bg-amber-500/15';
  }
}

export function MemorySettings({ onClose }: Props) {
  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [filter, setFilter] = useState<'all' | MemoryKind>('all');
  const [searchQuery, setSearchQuery] = useState('');

  const refresh = async () => {
    try {
      const list = await memoryApi.list();
      setRecords(list);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const grouped = useMemo(() => {
    const filtered = records.filter(record => {
      if (filter !== 'all' && record.kind !== filter) return false;
      if (searchQuery.trim()) {
        const lower = searchQuery.trim().toLowerCase();
        if (!record.content.toLowerCase().includes(lower)) return false;
      }
      return true;
    });
    const map = new Map<string, MemoryRecord[]>();
    for (const record of filtered) {
      const key = describeScope(record.scope);
      const list = map.get(key) ?? [];
      list.push(record);
      map.set(key, list);
    }
    return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b));
  }, [records, filter, searchQuery]);

  const handleDelete = async (record: MemoryRecord) => {
    if (!window.confirm(`Forget this memory? "${record.content.slice(0, 80)}"`)) return;
    setBusyId(record.id);
    setError(null);
    try {
      await memoryApi.remove(record.id);
      setStatus('Forgotten');
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex max-h-[85vh] w-[min(92vw,56rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-3">
          <div>
            <h2 className="text-sm font-semibold text-foreground">Agent memory</h2>
            <p className="text-[11px] text-muted-foreground">
              Facts, preferences, lessons, and MCP tool shapes the agent has learned across sessions.
            </p>
          </div>
          <button onClick={onClose} className="p-1 rounded hover:bg-muted text-muted-foreground">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <div className="flex items-center gap-2 border-b border-border px-5 py-2">
          <select
            value={filter}
            onChange={event => setFilter(event.target.value as 'all' | MemoryKind)}
            className="rounded-md border border-border bg-background px-2 py-1 text-xs"
          >
            <option value="all">All kinds</option>
            <option value="fact">Facts</option>
            <option value="preference">Preferences</option>
            <option value="lesson">Lessons</option>
            <option value="tool_shape">Tool shapes</option>
          </select>
          <input
            value={searchQuery}
            onChange={event => setSearchQuery(event.target.value)}
            placeholder="Filter by substring..."
            className="flex-1 rounded-md border border-border bg-background px-2 py-1 text-xs"
          />
          <button
            onClick={refresh}
            className="rounded-md border border-border px-2 py-1 text-xs hover:bg-muted"
          >
            Refresh
          </button>
        </div>

        {(status || error) && (
          <div className={`border-b border-border px-5 py-2 text-xs ${error ? 'text-destructive' : 'text-emerald-600 dark:text-emerald-400'}`}>
            {error ?? status}
          </div>
        )}

        <div className="flex-1 overflow-auto p-5 space-y-4">
          {grouped.length === 0 && (
            <div className="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
              No memories yet. The agent will start saving facts and tool shapes as you chat with it.
            </div>
          )}
          {grouped.map(([scopeLabel, items]) => (
            <div key={scopeLabel} className="rounded-lg border border-border bg-background/50">
              <div className="border-b border-border/60 px-3 py-1.5 text-[11px] uppercase tracking-wide text-muted-foreground font-mono">
                {scopeLabel} · {items.length}
              </div>
              <ul className="divide-y divide-border/40">
                {items.map(record => (
                  <li key={record.id} className="flex items-start gap-3 px-3 py-2">
                    <span className={`shrink-0 rounded-full px-1.5 py-0.5 text-[10px] uppercase tracking-wide ${kindTone(record.kind)}`}>
                      {record.kind}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="whitespace-pre-wrap break-words text-xs text-foreground">
                        {record.content}
                      </p>
                      <p className="mt-1 text-[10px] text-muted-foreground">
                        recalled {record.accessed_count}× · created {new Date(record.created_at).toLocaleString()}
                      </p>
                    </div>
                    <button
                      onClick={() => handleDelete(record)}
                      disabled={busyId === record.id}
                      className="shrink-0 rounded-md border border-destructive/30 text-destructive px-2 py-1 text-[11px] hover:bg-destructive/10 disabled:opacity-50"
                    >
                      {busyId === record.id ? '...' : 'Forget'}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
