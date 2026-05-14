import {
  LineChart, Line, BarChart, Bar, AreaChart, Area,
  PieChart, Pie, Cell, XAxis, YAxis, CartesianGrid,
  Tooltip, Legend, ResponsiveContainer, ScatterChart, Scatter,
} from 'recharts';
import type { ChartConfig, ChartWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: ChartConfig;
  data?: ChartWidgetRuntimeData;
}

const COLORS = ['#C1784A', '#4A9B9B', '#D4A843', '#A8556E', '#5A9E7A', '#C75A3A', '#7BA3C0'];

const tooltipStyle = {
  backgroundColor: 'hsl(30 15% 99%)',
  border: '1px solid hsl(30 15% 88%)',
  borderRadius: '8px',
  fontSize: '12px',
};

export function ChartWidget({ config, data }: Props) {
  const { kind } = config;
  const rows = data?.rows ?? [];
  const xKey = config.x_axis ?? 'name';
  const yKeys = config.y_axis?.length
    ? config.y_axis
    : inferNumericKeys(rows, xKey);
  const colors = config.colors?.length ? config.colors : COLORS;

  if (rows.length === 0 || yKeys.length === 0) {
    return <EmptyRuntimeData label="Chart data unavailable" />;
  }

  switch (kind) {
    case 'line':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={rows}>
            <CartesianGrid strokeDasharray="3 3" stroke="hsl(30 15% 88%)" />
            <XAxis dataKey={xKey} tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <YAxis tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <Tooltip contentStyle={tooltipStyle} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px' }} />}
            {yKeys.map((key, index) => (
              <Line key={key} type="monotone" dataKey={key} stroke={colors[index % colors.length]} strokeWidth={2} dot={{ r: 3 }} />
            ))}
          </LineChart>
        </ResponsiveContainer>
      );
    case 'bar':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={rows}>
            <CartesianGrid strokeDasharray="3 3" stroke="hsl(30 15% 88%)" />
            <XAxis dataKey={xKey} tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <YAxis tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <Tooltip contentStyle={tooltipStyle} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px' }} />}
            {yKeys.map((key, index) => (
              <Bar key={key} dataKey={key} fill={colors[index % colors.length]} radius={[4, 4, 0, 0]} stackId={config.stacked ? 'stack' : undefined} />
            ))}
          </BarChart>
        </ResponsiveContainer>
      );
    case 'area':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={rows}>
            <CartesianGrid strokeDasharray="3 3" stroke="hsl(30 15% 88%)" />
            <XAxis dataKey={xKey} tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <YAxis tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <Tooltip contentStyle={tooltipStyle} />
            {yKeys.map((key, index) => (
              <Area key={key} type="monotone" dataKey={key} stroke={colors[index % colors.length]} fill={colors[index % colors.length]} fillOpacity={0.3} stackId={config.stacked ? 'stack' : undefined} />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      );
    case 'pie':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={rows} cx="50%" cy="50%" innerRadius="40%" outerRadius="70%" paddingAngle={4} dataKey={yKeys[0]} nameKey={xKey}>
              {rows.map((_, i) => <Cell key={i} fill={colors[i % colors.length]} />)}
            </Pie>
            <Tooltip contentStyle={tooltipStyle} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px' }} />}
          </PieChart>
        </ResponsiveContainer>
      );
    case 'scatter':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <ScatterChart>
            <CartesianGrid strokeDasharray="3 3" stroke="hsl(30 15% 88%)" />
            <XAxis type="number" dataKey={yKeys[0]} tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <YAxis type="number" dataKey={yKeys[1] ?? yKeys[0]} tick={{ fill: 'hsl(30 5% 45%)', fontSize: 11 }} />
            <Tooltip contentStyle={tooltipStyle} />
            <Scatter data={rows} fill={colors[0]} />
          </ScatterChart>
        </ResponsiveContainer>
      );
    default:
      return <div className="text-muted-foreground text-sm">Unsupported chart: {kind}</div>;
  }
}

function inferNumericKeys(rows: Record<string, string | number | null>[], xKey: string) {
  const firstRow = rows[0];
  if (!firstRow) return [];
  return Object.entries(firstRow)
    .filter(([key, value]) => key !== xKey && typeof value === 'number')
    .map(([key]) => key);
}

function EmptyRuntimeData({ label }: { label: string }) {
  return (
    <div className="flex h-full min-h-24 items-center justify-center text-center text-xs text-muted-foreground">
      {label}
    </div>
  );
}
