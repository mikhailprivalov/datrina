import { useEffect, useState } from 'react';
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

/**
 * Recharts SVG attrs need real color strings, not Tailwind classes. Read the
 * cyber palette from the active theme's CSS variables and rebuild on theme
 * toggle so the chart never falls behind the rest of the UI.
 */
function readChartPalette(): { colors: string[]; grid: string; axis: string; tooltipBg: string; tooltipBorder: string; tooltipFg: string } {
  if (typeof window === 'undefined') {
    return {
      colors: ['#22d3ee', '#e879f9', '#84cc16', '#fbbf24', '#a78bfa', '#fb7185'],
      grid: 'rgba(120,128,148,0.18)',
      axis: 'rgba(160,168,184,0.85)',
      tooltipBg: '#0f1115',
      tooltipBorder: '#262b35',
      tooltipFg: '#e6edf3',
    };
  }
  const style = window.getComputedStyle(document.documentElement);
  const hsl = (token: string, alpha?: number) => {
    const value = style.getPropertyValue(token).trim();
    if (!value) return 'transparent';
    return alpha === undefined ? `hsl(${value})` : `hsl(${value} / ${alpha})`;
  };
  return {
    colors: [
      hsl('--chart-1'),
      hsl('--chart-2'),
      hsl('--chart-3'),
      hsl('--chart-4'),
      hsl('--chart-5'),
      hsl('--chart-6'),
    ],
    grid: hsl('--grid-line', 0.6),
    axis: hsl('--muted-foreground'),
    tooltipBg: hsl('--popover'),
    tooltipBorder: hsl('--border'),
    tooltipFg: hsl('--popover-foreground'),
  };
}

function usePalette() {
  const [palette, setPalette] = useState(readChartPalette);
  useEffect(() => {
    setPalette(readChartPalette());
    const root = document.documentElement;
    const observer = new MutationObserver(() => setPalette(readChartPalette()));
    observer.observe(root, { attributes: true, attributeFilter: ['class'] });
    return () => observer.disconnect();
  }, []);
  return palette;
}

export function ChartWidget({ config, data }: Props) {
  const { kind } = config;
  const rows = data?.rows ?? [];
  const xKey = config.x_axis ?? 'name';
  const yKeys = config.y_axis?.length
    ? config.y_axis
    : inferNumericKeys(rows, xKey);
  const palette = usePalette();
  const colors = config.colors?.length ? config.colors : palette.colors;
  const axisTick = { fill: palette.axis, fontSize: 11 };
  const tooltipStyle = {
    backgroundColor: palette.tooltipBg,
    border: `1px solid ${palette.tooltipBorder}`,
    borderRadius: '6px',
    color: palette.tooltipFg,
    fontSize: '12px',
  };

  if (rows.length === 0 || yKeys.length === 0) {
    return <EmptyRuntimeData label="Chart data unavailable" />;
  }

  switch (kind) {
    case 'line':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={rows}>
            <CartesianGrid strokeDasharray="2 4" stroke={palette.grid} />
            <XAxis dataKey={xKey} tick={axisTick} stroke={palette.grid} />
            <YAxis tick={axisTick} stroke={palette.grid} />
            <Tooltip contentStyle={tooltipStyle} cursor={{ stroke: palette.colors[0], strokeOpacity: 0.35 }} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px', color: palette.axis }} />}
            {yKeys.map((key, index) => (
              <Line key={key} type="monotone" dataKey={key} stroke={colors[index % colors.length]} strokeWidth={2} dot={{ r: 2, strokeWidth: 0, fill: colors[index % colors.length] }} activeDot={{ r: 4 }} />
            ))}
          </LineChart>
        </ResponsiveContainer>
      );
    case 'bar':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={rows}>
            <CartesianGrid strokeDasharray="2 4" stroke={palette.grid} />
            <XAxis dataKey={xKey} tick={axisTick} stroke={palette.grid} />
            <YAxis tick={axisTick} stroke={palette.grid} />
            <Tooltip contentStyle={tooltipStyle} cursor={{ fill: palette.colors[0], fillOpacity: 0.08 }} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px', color: palette.axis }} />}
            {yKeys.map((key, index) => (
              <Bar key={key} dataKey={key} fill={colors[index % colors.length]} radius={[3, 3, 0, 0]} stackId={config.stacked ? 'stack' : undefined} />
            ))}
          </BarChart>
        </ResponsiveContainer>
      );
    case 'area':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={rows}>
            <defs>
              {yKeys.map((key, index) => (
                <linearGradient key={key} id={`area-${index}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={colors[index % colors.length]} stopOpacity={0.55} />
                  <stop offset="100%" stopColor={colors[index % colors.length]} stopOpacity={0.04} />
                </linearGradient>
              ))}
            </defs>
            <CartesianGrid strokeDasharray="2 4" stroke={palette.grid} />
            <XAxis dataKey={xKey} tick={axisTick} stroke={palette.grid} />
            <YAxis tick={axisTick} stroke={palette.grid} />
            <Tooltip contentStyle={tooltipStyle} />
            {yKeys.map((key, index) => (
              <Area key={key} type="monotone" dataKey={key} stroke={colors[index % colors.length]} fill={`url(#area-${index})`} strokeWidth={2} stackId={config.stacked ? 'stack' : undefined} />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      );
    case 'pie':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Pie data={rows} cx="50%" cy="50%" innerRadius="45%" outerRadius="72%" paddingAngle={3} dataKey={yKeys[0]} nameKey={xKey} stroke={palette.tooltipBg} strokeWidth={2}>
              {rows.map((_, i) => <Cell key={i} fill={colors[i % colors.length]} />)}
            </Pie>
            <Tooltip contentStyle={tooltipStyle} />
            {config.show_legend !== false && <Legend wrapperStyle={{ fontSize: '11px', color: palette.axis }} />}
          </PieChart>
        </ResponsiveContainer>
      );
    case 'scatter':
      return (
        <ResponsiveContainer width="100%" height="100%">
          <ScatterChart>
            <CartesianGrid strokeDasharray="2 4" stroke={palette.grid} />
            <XAxis type="number" dataKey={yKeys[0]} tick={axisTick} stroke={palette.grid} />
            <YAxis type="number" dataKey={yKeys[1] ?? yKeys[0]} tick={axisTick} stroke={palette.grid} />
            <Tooltip contentStyle={tooltipStyle} cursor={{ stroke: palette.colors[0], strokeOpacity: 0.35 }} />
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
    <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
      <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
}
