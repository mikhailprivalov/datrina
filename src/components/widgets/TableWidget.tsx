import { useState } from 'react';
import { LineChart, Line, ResponsiveContainer } from 'recharts';
import type { GaugeThreshold, TableColumn, TableColumnFormat, TableConfig, TableWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: TableConfig;
  data?: TableWidgetRuntimeData;
}

const DEFAULT_STATUS_COLORS: Record<string, string> = {
  ok: '#10b981',
  up: '#10b981',
  healthy: '#10b981',
  success: '#10b981',
  active: '#10b981',
  warning: '#f59e0b',
  warn: '#f59e0b',
  degraded: '#f59e0b',
  pending: '#f59e0b',
  error: '#ef4444',
  down: '#ef4444',
  failed: '#ef4444',
  critical: '#ef4444',
  unknown: '#94a3b8',
};

export function TableWidget({ config, data }: Props) {
  const { columns, page_size = 10, sortable = true } = config;
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');
  const rows = data?.rows ?? [];

  const cols: TableColumn[] = columns.length > 0 ? columns : inferColumns(rows);

  const sorted = [...rows].sort((a, b) => {
    if (!sortKey || !sortable) return 0;
    const av = a[sortKey];
    const bv = b[sortKey];
    if (typeof av === 'number' && typeof bv === 'number') {
      return sortDir === 'asc' ? av - bv : bv - av;
    }
    const as = String(av ?? '');
    const bs = String(bv ?? '');
    return sortDir === 'asc' ? as.localeCompare(bs) : bs.localeCompare(as);
  });

  if (rows.length === 0 || cols.length === 0) {
    return (
      <div className="flex h-full min-h-24 items-center justify-center text-center text-xs text-muted-foreground">
        Table data unavailable
      </div>
    );
  }

  return (
    <div className="w-full h-full overflow-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border">
            {cols.map(col => (
              <th
                key={col.key}
                style={col.width ? { width: col.width } : undefined}
                onClick={() => { if (sortable) { setSortKey(sk => sk === col.key ? null : col.key); setSortDir(d => d === 'asc' ? 'desc' : 'asc'); }}}
                className={`text-left py-1.5 px-2 font-medium text-muted-foreground ${sortable ? 'cursor-pointer hover:text-foreground' : ''}`}
              >
                <div className="flex items-center gap-1">
                  {col.header}
                  {sortable && sortKey === col.key && (
                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d={sortDir === 'asc' ? 'M5 15l7-7 7 7' : 'M19 9l-7 7-7-7'} />
                    </svg>
                  )}
                </div>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {sorted.slice(0, page_size).map((row, i) => (
            <tr key={i} className="border-b border-border/50 hover:bg-muted/30 transition-colors">
              {cols.map(col => (
                <td key={col.key} className="py-1.5 px-2 align-middle">
                  <CellRenderer value={row[col.key]} column={col} row={row} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function CellRenderer({
  value,
  column,
  row,
}: {
  value: unknown;
  column: TableColumn;
  row: Record<string, unknown>;
}) {
  const format = column.format ?? 'text';
  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">-</span>;
  }
  switch (format) {
    case 'status': {
      const key = String(value).toLowerCase();
      const colors = { ...DEFAULT_STATUS_COLORS, ...(column.status_colors ?? {}) };
      const color = colors[key] ?? colors[String(value)] ?? '#94a3b8';
      return (
        <span
          className="inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide"
          style={{ backgroundColor: withAlpha(color, 0.16), color }}
        >
          <span className="h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} />
          {String(value)}
        </span>
      );
    }
    case 'badge': {
      return (
        <span className="inline-block rounded-full bg-muted/60 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-foreground">
          {String(value)}
        </span>
      );
    }
    case 'progress': {
      const num = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(num)) return <span>{String(value)}</span>;
      const ratio = Math.max(0, Math.min(1, num / 100));
      const color = pickColor(num, column.thresholds);
      return (
        <div className="flex items-center gap-2">
          <div className="relative h-2 w-20 overflow-hidden rounded-full bg-muted">
            <div className="absolute inset-y-0 left-0" style={{ width: `${ratio * 100}%`, backgroundColor: color }} />
          </div>
          <span className="tabular-nums text-[11px] text-muted-foreground">{num.toFixed(0)}%</span>
        </div>
      );
    }
    case 'link': {
      const href = column.link_template
        ? column.link_template.replace(/\{([^}]+)\}/g, (_, k) => String(row[k] ?? ''))
        : String(value);
      return (
        <a href={href} target="_blank" rel="noopener noreferrer" className="text-primary underline">
          {String(value)}
        </a>
      );
    }
    case 'sparkline': {
      const arr = Array.isArray(value) ? value : null;
      if (!arr || arr.length < 2) return <span className="text-muted-foreground">-</span>;
      const series = arr.map((v: unknown, i: number) => ({ i, v: typeof v === 'number' ? v : Number(v) || 0 }));
      return (
        <div className="h-6 w-24">
          <ResponsiveContainer width="100%" height="100%">
            <LineChart data={series}>
              <Line type="monotone" dataKey="v" stroke="currentColor" strokeWidth={1.25} dot={false} isAnimationActive={false} />
            </LineChart>
          </ResponsiveContainer>
        </div>
      );
    }
    case 'number': {
      const num = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(num)) return <span>{String(value)}</span>;
      return <span className="tabular-nums">{num.toLocaleString()}</span>;
    }
    case 'currency': {
      const num = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(num)) return <span>{String(value)}</span>;
      return <span className="tabular-nums">{new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(num)}</span>;
    }
    case 'percent': {
      const num = typeof value === 'number' ? value : Number(value);
      if (!Number.isFinite(num)) return <span>{String(value)}</span>;
      return <span className="tabular-nums">{num.toFixed(1)}%</span>;
    }
    case 'date': {
      const d = typeof value === 'number' ? new Date(value) : new Date(String(value));
      if (Number.isNaN(d.getTime())) return <span>{String(value)}</span>;
      return <span className="tabular-nums">{d.toLocaleString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>;
    }
    case 'text':
    default:
      return <span className="text-foreground/90">{String(value)}</span>;
  }
}

function inferColumns(rows: TableWidgetRuntimeData['rows']): TableColumn[] {
  const firstRow = rows[0];
  if (!firstRow) return [];
  return Object.keys(firstRow).map(key => ({ key, header: key, format: 'text' as TableColumnFormat }));
}

function pickColor(value: number, thresholds?: GaugeThreshold[]): string {
  if (!thresholds || thresholds.length === 0) return '#10b981';
  const sorted = [...thresholds].sort((a, b) => a.value - b.value);
  let color = sorted[0].color;
  for (const t of sorted) {
    if (value >= t.value) color = t.color;
  }
  return color;
}

function withAlpha(color: string, alpha: number): string {
  if (color.startsWith('#') && color.length === 7) {
    const a = Math.round(alpha * 255).toString(16).padStart(2, '0');
    return `${color}${a}`;
  }
  return color;
}
