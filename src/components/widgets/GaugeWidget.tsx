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
      <div className="flex h-full min-h-24 items-center justify-center text-center text-xs text-muted-foreground">
        Gauge data unavailable
      </div>
    );
  }

  const pct = Math.min(100, Math.max(0, ((value - min) / (max - min)) * 100));

  const color = thresholds && thresholds.length > 0
    ? [...thresholds].sort((a, b) => a.value - b.value).find(t => value <= t.value)?.color ?? thresholds[thresholds.length - 1]?.color
    : 'hsl(25 45% 45%)';

  const r = 70;
  const stroke = 10;
  const nr = r - stroke / 2;
  const circ = nr * 2 * Math.PI;
  const offset = circ - (pct / 100) * circ;

  return (
    <div className="w-full h-full flex flex-col items-center justify-center gap-2">
      <div className="relative">
        <svg height={r * 2} width={r * 2} className="transform -rotate-90">
          <circle stroke="hsl(var(--muted))" fill="transparent" strokeWidth={stroke} r={nr} cx={r} cy={r} />
          <circle stroke={color} fill="transparent" strokeWidth={stroke} strokeDasharray={`${circ} ${circ}`} strokeDashoffset={offset} strokeLinecap="round" r={nr} cx={r} cy={r} className="transition-all duration-700" />
        </svg>
        {show_value && (
          <div className="absolute inset-0 flex flex-col items-center justify-center">
            <span className="text-2xl font-bold">{value}{unit ?? ''}</span>
            <span className="text-xs text-muted-foreground">of {max}{unit ?? ''}</span>
          </div>
        )}
      </div>
      {thresholds && thresholds.length > 0 && (
        <div className="flex gap-3 mt-1">
          {thresholds.map((t, i) => (
            <div key={i} className="flex items-center gap-1">
              <span className="w-2 h-2 rounded-full" style={{ backgroundColor: t.color }} />
              <span className="text-xs text-muted-foreground">{t.label ?? `${t.value}${unit ?? ''}`}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
