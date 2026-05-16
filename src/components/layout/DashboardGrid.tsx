import { useEffect, useRef, useState } from 'react';
import { Responsive, WidthProvider } from 'react-grid-layout';
import 'react-grid-layout/css/styles.css';
import 'react-resizable/css/styles.css';
import type { Dashboard, Widget, WidgetRuntimeData, WorkflowRun } from '../../lib/api';
import { ChartWidget } from '../widgets/ChartWidget';
import { TextWidget } from '../widgets/TextWidget';
import { TableWidget } from '../widgets/TableWidget';
import { GaugeWidget } from '../widgets/GaugeWidget';
import { ImageWidget } from '../widgets/ImageWidget';
import { StatWidget } from '../widgets/StatWidget';
import { LogsWidget } from '../widgets/LogsWidget';
import { BarGaugeWidget } from '../widgets/BarGaugeWidget';
import { StatusGridWidget } from '../widgets/StatusGridWidget';
import { HeatmapWidget } from '../widgets/HeatmapWidget';
import type { Layout } from 'react-grid-layout';

const ResponsiveGridLayout = WidthProvider(Responsive);

interface Props {
  dashboard: Dashboard;
  widgetData: Record<string, WidgetRuntimeData | undefined>;
  widgetErrors: Record<string, string | undefined>;
  workflowRuns: Record<string, WorkflowRun | undefined>;
  refreshingWidgetId: string | null;
  onRefreshWidget: (widgetId: string) => void;
  onLayoutCommit: (layout: Widget[]) => void;
  onAddWidget: (widgetType: 'text' | 'gauge') => void;
  onUpdateWidgets: (next: Widget[]) => void;
}

