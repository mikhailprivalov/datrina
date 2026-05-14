import { useState } from 'react';
import type { TableColumn, TableConfig, TableWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: TableConfig;
  data?: TableWidgetRuntimeData;
}

export function TableWidget({ config, data }: Props) {
  const { columns, page_size = 10, sortable = true } = config;
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');
  const rows = data?.rows ?? [];

  const cols: TableColumn[] = columns.length > 0 ? columns : inferColumns(rows);

  const sorted = [...rows].sort((a, b) => {
    if (!sortKey || !sortable) return 0;
    const av = String(a[sortKey] ?? '');
    const bv = String(b[sortKey] ?? '');
    return sortDir === 'asc' ? av.localeCompare(bv) : bv.localeCompare(av);
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
              {cols.map(col => {
                const val = row[col.key];
                return (
                  <td key={col.key} className="py-1.5 px-2">
                    <span className="text-foreground/90">{formatValue(val, col.format)}</span>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function inferColumns(rows: TableWidgetRuntimeData['rows']): TableColumn[] {
  const firstRow = rows[0];
  if (!firstRow) return [];
  return Object.keys(firstRow).map(key => ({ key, header: key }));
}

function formatValue(value: string | number | boolean | null | undefined, format = 'text') {
  if (value === null || value === undefined) return '-';
  if (typeof value === 'number') {
    if (format === 'currency') return new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(value);
    if (format === 'percent') return `${value}%`;
    return new Intl.NumberFormat().format(value);
  }
  return String(value);
}
