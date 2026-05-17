import type { StatusGridConfig, StatusGridWidgetRuntimeData, StatusGridItem } from '../../lib/api';

interface Props {
  config: StatusGridConfig;
  data?: StatusGridWidgetRuntimeData;
}

// Cyber status palette mirrors LogsWidget/TableWidget. Opaque hsl() so inline
// styles can fade them via withAlpha without re-resolving CSS vars.
const OK = 'hsl(150 85% 56%)';
const WARN = 'hsl(38 95% 60%)';
const ERR = 'hsl(350 90% 62%)';
const UNK = 'hsl(220 12% 64%)';

const DEFAULT_STATUS_COLORS: Record<string, string> = {
  ok: OK, up: OK, healthy: OK, success: OK, green: OK, active: OK,
  warning: WARN, warn: WARN, degraded: WARN, pending: WARN, yellow: WARN,
  error: ERR, down: ERR, failed: ERR, critical: ERR, red: ERR,
  unknown: UNK, unavailable: UNK,
};

export function StatusGridWidget({ config, data }: Props) {
  if (!data || data.items.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
      </div>
    );
  }
  const layout = config.layout ?? 'grid';
  const columns = config.columns ?? 4;
  const showLabel = config.show_label !== false;
  const colorMap = { ...DEFAULT_STATUS_COLORS, ...(config.status_colors ?? {}) };

  if (layout === 'compact') {
    return (
      <div className="flex h-full flex-wrap content-start items-start gap-1 p-1">
        {data.items.map((item, i) => (
          <CompactCell key={i} item={item} color={pickColor(item.status, colorMap)} showLabel={showLabel} />
        ))}
      </div>
    );
  }

  if (layout === 'row') {
    return (
      <div className="flex h-full flex-wrap content-start items-start gap-2 p-1 overflow-auto">
        {data.items.map((item, i) => (
          <RowCell key={i} item={item} color={pickColor(item.status, colorMap)} showLabel={showLabel} />
        ))}
      </div>
    );
  }

  return (
    <div
      className="grid h-full gap-1.5 overflow-auto p-1"
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {data.items.map((item, i) => (
        <GridCell key={i} item={item} color={pickColor(item.status, colorMap)} showLabel={showLabel} />
      ))}
    </div>
  );
}

function GridCell({ item, color, showLabel }: { item: StatusGridItem; color: string; showLabel: boolean }) {
  return (
    <div
      className="flex flex-col items-center justify-center rounded-sm p-2 text-center text-foreground"
      style={{ backgroundColor: withAlpha(color, 0.14), borderColor: withAlpha(color, 0.5), borderWidth: 1, borderStyle: 'solid' }}
      title={item.detail ? String(item.detail) : item.status}
    >
      <span
        className="inline-block h-2 w-2 rounded-full"
        style={{ backgroundColor: color, boxShadow: `0 0 6px ${color}` }}
      />
      {showLabel && <span className="mt-1 truncate text-[10px] font-medium">{item.name}</span>}
      <span className="mt-0.5 truncate text-[10px] mono uppercase tracking-wider" style={{ color }}>
        {item.status}
      </span>
    </div>
  );
}

function RowCell({ item, color, showLabel }: { item: StatusGridItem; color: string; showLabel: boolean }) {
  return (
    <div
      className="flex items-center gap-1.5 rounded-sm px-2 py-1 text-[11px]"
      style={{ backgroundColor: withAlpha(color, 0.14), borderColor: withAlpha(color, 0.5), borderWidth: 1, borderStyle: 'solid' }}
      title={item.detail ? String(item.detail) : item.status}
    >
      <span className="inline-block h-2 w-2 rounded-full" style={{ backgroundColor: color, boxShadow: `0 0 6px ${color}` }} />
      {showLabel && <span className="truncate font-medium">{item.name}</span>}
      <span className="text-[9px] mono uppercase tracking-wider" style={{ color }}>{item.status}</span>
    </div>
  );
}

function CompactCell({ item, color, showLabel }: { item: StatusGridItem; color: string; showLabel: boolean }) {
  return (
    <div
      className="h-3.5 w-3.5 rounded-sm"
      style={{ backgroundColor: color, boxShadow: `0 0 4px ${color}` }}
      title={`${item.name}: ${item.status}${item.detail ? ` - ${item.detail}` : ''}${showLabel ? '' : ''}`}
    />
  );
}

function pickColor(status: string, map: Record<string, string>): string {
  const key = (status ?? '').toLowerCase().trim();
  return map[key] ?? map[status] ?? UNK;
}

function withAlpha(color: string, alpha: number): string {
  if (color.startsWith('#') && color.length === 7) {
    const a = Math.round(alpha * 255).toString(16).padStart(2, '0');
    return `${color}${a}`;
  }
  if (color.startsWith('hsl(') && !color.startsWith('hsla(')) {
    return `hsl(${color.slice(4, -1).trim()} / ${alpha})`;
  }
  return color;
}
