import { useMemo, useState } from 'react';
import type { LogsConfig, LogsWidgetRuntimeData, LogEntry } from '../../lib/api';

interface Props {
  config: LogsConfig;
  data?: LogsWidgetRuntimeData;
}

const LEVEL_COLORS: Record<string, string> = {
  debug: 'text-muted-foreground',
  info: 'text-blue-600',
  warn: 'text-amber-600',
  warning: 'text-amber-600',
  error: 'text-destructive',
  fatal: 'text-destructive',
  critical: 'text-destructive',
};

export function LogsWidget({ config, data }: Props) {
  const [query, setQuery] = useState('');
  const filtered = useMemo(() => {
    const entries = data?.entries ?? [];
    const limit = config.max_entries ?? 200;
    const trimmed = entries.slice(-limit);
    const ordered = config.reverse ? trimmed : [...trimmed].reverse();
    if (!query.trim()) return ordered;
    const q = query.toLowerCase();
    return ordered.filter(e => JSON.stringify(e).toLowerCase().includes(q));
  }, [data, config.max_entries, config.reverse, query]);

  if (!data) {
    return <div className="flex h-full items-center justify-center text-xs text-muted-foreground">No log entries</div>;
  }

  return (
    <div className="flex h-full flex-col">
      <div className="px-1 pb-1.5">
        <input
          value={query}
          onChange={e => setQuery(e.target.value)}
          placeholder="Filter logs..."
          className="w-full rounded-md border border-border/60 bg-background/80 px-2 py-1 text-[11px] focus:outline-none focus:ring-1 focus:ring-primary/30"
        />
      </div>
      <div className="flex-1 overflow-auto rounded-md border border-border/60 bg-background/70 font-mono text-[11px] leading-tight">
        {filtered.length === 0 ? (
          <p className="p-2 text-muted-foreground">No matching entries.</p>
        ) : (
          <ul className="divide-y divide-border/30">
            {filtered.map((entry, index) => (
              <LogRow key={index} entry={entry} config={config} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function LogRow({ entry, config }: { entry: LogEntry; config: LogsConfig }) {
  const level = (entry.level ?? '').toLowerCase();
  const levelColor = LEVEL_COLORS[level] ?? 'text-foreground';
  const wrap = config.wrap ?? false;
  return (
    <li className={`flex items-baseline gap-2 px-2 py-1 ${wrap ? '' : 'whitespace-nowrap overflow-hidden'}`}>
      {config.show_timestamp !== false && entry.ts !== undefined && (
        <span className="flex-shrink-0 text-muted-foreground tabular-nums">{formatTs(entry.ts)}</span>
      )}
      {config.show_level !== false && level && (
        <span className={`flex-shrink-0 uppercase text-[9px] font-semibold tracking-wide ${levelColor}`}>{level.slice(0, 4)}</span>
      )}
      {entry.source && (
        <span className="flex-shrink-0 text-muted-foreground">[{entry.source}]</span>
      )}
      <span className={`${wrap ? 'whitespace-pre-wrap break-words' : 'truncate'} text-foreground`}>
        {entry.message ?? JSON.stringify(entry)}
      </span>
    </li>
  );
}

function formatTs(ts: string | number): string {
  if (typeof ts === 'number') {
    const d = new Date(ts);
    if (!Number.isNaN(d.getTime())) {
      return d.toISOString().slice(11, 19);
    }
  }
  if (typeof ts === 'string') {
    const d = new Date(ts);
    if (!Number.isNaN(d.getTime())) {
      return d.toISOString().slice(11, 19);
    }
    return ts.slice(0, 19);
  }
  return String(ts);
}
