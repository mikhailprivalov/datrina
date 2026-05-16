import type { StatusGridConfig, StatusGridWidgetRuntimeData, StatusGridItem } from '../../lib/api';

interface Props {
  config: StatusGridConfig;
  data?: StatusGridWidgetRuntimeData;
}

const DEFAULT_STATUS_COLORS: Record<string, string> = {
  ok: '#10b981',
  up: '#10b981',
  healthy: '#10b981',
  success: '#10b981',
  green: '#10b981',
  active: '#10b981',
  warning: '#f59e0b',
  warn: '#f59e0b',
  degraded: '#f59e0b',
  pending: '#f59e0b',
  yellow: '#f59e0b',
  error: '#ef4444',
  down: '#ef4444',
  failed: '#ef4444',
  critical: '#ef4444',
  red: '#ef4444',
  unknown: '#94a3b8',
  unavailable: '#94a3b8',
};

export function StatusGridWidget({ config, data }: Props) {
  if (!data || data.items.length === 0) {
    return <div className="flex h-full items-center justify-center text-xs text-muted-foreground">No status data</div>;
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
      className="flex flex-col items-center justify-center rounded-md p-2 text-center text-foreground"
      style={{ backgroundColor: withAlpha(color, 0.18), borderColor: color, borderWidth: 1, borderStyle: 'solid' }}
      title={item.detail ? String(item.detail) : item.status}
    >
      <span
        className="inline-block h-2 w-2 rounded-full"
        style={{ backgroundColor: color }}
      />
      {showLabel && <span className="mt-1 truncate text-[10px] font-medium">{item.name}</span>}
      <span className="mt-0.5 truncate text-[10px] uppercase tracking-wide" style={{ color }}>
        {item.status}
      </span>
    </div>
  );
}

function RowCell({ item, color, showLabel }: { item: StatusGridItem; color: string; showLabel: boolean }) {
  return (
    <div
      className="flex items-center gap-1.5 rounded-full px-2 py-1 text-[11px]"
      style={{ backgroundColor: withAlpha(color, 0.18), borderColor: color, borderWidth: 1, borderStyle: 'solid' }}
      title={item.detail ? String(item.detail) : item.status}
    >
      <span className="inline-block h-2 w-2 rounded-full" style={{ backgroundColor: color }} />
      {showLabel && <span className="truncate font-medium">{item.name}</span>}
      <span className="uppercase text-[9px] tracking-wide" style={{ color }}>{item.status}</span>
    </div>
  );
}

function CompactCell({ item, color, showLabel }: { item: StatusGridItem; color: string; showLabel: boolean }) {
  return (
    <div
      className="h-3.5 w-3.5 rounded-sm"
      style={{ backgroundColor: color }}
      title={`${item.name}: ${item.status}${item.detail ? ` - ${item.detail}` : ''}${showLabel ? '' : ''}`}
    />
  );
}

function pickColor(status: string, map: Record<string, string>): string {
  const key = (status ?? '').toLowerCase().trim();
  return map[key] ?? map[status] ?? '#94a3b8';
}

function withAlpha(color: string, alpha: number): string {
  if (color.startsWith('#') && color.length === 7) {
    const a = Math.round(alpha * 255).toString(16).padStart(2, '0');
    return `${color}${a}`;
  }
  return color;
}
