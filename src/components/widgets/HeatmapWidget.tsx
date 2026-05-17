import { useMemo } from 'react';
import type { HeatmapConfig, HeatmapWidgetRuntimeData, HeatmapCell } from '../../lib/api';

interface Props {
  config: HeatmapConfig;
  data?: HeatmapWidgetRuntimeData;
}

type Stop = [number, [number, number, number]];

const SCHEMES: Record<string, Stop[]> = {
  viridis: [
    [0, [68, 1, 84]],
    [0.25, [59, 82, 139]],
    [0.5, [33, 145, 140]],
    [0.75, [94, 201, 98]],
    [1, [253, 231, 37]],
  ],
  magma: [
    [0, [0, 0, 4]],
    [0.25, [80, 18, 123]],
    [0.5, [183, 55, 121]],
    [0.75, [251, 136, 97]],
    [1, [252, 253, 191]],
  ],
  cool: [
    [0, [70, 130, 180]],
    [0.5, [130, 180, 220]],
    [1, [220, 250, 255]],
  ],
  warm: [
    [0, [255, 245, 235]],
    [0.5, [254, 178, 76]],
    [1, [189, 0, 38]],
  ],
  green_red: [
    [0, [16, 185, 129]],
    [0.5, [245, 158, 11]],
    [1, [239, 68, 68]],
  ],
};

export function HeatmapWidget({ config, data }: Props) {
  const {
    xs,
    ys,
    cellMap,
    min,
    max,
  } = useMemo(() => computeAxes(data?.cells ?? []), [data]);
  const scheme = SCHEMES[config.color_scheme ?? 'viridis'] ?? SCHEMES.viridis;

  if (!data || data.cells.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
      </div>
    );
  }
  const useLog = (config.log_scale ?? false) && min > 0;
  return (
    <div className="flex h-full flex-col gap-1.5 p-1">
      <div className="flex-1 overflow-auto">
        <table className="border-separate border-spacing-0.5 text-[9px]">
          <thead>
            <tr>
              <th></th>
              {xs.map(x => (
                <th key={String(x)} className="px-1 text-muted-foreground font-normal truncate max-w-[3rem]">{String(x)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {ys.map(y => (
              <tr key={String(y)}>
                <th className="pr-1 text-right text-muted-foreground font-normal truncate max-w-[5rem]">{String(y)}</th>
                {xs.map(x => {
                  const v = cellMap.get(cellKey(x, y));
                  if (v === undefined) {
                    return <td key={String(x)} className="h-5 w-5 rounded-sm bg-muted/30" />;
                  }
                  const ratio = normalize(v, min, max, useLog);
                  const color = colorAt(ratio, scheme);
                  return (
                    <td
                      key={String(x)}
                      title={`x=${x}, y=${y}, value=${v}${config.unit ?? ''}`}
                      style={{ backgroundColor: color }}
                      className="h-5 w-5 rounded-sm"
                    />
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {config.show_legend !== false && (
        <HeatmapLegend min={min} max={max} scheme={scheme} unit={config.unit} />
      )}
    </div>
  );
}

function HeatmapLegend({ min, max, scheme, unit }: { min: number; max: number; scheme: Stop[]; unit?: string }) {
  const gradient = `linear-gradient(to right, ${scheme.map(([stop, rgb]) => `rgb(${rgb.join(',')}) ${Math.round(stop * 100)}%`).join(', ')})`;
  return (
    <div className="flex items-center gap-2 text-[10px] mono tabular text-muted-foreground">
      <span>{formatValue(min)}{unit ?? ''}</span>
      <div className="flex-1 h-2 rounded-sm border border-border/60" style={{ background: gradient }} />
      <span>{formatValue(max)}{unit ?? ''}</span>
    </div>
  );
}

function computeAxes(cells: HeatmapCell[]) {
  const xSet = new Set<string>();
  const ySet = new Set<string>();
  const xKey = new Map<string, string | number>();
  const yKey = new Map<string, string | number>();
  const cellMap = new Map<string, number>();
  let min = Infinity;
  let max = -Infinity;
  for (const c of cells) {
    const xs = String(c.x);
    const ys = String(c.y);
    xSet.add(xs);
    ySet.add(ys);
    xKey.set(xs, c.x);
    yKey.set(ys, c.y);
    cellMap.set(cellKey(c.x, c.y), c.value);
    if (c.value < min) min = c.value;
    if (c.value > max) max = c.value;
  }
  const xs = [...xSet].sort(compareAxis).map(s => xKey.get(s) ?? s);
  const ys = [...ySet].sort(compareAxis).map(s => yKey.get(s) ?? s);
  if (!Number.isFinite(min)) min = 0;
  if (!Number.isFinite(max)) max = 1;
  return { xs, ys, cellMap, min, max };
}

function compareAxis(a: string, b: string) {
  const na = Number(a);
  const nb = Number(b);
  if (!Number.isNaN(na) && !Number.isNaN(nb)) return na - nb;
  return a.localeCompare(b);
}

function cellKey(x: number | string, y: number | string) {
  return `${x}::${y}`;
}

function normalize(value: number, min: number, max: number, useLog: boolean): number {
  if (max === min) return 0.5;
  if (useLog) {
    const lmin = Math.log(min);
    const lmax = Math.log(max);
    if (!Number.isFinite(lmin) || !Number.isFinite(lmax) || lmin === lmax) return 0.5;
    return (Math.log(Math.max(value, min)) - lmin) / (lmax - lmin);
  }
  return (value - min) / (max - min);
}

function colorAt(ratio: number, scheme: Stop[]): string {
  const r = Math.max(0, Math.min(1, ratio));
  for (let i = 1; i < scheme.length; i++) {
    const [stop, rgb] = scheme[i];
    const [prevStop, prevRgb] = scheme[i - 1];
    if (r <= stop) {
      const t = (r - prevStop) / (stop - prevStop || 1);
      const c = [0, 1, 2].map(j => Math.round(prevRgb[j] + (rgb[j] - prevRgb[j]) * t));
      return `rgb(${c.join(',')})`;
    }
  }
  const last = scheme[scheme.length - 1][1];
  return `rgb(${last.join(',')})`;
}

function formatValue(v: number): string {
  if (Math.abs(v) >= 1000) return v.toLocaleString(undefined, { maximumFractionDigits: 0 });
  if (v % 1 === 0) return String(v);
  return v.toFixed(2);
}
