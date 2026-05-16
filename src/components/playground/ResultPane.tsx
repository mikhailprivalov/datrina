import { useMemo, useState } from 'react';
import {
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';

interface Props {
  result: unknown;
  durationMs?: number;
}

type TabKey = 'json' | 'table' | 'chart' | 'schema';

export function ResultPane({ result, durationMs }: Props) {
  const [tab, setTab] = useState<TabKey>('json');
  const tableCandidates = useMemo(() => findTableCandidates(result), [result]);
  const [tablePath, setTablePath] = useState<string>('');
  const activeRows = useMemo(
    () =>
      tableCandidates.find(c => c.path === tablePath)?.rows ??
      tableCandidates[0]?.rows ??
      [],
    [tableCandidates, tablePath]
  );
  const numericColumns = useMemo(() => firstNumericColumns(activeRows), [activeRows]);

  if (result === undefined) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Run a tool to see results here.
      </div>
    );
  }

  const tabs: Array<{ key: TabKey; label: string; disabled?: boolean }> = [
    { key: 'json', label: 'JSON' },
    { key: 'table', label: 'Table', disabled: tableCandidates.length === 0 },
    { key: 'chart', label: 'Chart', disabled: numericColumns.length === 0 || activeRows.length === 0 },
    { key: 'schema', label: 'Schema' },
  ];

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex items-center justify-between border-b border-border pb-1">
        <div className="flex gap-1">
          {tabs.map(t => (
            <button
              key={t.key}
              disabled={t.disabled}
              onClick={() => !t.disabled && setTab(t.key)}
              className={`rounded-md px-2 py-1 text-xs transition-colors ${
                tab === t.key
                  ? 'bg-primary/15 text-primary'
                  : 'text-muted-foreground hover:bg-muted'
              } ${t.disabled ? 'opacity-40 cursor-not-allowed' : ''}`}
            >
              {t.label}
            </button>
          ))}
        </div>
        {durationMs !== undefined && (
          <span className="text-[11px] text-muted-foreground tabular-nums">
            {Math.round(durationMs)}ms
          </span>
        )}
      </div>

      <div className="flex-1 min-h-0 overflow-auto">
        {tab === 'json' && <JsonView value={result} />}
        {tab === 'table' && (
          <TableView
            rows={activeRows}
            candidates={tableCandidates}
            selected={tablePath}
            onSelect={setTablePath}
          />
        )}
        {tab === 'chart' && (
          <ChartView rows={activeRows} numericColumns={numericColumns} />
        )}
        {tab === 'schema' && <SchemaView value={result} />}
      </div>
    </div>
  );
}

function JsonView({ value }: { value: unknown }) {
  const text = useMemo(() => safeStringify(value, 2), [value]);
  return (
    <pre className="rounded-md bg-background/70 p-2 text-[11px] font-mono whitespace-pre-wrap break-all">
      {text}
    </pre>
  );
}

