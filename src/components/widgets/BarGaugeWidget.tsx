import type { BarGaugeConfig, BarGaugeWidgetRuntimeData, GaugeThreshold } from '../../lib/api';

interface Props {
  config: BarGaugeConfig;
  data?: BarGaugeWidgetRuntimeData;
}

const DEFAULT_BAR_COLOR = 'hsl(var(--primary))';

export function BarGaugeWidget({ config, data }: Props) {
  if (!data || data.rows.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
      </div>
    );
  }

  const orientation = config.orientation ?? 'horizontal';
  const explicitMax = config.max;
  const minBase = config.min ?? 0;
  const computedMax = explicitMax ?? Math.max(...data.rows.map(r => r.max ?? r.value));
  const max = Math.max(computedMax, 1);
  const thresholds = config.thresholds;
  const valueSuffix = config.unit ?? '';

  if (orientation === 'vertical') {
    return (
      <div className="flex h-full items-end gap-2 p-2">
        {data.rows.map(row => {
          const ratio = clamp01((row.value - minBase) / (max - minBase));
          const color = pickColor(row.value, thresholds);
          return (
            <div key={row.name} className="flex flex-1 min-w-0 flex-col items-center justify-end gap-1">
              {config.show_value !== false && (
                <span className="text-[10px] mono tabular text-foreground">{formatValue(row.value)}{valueSuffix}</span>
              )}
              <div className="w-full flex-1 rounded-sm bg-muted/50 border border-border/60 overflow-hidden flex items-end">
                <div
                  className="w-full transition-all"
                  style={{ height: `${ratio * 100}%`, backgroundColor: color, boxShadow: `0 0 8px ${color}55` }}
                />
              </div>
              <span className="block w-full truncate text-center text-[10px] mono uppercase tracking-wider text-muted-foreground">{row.name}</span>
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col gap-1.5 overflow-auto p-1">
      {data.rows.map(row => {
        const rowMax = row.max ?? max;
        const ratio = clamp01((row.value - minBase) / (rowMax - minBase || 1));
        const color = pickColor(row.value, thresholds);
        return (
          <div key={row.name} className="flex items-center gap-2">
            <span className="w-1/3 min-w-0 truncate text-[11px] mono text-muted-foreground">{row.name}</span>
            <div className="relative flex-1 h-5 rounded-sm bg-muted/50 border border-border/60 overflow-hidden">
              <div
                className="absolute inset-y-0 left-0 transition-all"
                style={{ width: `${ratio * 100}%`, backgroundColor: color, boxShadow: `inset 0 0 8px ${color}66` }}
              />
              {config.show_value !== false && (
                <span className="absolute inset-0 flex items-center justify-end pr-1.5 text-[10px] mono font-medium tabular text-foreground">
                  {formatValue(row.value)}{valueSuffix}
                </span>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function pickColor(value: number, thresholds?: GaugeThreshold[]): string {
  if (!thresholds || thresholds.length === 0) return DEFAULT_BAR_COLOR;
  const sorted = [...thresholds].sort((a, b) => a.value - b.value);
  let color = sorted[0].color;
  for (const t of sorted) {
    if (value >= t.value) color = t.color;
  }
  return color;
}

function clamp01(x: number): number {
  if (!Number.isFinite(x)) return 0;
  return Math.max(0, Math.min(1, x));
}

function formatValue(value: number): string {
  if (Math.abs(value) >= 100 || value % 1 === 0) {
    return value.toLocaleString(undefined, { maximumFractionDigits: 0 });
  }
  return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
}