export function DashboardGrid({
  dashboard,
  widgetData,
  widgetErrors,
  workflowRuns,
  refreshingWidgetId,
  onRefreshWidget,
  onLayoutCommit,
  onAddWidget,
  onUpdateWidgets,
}: Props) {
  const [inspecting, setInspecting] = useState<{ widget: Widget; data?: WidgetRuntimeData; run?: WorkflowRun } | null>(null);
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  const handleDeleteWidget = (id: string) => {
    if (!window.confirm('Delete this widget? Its workflow stays so you can re-add it later.')) return;
    onUpdateWidgets(dashboard.layout.filter(w => w.id !== id));
  };
  const handleDuplicateWidget = (id: string) => {
    const widget = dashboard.layout.find(w => w.id === id);
    if (!widget) return;
    const newId = (typeof crypto !== 'undefined' && 'randomUUID' in crypto)
      ? crypto.randomUUID()
      : `${widget.id}-copy-${Date.now()}`;
    const maxY = Math.max(0, ...dashboard.layout.map(w => w.y + w.h));
    const copy = { ...widget, id: newId, title: `${widget.title} (copy)`, x: 0, y: maxY } as Widget;
    onUpdateWidgets([...dashboard.layout, copy]);
  };
  const handleRenameWidget = (id: string, nextTitle: string) => {
    const trimmed = nextTitle.trim();
    if (!trimmed) return;
    onUpdateWidgets(dashboard.layout.map(w => w.id === id ? ({ ...w, title: trimmed } as Widget) : w));
  };
  const layouts = {
    lg: dashboard.layout.map(w => ({
      i: w.id,
      x: w.x,
      y: w.y,
      w: w.w,
      h: w.h,
    })),
  };

  const handleLayoutCommit = (layout: Layout[]) => {
    const byId = new Map(layout.map(item => [item.i, item]));
    const nextLayout = dashboard.layout.map(widget => {
      const item = byId.get(widget.id);
      if (!item) return widget;
      return { ...widget, x: item.x, y: item.y, w: item.w, h: item.h };
    });
    onLayoutCommit(nextLayout);
  };

  if (dashboard.layout.length === 0) {
    return (
      <div className="flex h-full min-h-[320px] items-center justify-center rounded-lg border border-dashed border-border bg-muted/20 text-center">
        <div className="max-w-sm px-6">
          <h2 className="text-sm font-medium text-foreground">No widgets yet</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            This dashboard is saved locally. Widgets will appear here after a workflow or build step adds them.
          </p>
          <div className="mt-4 flex justify-center gap-2">
            <button onClick={() => onAddWidget('text')} className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted">Add text</button>
            <button onClick={() => onAddWidget('gauge')} className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted">Add gauge</button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex justify-end gap-2">
        <button onClick={() => onAddWidget('text')} className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted">Add text</button>
        <button onClick={() => onAddWidget('gauge')} className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted">Add gauge</button>
      </div>
      <ResponsiveGridLayout
        className="layout"
        layouts={layouts}
        breakpoints={{ lg: 1200, md: 996, sm: 768, xs: 480, xxs: 0 }}
        cols={{ lg: 12, md: 10, sm: 6, xs: 4, xxs: 2 }}
        rowHeight={60}
        draggableHandle=".widget-drag-handle"
        margin={[12, 12]}
        onDragStop={handleLayoutCommit}
        onResizeStop={handleLayoutCommit}
      >
        {dashboard.layout.map(widget => {
          const run = workflowRuns[widget.datasource?.workflow_id ?? ''];
          const data = widgetData[widget.id];
          const fallback = isFallback(data);
          const refreshing = refreshingWidgetId === widget.id;
          return (
            <div key={widget.id} className="bg-card rounded-xl border border-border shadow-sm overflow-hidden flex flex-col">
              <div className="widget-drag-handle flex items-center justify-between px-3 py-2 border-b border-border/50 cursor-move">
                {editingTitleId === widget.id ? (
                  <TitleEditor
                    initial={widget.title}
                    onSave={value => { handleRenameWidget(widget.id, value); setEditingTitleId(null); }}
                    onCancel={() => setEditingTitleId(null)}
                  />
                ) : (
                  <span
                    className="text-sm font-medium truncate"
                    onDoubleClick={event => { event.stopPropagation(); setEditingTitleId(widget.id); }}
                    title="Double-click to rename"
                  >
                    {widget.title}
                  </span>
                )}
                <div className="flex items-center gap-1">
                  <WorkflowBadge widget={widget} run={run} fallback={fallback} />
                  <button
                    className="p-1 rounded hover:bg-muted disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                    title="Refresh widget data"
                    disabled={refreshing}
                    onMouseDown={event => event.stopPropagation()}
                    onClick={() => onRefreshWidget(widget.id)}
                  >
                    <svg className={`w-3.5 h-3.5 text-muted-foreground ${refreshing ? 'animate-spin' : ''}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                  </button>
                  <WidgetMenu
                    onRename={() => setEditingTitleId(widget.id)}
                    onDuplicate={() => handleDuplicateWidget(widget.id)}
                    onDelete={() => handleDeleteWidget(widget.id)}
                    onInspect={() => setInspecting({ widget, data, run })}
                  />
                </div>
              </div>
              <div className="p-3 flex-1 overflow-auto min-h-0">
                {widgetErrors[widget.id] ? (
                  <WidgetError message={widgetErrors[widget.id]} />
                ) : (
                  <WidgetRenderer widget={widget} data={data} />
                )}
              </div>
              <WidgetFooter widget={widget} run={run} />
            </div>
          );
        })}
      </ResponsiveGridLayout>
      {inspecting && (
        <InspectModal
          widget={inspecting.widget}
          data={inspecting.data}
          run={inspecting.run}
          onClose={() => setInspecting(null)}
        />
      )}
    </div>
  );
}

function InspectModal({
  widget,
  data,
  run,
  onClose,
}: {
  widget: Widget;
  data?: WidgetRuntimeData;
  run?: WorkflowRun;
  onClose: () => void;
}) {
  const dataJson = data ? JSON.stringify(data, null, 2) : 'No runtime data captured.';
  const runJson = run ? JSON.stringify(run, null, 2) : 'No workflow run recorded yet.';
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex max-h-[80vh] w-[min(90vw,52rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="min-w-0">
            <p className="text-sm font-medium truncate">{widget.title}</p>
            <p className="text-[11px] text-muted-foreground truncate">{widget.type} - id {widget.id}</p>
          </div>
          <button onClick={onClose} className="p-1 rounded hover:bg-muted">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="flex-1 overflow-auto p-4 space-y-4">
          <Section title="Runtime data" copyable={dataJson}>
            <pre className="max-h-72 overflow-auto rounded bg-background/70 p-2 text-[11px] font-mono">{dataJson}</pre>
          </Section>
          <Section title="Last workflow run" copyable={runJson}>
            <pre className="max-h-72 overflow-auto rounded bg-background/70 p-2 text-[11px] font-mono">{runJson}</pre>
          </Section>
          <Section title="Widget config" copyable={JSON.stringify(widget, null, 2)}>
            <pre className="max-h-72 overflow-auto rounded bg-background/70 p-2 text-[11px] font-mono">{JSON.stringify(widget, null, 2)}</pre>
          </Section>
        </div>
      </div>
    </div>
  );
}

function Section({ title, children, copyable }: { title: string; children: React.ReactNode; copyable?: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    if (!copyable) return;
    try {
      await navigator.clipboard.writeText(copyable);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch (err) {
      console.error('Copy failed:', err);
    }
  };
  return (
    <div>
      <div className="mb-1 flex items-center justify-between">
        <p className="text-[11px] uppercase tracking-wide text-muted-foreground">{title}</p>
        {copyable && (
          <button onClick={onCopy} className="text-[11px] text-muted-foreground hover:text-foreground">
            {copied ? 'Copied' : 'Copy'}
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

function WorkflowBadge({ widget, run, fallback }: { widget: Widget; run?: WorkflowRun; fallback: boolean }) {
  if (!widget.datasource && !fallback) return null;
  const status = fallback ? 'fallback' : (run?.status ?? 'idle');
  const title = fallback
    ? 'Data did not match the widget shape; showing raw output'
    : run?.error ? `Last refresh failed: ${run.error}` : `Last refresh: ${status}`;
  const tone = status === 'success'
    ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400'
    : status === 'error'
      ? 'bg-destructive/15 text-destructive'
      : status === 'running'
        ? 'bg-blue-500/15 text-blue-700 dark:text-blue-400'
        : status === 'fallback'
          ? 'bg-amber-500/15 text-amber-700 dark:text-amber-400'
          : 'bg-muted text-muted-foreground';
  return (
    <span title={title} className={`rounded px-1.5 py-0.5 text-[10px] ${tone}`}>
      {status}
    </span>
  );
}

function WidgetMenu({
  onRename,
  onDuplicate,
  onDelete,
  onInspect,
}: {
  onRename: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onInspect: () => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [open]);
  return (
    <div ref={ref} className="relative">
      <button
        onMouseDown={e => e.stopPropagation()}
        onClick={() => setOpen(v => !v)}
        title="Widget actions"
        className="p-1 rounded hover:bg-muted transition-colors"
      >
        <svg className="w-3.5 h-3.5 text-muted-foreground" viewBox="0 0 20 20" fill="currentColor">
          <path d="M10 3a1.5 1.5 0 110 3 1.5 1.5 0 010-3zm0 5.5a1.5 1.5 0 110 3 1.5 1.5 0 010-3zM10 14a1.5 1.5 0 110 3 1.5 1.5 0 010-3z" />
        </svg>
      </button>
      {open && (
        <div className="absolute right-0 top-7 z-40 w-44 rounded-md border border-border bg-popover py-1 shadow-lg">
          <MenuItem label="Rename" onClick={() => { setOpen(false); onRename(); }} />
          <MenuItem label="Duplicate" onClick={() => { setOpen(false); onDuplicate(); }} />
          <MenuItem label="View raw data" onClick={() => { setOpen(false); onInspect(); }} />
          <div className="my-1 h-px bg-border/60" />
          <MenuItem label="Delete" destructive onClick={() => { setOpen(false); onDelete(); }} />
        </div>
      )}
    </div>
  );
}

function MenuItem({ label, onClick, destructive }: { label: string; onClick: () => void; destructive?: boolean }) {
  return (
    <button
      onMouseDown={e => e.stopPropagation()}
      onClick={onClick}
      className={`block w-full px-3 py-1.5 text-left text-[12px] ${destructive ? 'text-destructive hover:bg-destructive/10' : 'text-foreground hover:bg-muted'}`}
    >
      {label}
    </button>
  );
}

function TitleEditor({
  initial,
  onSave,
  onCancel,
}: {
  initial: string;
  onSave: (next: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <input
      autoFocus
      value={value}
      onChange={e => setValue(e.target.value)}
      onMouseDown={e => e.stopPropagation()}
      onClick={e => e.stopPropagation()}
      onKeyDown={e => {
        if (e.key === 'Enter') onSave(value);
        if (e.key === 'Escape') onCancel();
      }}
      onBlur={() => onSave(value)}
      className="w-full rounded border border-border bg-background px-1.5 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-primary/40"
    />
  );
}

function WidgetFooter({ widget, run }: { widget: Widget; run?: WorkflowRun }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 15_000);
    return () => window.clearInterval(id);
  }, []);
  if (!widget.datasource && !run) return null;
  const lastTs = run?.finished_at ?? run?.started_at;
  const last = lastTs ? relativeTime(now, lastTs) : 'never';
  const durationMs = run?.finished_at && run?.started_at ? run.finished_at - run.started_at : undefined;
  const errored = run?.status === 'error';
  return (
    <div className="flex items-center justify-between gap-2 border-t border-border/40 px-3 py-1 text-[10px] text-muted-foreground">
      <span className={errored ? 'text-destructive' : ''}>
        {errored ? `Failed ${last}` : `Updated ${last}`}
      </span>
      {durationMs !== undefined && durationMs > 0 && (
        <span className="tabular-nums opacity-70">{Math.round(durationMs)}ms</span>
      )}
    </div>
  );
}

function relativeTime(now: number, ts: number): string {
  const delta = Math.max(0, now - ts);
  if (delta < 5_000) return 'just now';
  if (delta < 60_000) return `${Math.round(delta / 1000)}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

function isFallback(data?: WidgetRuntimeData): boolean {
  if (!data) return false;
  return (data as unknown as { fallback?: boolean }).fallback === true;
}

function WidgetRenderer({ widget, data }: { widget: Dashboard['layout'][number]; data?: WidgetRuntimeData }) {
  switch (widget.type) {
    case 'chart': return <ChartWidget config={widget.config} data={data?.kind === 'chart' ? data : undefined} />;
    case 'text': return <TextWidget config={widget.config} data={data?.kind === 'text' ? data : undefined} />;
    case 'table': return <TableWidget config={widget.config} data={data?.kind === 'table' ? data : undefined} />;
    case 'gauge': return <GaugeWidget config={widget.config} data={data?.kind === 'gauge' ? data : undefined} />;
    case 'image': return <ImageWidget config={widget.config} data={data?.kind === 'image' ? data : undefined} />;
    case 'stat': return <StatWidget config={widget.config} data={data?.kind === 'stat' ? data : undefined} />;
    case 'logs': return <LogsWidget config={widget.config} data={data?.kind === 'logs' ? data : undefined} />;
    case 'bar_gauge': return <BarGaugeWidget config={widget.config} data={data?.kind === 'bar_gauge' ? data : undefined} />;
    case 'status_grid': return <StatusGridWidget config={widget.config} data={data?.kind === 'status_grid' ? data : undefined} />;
    case 'heatmap': return <HeatmapWidget config={widget.config} data={data?.kind === 'heatmap' ? data : undefined} />;
    default: return <div className="text-muted-foreground text-sm">Unknown widget type</div>;
  }
}

function WidgetError({ message }: { message?: string }) {
  return (
    <div className="flex h-full min-h-24 items-center justify-center rounded-md border border-destructive/30 bg-destructive/5 p-3 text-center text-xs text-destructive">
      {message}
    </div>
  );
}
