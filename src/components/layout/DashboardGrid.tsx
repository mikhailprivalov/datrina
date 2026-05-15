import { Responsive, WidthProvider } from 'react-grid-layout';
import 'react-grid-layout/css/styles.css';
import 'react-resizable/css/styles.css';
import type { Dashboard, Widget, WidgetRuntimeData, WorkflowRun } from '../../lib/api';
import { ChartWidget } from '../widgets/ChartWidget';
import { TextWidget } from '../widgets/TextWidget';
import { TableWidget } from '../widgets/TableWidget';
import { GaugeWidget } from '../widgets/GaugeWidget';
import { ImageWidget } from '../widgets/ImageWidget';
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
}: Props) {
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
        {dashboard.layout.map(widget => (
          <div key={widget.id} className="bg-card rounded-xl border border-border shadow-sm overflow-hidden">
            <div className="widget-drag-handle flex items-center justify-between px-3 py-2 border-b border-border/50 cursor-move">
              <span className="text-sm font-medium truncate">{widget.title}</span>
              <div className="flex items-center gap-1">
                <WorkflowBadge widget={widget} run={workflowRuns[widget.datasource?.workflow_id ?? '']} />
                <button
                  className="p-1 rounded hover:bg-muted disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                  title="Refresh widget data"
                  disabled={refreshingWidgetId === widget.id}
                  onMouseDown={event => event.stopPropagation()}
                  onClick={() => onRefreshWidget(widget.id)}
                >
                  <svg className={`w-3.5 h-3.5 text-muted-foreground ${refreshingWidgetId === widget.id ? 'animate-spin' : ''}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                  </svg>
                </button>
              </div>
            </div>
            <div className="p-3 h-[calc(100%-40px)] overflow-auto">
              {widgetErrors[widget.id] ? (
                <WidgetError message={widgetErrors[widget.id]} />
              ) : (
                <WidgetRenderer widget={widget} data={widgetData[widget.id]} />
              )}
            </div>
          </div>
        ))}
      </ResponsiveGridLayout>
    </div>
  );
}

function WorkflowBadge({ widget, run }: { widget: Widget; run?: WorkflowRun }) {
  if (!widget.datasource) {
    return null;
  }
  const status = run?.status ?? 'idle';
  const title = run?.error ? `Last refresh failed: ${run.error}` : `Last refresh: ${status}`;
  const tone = status === 'success'
    ? 'bg-emerald-500/15 text-emerald-700'
    : status === 'error'
      ? 'bg-destructive/15 text-destructive'
      : status === 'running'
        ? 'bg-blue-500/15 text-blue-700'
        : 'bg-muted text-muted-foreground';
  return (
    <span title={title} className={`rounded px-1.5 py-0.5 text-[10px] ${tone}`}>
      {status}
    </span>
  );
}

function WidgetRenderer({ widget, data }: { widget: Dashboard['layout'][number]; data?: WidgetRuntimeData }) {
  switch (widget.type) {
    case 'chart': return <ChartWidget config={widget.config} data={data?.kind === 'chart' ? data : undefined} />;
    case 'text': return <TextWidget config={widget.config} data={data?.kind === 'text' ? data : undefined} />;
    case 'table': return <TableWidget config={widget.config} data={data?.kind === 'table' ? data : undefined} />;
    case 'gauge': return <GaugeWidget config={widget.config} data={data?.kind === 'gauge' ? data : undefined} />;
    case 'image': return <ImageWidget config={widget.config} data={data?.kind === 'image' ? data : undefined} />;
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