function TableView({
  rows,
  candidates,
  selected,
  onSelect,
}: {
  rows: Record<string, unknown>[];
  candidates: TableCandidate[];
  selected: string;
  onSelect: (path: string) => void;
}) {
  if (rows.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        No array of objects found in the result.
      </p>
    );
  }
  const columns = Array.from(
    rows.reduce<Set<string>>((cols, row) => {
      Object.keys(row).forEach(k => cols.add(k));
      return cols;
    }, new Set<string>())
  ).slice(0, 12);
  return (
    <div className="space-y-2">
      {candidates.length > 1 && (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>Root:</span>
          <select
            value={selected}
            onChange={e => onSelect(e.target.value)}
            className="rounded-md border border-border bg-background px-1.5 py-0.5 text-xs"
          >
            {candidates.map(c => (
              <option key={c.path} value={c.path}>
                {c.path || '<root>'} ({c.rows.length})
              </option>
            ))}
          </select>
        </div>
      )}
      <div className="overflow-auto">
        <table className="w-full text-[11px]">
          <thead>
            <tr className="border-b border-border text-left text-muted-foreground">
              {columns.map(col => (
                <th key={col} className="px-2 py-1 font-medium">{col}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.slice(0, 100).map((row, idx) => (
              <tr key={idx} className="border-b border-border/40">
                {columns.map(col => (
                  <td key={col} className="px-2 py-1 font-mono whitespace-nowrap">
                    {formatCell(row[col])}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {rows.length > 100 && (
        <p className="text-[11px] text-muted-foreground">
          Showing first 100 of {rows.length} rows.
        </p>
      )}
    </div>
  );
}

function ChartView({
  rows,
  numericColumns,
}: {
  rows: Record<string, unknown>[];
  numericColumns: string[];
}) {
  const [yAxis, setYAxis] = useState<string>(() => numericColumns[0] ?? '');
  const [kind, setKind] = useState<'line' | 'bar'>('line');
  const xAxis = useMemo(() => guessXAxis(rows, numericColumns), [rows, numericColumns]);

  if (numericColumns.length === 0 || rows.length === 0) {
    return <p className="text-xs text-muted-foreground">No numeric columns to chart.</p>;
  }

  const data = rows
    .map(row => ({
      ...row,
      [xAxis]: row[xAxis] ?? '',
      [yAxis]: Number(row[yAxis] ?? 0),
    }))
    .slice(0, 200);

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs">
        <select
          value={kind}
          onChange={e => setKind(e.target.value as 'line' | 'bar')}
          className="rounded-md border border-border bg-background px-1.5 py-0.5"
        >
          <option value="line">Line</option>
          <option value="bar">Bar</option>
        </select>
        <span className="text-muted-foreground">y:</span>
        <select
          value={yAxis}
          onChange={e => setYAxis(e.target.value)}
          className="rounded-md border border-border bg-background px-1.5 py-0.5"
        >
          {numericColumns.map(col => (
            <option key={col} value={col}>{col}</option>
          ))}
        </select>
        <span className="text-muted-foreground">x: {xAxis}</span>
      </div>
      <div className="h-64 w-full">
        <ResponsiveContainer>
          {kind === 'line' ? (
            <LineChart data={data}>
              <CartesianGrid strokeDasharray="3 3" stroke="hsl(var(--border))" />
              <XAxis dataKey={xAxis} fontSize={11} />
              <YAxis fontSize={11} />
              <Tooltip />
              <Line type="monotone" dataKey={yAxis} stroke="hsl(var(--primary))" dot={false} />
            </LineChart>
          ) : (
            <BarChart data={data}>
              <CartesianGrid strokeDasharray="3 3" stroke="hsl(var(--border))" />
              <XAxis dataKey={xAxis} fontSize={11} />
              <YAxis fontSize={11} />
              <Tooltip />
              <Bar dataKey={yAxis} fill="hsl(var(--primary))" />
            </BarChart>
          )}
        </ResponsiveContainer>
      </div>
    </div>
  );
}

function SchemaView({ value }: { value: unknown }) {
  const summary = useMemo(() => describeShape(value, ''), [value]);
  return (
    <pre className="rounded-md bg-background/70 p-2 text-[11px] font-mono whitespace-pre-wrap">
      {summary.join('\n')}
    </pre>
  );
}

interface TableCandidate {
  path: string;
  rows: Record<string, unknown>[];
}

function findTableCandidates(value: unknown, prefix = '', depth = 0): TableCandidate[] {
  if (depth > 3) return [];
  const out: TableCandidate[] = [];
  if (Array.isArray(value) && value.length > 0 && value.every(isPlainObject)) {
    out.push({ path: prefix, rows: value as Record<string, unknown>[] });
    return out;
  }
  if (isPlainObject(value)) {
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      const childPath = prefix ? `${prefix}.${k}` : k;
      out.push(...findTableCandidates(v, childPath, depth + 1));
    }
  }
  return out;
}

function firstNumericColumns(rows: Record<string, unknown>[]): string[] {
  if (rows.length === 0) return [];
  const cols = Array.from(rows.reduce<Set<string>>((set, row) => {
    Object.keys(row).forEach(k => set.add(k));
    return set;
  }, new Set<string>()));
  return cols.filter(col => rows.some(row => typeof row[col] === 'number' && Number.isFinite(row[col])));
}

function guessXAxis(rows: Record<string, unknown>[], numericColumns: string[]): string {
  if (rows.length === 0) return '';
  const numericSet = new Set(numericColumns);
  const allCols = Array.from(rows.reduce<Set<string>>((set, row) => {
    Object.keys(row).forEach(k => set.add(k));
    return set;
  }, new Set<string>()));
  return (
    allCols.find(col => !numericSet.has(col) && rows.some(row => typeof row[col] === 'string')) ??
    allCols[0] ??
    ''
  );
}

function describeShape(value: unknown, path: string, depth = 0, lines: string[] = []): string[] {
  if (depth > 5) {
    lines.push(`${path || '<root>'}: …`);
    return lines;
  }
  if (value === null) {
    lines.push(`${path || '<root>'}: null`);
    return lines;
  }
  if (Array.isArray(value)) {
    const sample = value[0];
    lines.push(`${path || '<root>'}: array[${value.length}]`);
    if (sample !== undefined) {
      describeShape(sample, path ? `${path}[]` : '[]', depth + 1, lines);
    }
    return lines;
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (depth === 0) lines.push(`${path || '<root>'}: object`);
    for (const [key, v] of entries) {
      const next = path ? `${path}.${key}` : key;
      describeShape(v, next, depth + 1, lines);
    }
    return lines;
  }
  lines.push(`${path || '<root>'}: ${typeof value}`);
  return lines;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function formatCell(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'object') return safeStringify(value);
  return String(value);
}

export function safeStringify(value: unknown, indent?: number): string {
  try {
    return JSON.stringify(value, null, indent);
  } catch {
    return String(value);
  }
}
