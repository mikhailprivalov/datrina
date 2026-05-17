import { LineChart, Line, ResponsiveContainer } from 'recharts';
import type { StatConfig, StatWidgetRuntimeData, GaugeThreshold } from '../../lib/api';

interface Props {
  config: StatConfig;
  data?: StatWidgetRuntimeData;
}

export function StatWidget({ config, data }: Props) {
  if (!data) {
    return <EmptyStat />;
  }
  const numericValue = typeof data.value === 'number' ? data.value : parseFloat(String(data.value));
  const displayValue = formatValue(data.value, config);
  const deltaInfo = formatDelta(data.delta);
  const color = pickColor(numericValue, config);
  const align = config.align ?? 'center';
  const showSpark = (config.graph_mode === 'sparkline' || config.show_sparkline)
    && Array.isArray(data.sparkline) && data.sparkline.length >= 2;

  const valueColor = config.color_mode === 'value' ? color : undefined;
  const bgColor = config.color_mode === 'background' ? color : undefined;

  return (
    <div
      className={`flex h-full w-full flex-col rounded-md p-2 ${alignClass(align)}`}
      style={{ backgroundColor: bgColor ? withAlpha(bgColor, 0.12) : undefined }}
    >
      <div className="flex flex-1 flex-col justify-center">
        <div
          className="text-3xl font-semibold tabular leading-none tracking-tight"
          style={{ color: valueColor ?? 'hsl(var(--foreground))' }}
        >
          {displayValue}
        </div>
        {(data.label || deltaInfo) && (
          <div className="mt-1.5 flex items-center gap-2 text-[11px] mono uppercase tracking-wider text-muted-foreground">
            {deltaInfo && (
              <span className={deltaInfo.direction === 'up' ? 'text-neon-lime' : deltaInfo.direction === 'down' ? 'text-destructive' : 'text-muted-foreground'}>
                {deltaInfo.direction === 'up' ? '↑' : deltaInfo.direction === 'down' ? '↓' : '·'} {deltaInfo.label}
              </span>
            )}
            {data.label && <span className="truncate normal-case tracking-normal">{data.label}</span>}
          </div>
        )}
      </div>
      {showSpark && (
        <div className="h-8 w-full">
          <ResponsiveContainer width="100%" height="100%">
            <LineChart data={normalizeSparkline(data.sparkline ?? [])}>
              <Line
                type="monotone"
                dataKey="v"
                stroke={color ?? 'currentColor'}
                strokeWidth={1.5}
                dot={false}
                isAnimationActive={false}
              />
            </LineChart>
          </ResponsiveContainer>
        </div>
      )}
    </div>
  );
}

function EmptyStat() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-1 text-center">
      <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
    </div>
  );
}

function formatValue(raw: number | string, config: StatConfig): string {
  let core: string;
  if (typeof raw === 'number' && Number.isFinite(raw)) {
    const decimals = config.decimals ?? (Math.abs(raw) >= 100 ? 0 : raw % 1 === 0 ? 0 : 2);
    core = raw.toLocaleString(undefined, {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    });
  } else {
    core = String(raw);
  }
  return `${config.prefix ?? ''}${core}${config.suffix ?? config.unit ?? ''}`;
}

function formatDelta(raw: number | string | null | undefined) {
  if (raw === null || raw === undefined || raw === '') return null;
  const num = typeof raw === 'number' ? raw : parseFloat(String(raw));
  if (!Number.isFinite(num)) {
    return { direction: 'flat' as const, label: String(raw) };
  }
  const direction = num > 0 ? 'up' : num < 0 ? 'down' : 'flat';
  const sign = num > 0 ? '+' : '';
  return { direction: direction as 'up' | 'down' | 'flat', label: `${sign}${num.toLocaleString()}` };
}

function pickColor(value: number, config: StatConfig): string | undefined {
  const thresholds = config.thresholds;
  if (!thresholds || thresholds.length === 0 || !Number.isFinite(value)) return undefined;
  const sorted: GaugeThreshold[] = [...thresholds].sort((a, b) => a.value - b.value);
  let active = sorted[0];
  for (const t of sorted) {
    if (value >= t.value) active = t;
  }
  return active.color;
}

function alignClass(align: 'left' | 'center' | 'right') {
  if (align === 'left') return 'items-start text-left';
  if (align === 'right') return 'items-end text-right';
  return 'items-center text-center';
}

function withAlpha(color: string, alpha: number): string {
  // accepts hex (#rrggbb) or names; for hex, append alpha.
  if (color.startsWith('#') && (color.length === 7)) {
    const a = Math.round(alpha * 255).toString(16).padStart(2, '0');
    return `${color}${a}`;
  }
  return color;
}

function normalizeSparkline(input: Array<{ t?: string | number; v: number } | number>) {
  return input.map((item, index) => {
    if (typeof item === 'number') {
      return { t: index, v: item };
    }
    return { t: item.t ?? index, v: item.v };
  });
}
