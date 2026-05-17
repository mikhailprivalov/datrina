import type { GaugeConfig, GaugeWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: GaugeConfig;
  data?: GaugeWidgetRuntimeData;
}

export function GaugeWidget({ config, data }: Props) {
  const { min, max, unit, thresholds, show_value = true } = config;
  const value = data?.value;

  if (typeof value !== 'number') {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
        <span className="text-xs text-muted-foreground">Gauge data unavailable</span>
      </div>
    );
  }

  const pct = Math.min(100, Math.max(0, ((value - min) / (max - min)) * 100));

  const color = thresholds && thresholds.length > 0
    ? [...thresholds].sort((a, b) => a.value - b.value).find(t => value <= t.value)?.color ?? thresholds[thresholds.length - 1]?.color
    : 'hsl(var(--primary))';

  const r = 70;
  const stroke = 10;
  const nr = r - stroke / 2;
  const circ = nr * 2 * Math.PI;
  const offset = circ - (pct / 100) * circ;
  const gradientId = `gauge-grad-${Math.round(pct)}`;

  return (
    <div className="w-full h-full flex flex-col items-center justify-center gap-2">
      <div className="relative">
        <svg height={r * 2} width={r * 2} className="transform -rotate-90">
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity="0.9" />
              <stop offset="100%" stopColor={color} stopOpacity="1" />
            </linearGradient>
          </defs>
          <circle stroke="hsl(var(--muted))" fill="transparent" strokeWidth={stroke} r={nr} cx={r} cy={r} />
          <circle stroke={`url(#${gradientId})`} fill="transparent" strokeWidth={stroke} strokeDasharray={`${circ} ${circ}`} strokeDashoffset={offset} strokeLinecap="round" r={nr} cx={r} cy={r} className="transition-all duration-700" style={{ filter: `drop-shadow(0 0 6px ${color})` }} />
        </svg>
        {show_value && (
          <div className="absolute inset-0 flex flex-col items-center justify-center">
            <span className="text-2xl font-semibold tabular tracking-tight" style={{ color }}>{value}{unit ?? ''}</span>
            <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">of {max}{unit ?? ''}</span>
          </div>
        )}
      </div>
      {thresholds && thresholds.length > 0 && (
        <div className="flex gap-3 mt-1">
          {thresholds.map((t, i) => (
            <div key={i} className="flex items-center gap-1">
              <span className="w-2 h-2 rounded-full" style={{ backgroundColor: t.color }} />
              <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">{t.label ?? `${t.value}${unit ?? ''}`}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
