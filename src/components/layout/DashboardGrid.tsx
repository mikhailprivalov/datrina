import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { readParameterHash, writeParameterHash } from '../../lib/parameterHash';
import { Responsive, WidthProvider } from 'react-grid-layout';
import 'react-grid-layout/css/styles.css';
import 'react-resizable/css/styles.css';
import type {
  Dashboard,
  DashboardModelPolicy,
  LLMProvider,
  ScheduleDisplayState,
  Widget,
  WidgetCapability,
  WidgetModelOverride,
  WidgetProvenance,
  WidgetRuntimeData,
  WidgetStreamState,
  WorkflowRun,
  WorkflowSummary,
  AlertSeverity,
} from '../../lib/api';
import { dashboardApi, operationsApi, provenanceApi, scheduleApi } from '../../lib/api';
import { ScheduleEditor } from '../schedule/ScheduleEditor';
import { AssistantLanguagePicker } from '../settings/AssistantLanguagePicker';
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
import { GalleryWidget } from '../widgets/GalleryWidget';
import { PipelineDebugModal } from '../debug/PipelineDebugModal';
import { ParameterBar } from '../dashboard/ParameterBar';
import type { Layout } from 'react-grid-layout';

const ResponsiveGridLayout = WidthProvider(Responsive);

export interface WidgetAlertStatus {
  count: number;
  severity: AlertSeverity;
}

interface Props {
  dashboard: Dashboard;
  widgetData: Record<string, WidgetRuntimeData | undefined>;
  /** W36: per-widget snapshot capture timestamp. Non-null = the value
   *  shown is the cached last-known-good while a live refresh runs. */
  widgetCachedAt?: Record<string, number | undefined>;
  widgetErrors: Record<string, string | undefined>;
  /** W42: per-widget live streaming state. Partial text is displayed
   *  but never persisted as the widget's committed runtime value. */
  widgetStream?: Record<string, WidgetStreamState | undefined>;
  workflowRuns: Record<string, WorkflowRun | undefined>;
  refreshingWidgetId: string | null;
  onRefreshWidget: (widgetId: string) => void;
  onLayoutCommit: (layout: Widget[]) => void;
  onAddWidget: (widgetType: 'text' | 'gauge') => void;
  onUpdateWidgets: (next: Widget[]) => void;
  onOpenHistory: () => void;
  widgetAlertStatus?: Record<string, WidgetAlertStatus | undefined>;
  onOpenAlertsEditor?: (widgetId: string) => void;
  /** W43: providers powering the dashboard/widget model policy editors.
   *  Empty list disables the controls (with a "no providers" hint). */
  providers?: LLMProvider[];
  /** W43: full replacement callback from policy/override mutations.
   *  The command returns the updated dashboard, which `App` then merges
   *  into its loaded list so the next refresh sees the change. */
  onDashboardChange?: (dashboard: Dashboard) => void;
}

export function DashboardGrid({
  dashboard,
  widgetData,
  widgetCachedAt,
  widgetErrors,
  widgetStream,
  workflowRuns,
  refreshingWidgetId,
  onRefreshWidget,
  onLayoutCommit,
  onAddWidget,
  onUpdateWidgets,
  onOpenHistory,
  widgetAlertStatus,
  onOpenAlertsEditor,
  providers,
  onDashboardChange,
}: Props) {
  const [inspecting, setInspecting] = useState<{ widget: Widget; data?: WidgetRuntimeData; run?: WorkflowRun } | null>(null);
  const [debuggingWidget, setDebuggingWidget] = useState<Widget | null>(null);
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  // W34: read parameter selections from URL hash once per dashboard mount.
  // The hash is the only persistence we read here — listParameters already
  // hydrates from the persisted SQLite selections, and ParameterBar will
  // commit any hash-supplied values that differ from what is persisted.
  const initialParameterSelections = useMemo(
    () => readParameterHash(dashboard.id) ?? undefined,
    [dashboard.id],
  );
  // W40: keep handler identities stable across parent renders so
  // `WidgetCell` (memoized) only rerenders when its own per-widget
  // props change. Reaching into the current layout via a ref avoids
  // closing over the previous dashboard snapshot.
  const layoutRef = useRef(dashboard.layout);
  layoutRef.current = dashboard.layout;
  const handleDeleteWidget = useCallback((id: string) => {
    if (!window.confirm('Delete this widget? Its workflow stays so you can re-add it later.')) return;
    onUpdateWidgets(layoutRef.current.filter(w => w.id !== id));
  }, [onUpdateWidgets]);
  const handleDuplicateWidget = useCallback((id: string) => {
    const current = layoutRef.current;
    const widget = current.find(w => w.id === id);
    if (!widget) return;
    const newId = (typeof crypto !== 'undefined' && 'randomUUID' in crypto)
      ? crypto.randomUUID()
      : `${widget.id}-copy-${Date.now()}`;
    const maxY = Math.max(0, ...current.map(w => w.y + w.h));
    const copy = { ...widget, id: newId, title: `${widget.title} (copy)`, x: 0, y: maxY } as Widget;
    onUpdateWidgets([...current, copy]);
  }, [onUpdateWidgets]);
  const handleRenameWidget = useCallback((id: string, nextTitle: string) => {
    const trimmed = nextTitle.trim();
    if (!trimmed) return;
    onUpdateWidgets(
      layoutRef.current.map(w => (w.id === id ? ({ ...w, title: trimmed } as Widget) : w)),
    );
  }, [onUpdateWidgets]);
  const handleInspect = useCallback((id: string) => {
    const widget = layoutRef.current.find(w => w.id === id);
    if (!widget) return;
    const data = widgetData[id];
    const run = workflowRuns[widget.datasource?.workflow_id ?? ''];
    setInspecting({ widget, data, run });
  }, [widgetData, workflowRuns]);
  const dashboardIdForInspect = dashboard.id;
  const handleDebug = useCallback((id: string) => {
    const widget = layoutRef.current.find(w => w.id === id);
    if (widget) setDebuggingWidget(widget);
  }, []);
  const handleStartRename = useCallback((id: string) => setEditingTitleId(id), []);
  const handleStopRename = useCallback(() => setEditingTitleId(null), []);
  const handleOpenAlerts = useMemo(() => {
    if (!onOpenAlertsEditor) return undefined;
    return (id: string) => onOpenAlertsEditor(id);
  }, [onOpenAlertsEditor]);
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
      <div className="grid-backdrop flex h-full min-h-[320px] items-center justify-center rounded-md border border-dashed border-border bg-muted/10 text-center">
        <div className="max-w-sm px-6 panel py-6 shadow-sm">
          <p className="mono text-[10px] uppercase tracking-[0.2em] text-primary">empty dashboard</p>
          <h2 className="mt-2 text-sm font-medium text-foreground">No widgets yet</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            Saved locally. Widgets appear after a workflow or Build step adds them.
          </p>
          <div className="mt-4 flex justify-center gap-2">
            <button onClick={() => onAddWidget('text')} className="rounded-md border border-border bg-muted/40 px-3 py-1.5 text-xs hover:bg-muted hover:border-primary/40 transition-colors">+ Text</button>
            <button onClick={() => onAddWidget('gauge')} className="rounded-md border border-border bg-muted/40 px-3 py-1.5 text-xs hover:bg-muted hover:border-primary/40 transition-colors">+ Gauge</button>
          </div>
        </div>
      </div>
    );
  }

  const toolbarBtn = 'rounded-md border border-border bg-muted/30 px-2.5 py-1 text-xs hover:bg-muted hover:border-primary/40 transition-colors mono uppercase tracking-wider';
  return (
    <div className="space-y-3">
      <ParameterBar
        dashboardId={dashboard.id}
        parameters={dashboard.parameters ?? []}
        onAffectedWidgets={ids => ids.forEach(id => onRefreshWidget(id))}
        initialSelections={initialParameterSelections}
        onSelectionChange={values => writeParameterHash(dashboard.id, values)}
      />
      <div className="flex flex-wrap items-center justify-end gap-1.5">
        <DashboardScheduleControl dashboard={dashboard} />
        <DashboardModelPolicyControl
          dashboard={dashboard}
          providers={providers ?? []}
          onChange={onDashboardChange}
        />
        <DashboardLanguagePolicyControl
          dashboard={dashboard}
          onChange={onDashboardChange}
        />
        <button onClick={onOpenHistory} className={toolbarBtn} title="View dashboard history and restore prior versions">
          History
        </button>
        <button onClick={() => onAddWidget('text')} className={toolbarBtn}>+ Text</button>
        <button onClick={() => onAddWidget('gauge')} className={toolbarBtn}>+ Gauge</button>
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
          <div key={widget.id} className="group/widget bg-card rounded-md border border-border hover:border-primary/30 shadow-sm overflow-hidden flex flex-col transition-colors">
            <WidgetCell
              widget={widget}
              data={widgetData[widget.id]}
              error={widgetErrors[widget.id]}
              run={workflowRuns[widget.datasource?.workflow_id ?? '']}
              refreshing={refreshingWidgetId === widget.id}
              cachedAt={widgetCachedAt?.[widget.id]}
              streamState={widgetStream?.[widget.id]}
              alertStatus={widgetAlertStatus?.[widget.id]}
              isEditingTitle={editingTitleId === widget.id}
              onRefresh={onRefreshWidget}
              onStartRename={handleStartRename}
              onStopRename={handleStopRename}
              onRenameSave={handleRenameWidget}
              onDuplicate={handleDuplicateWidget}
              onDelete={handleDeleteWidget}
              onInspect={handleInspect}
              onOpenAlerts={handleOpenAlerts}
              onDebug={handleDebug}
            />
          </div>
        ))}
      </ResponsiveGridLayout>
      {inspecting && (
        <InspectModal
          dashboardId={dashboardIdForInspect}
          widget={inspecting.widget}
          data={inspecting.data}
          run={inspecting.run}
          providers={providers ?? []}
          dashboardPolicy={dashboard.model_policy ?? null}
          onDashboardChange={onDashboardChange}
          onOpenDebug={() => {
            setDebuggingWidget(inspecting.widget);
            setInspecting(null);
          }}
          onClose={() => setInspecting(null)}
        />
      )}
      {debuggingWidget && (
        <PipelineDebugModal
          dashboardId={dashboard.id}
          widgetId={debuggingWidget.id}
          widgetTitle={debuggingWidget.title}
          initialCaptureTraces={!!debuggingWidget.datasource?.capture_traces}
          onClose={() => setDebuggingWidget(null)}
          onCaptureChange={next => {
            onUpdateWidgets(
              dashboard.layout.map(w =>
                w.id === debuggingWidget.id && w.datasource
                  ? { ...w, datasource: { ...w.datasource, capture_traces: next } }
                  : w
              )
            );
          }}
        />
      )}
    </div>
  );
}

function InspectModal({
  dashboardId,
  widget,
  data,
  run,
  providers,
  dashboardPolicy,
  onDashboardChange,
  onOpenDebug,
  onClose,
}: {
  dashboardId: string;
  widget: Widget;
  data?: WidgetRuntimeData;
  run?: WorkflowRun;
  providers: LLMProvider[];
  dashboardPolicy: DashboardModelPolicy | null;
  onDashboardChange?: (dashboard: Dashboard) => void;
  onOpenDebug: () => void;
  onClose: () => void;
}) {
  const dataJson = data ? JSON.stringify(data, null, 2) : 'No runtime data captured.';
  const runJson = run ? JSON.stringify(run, null, 2) : 'No workflow run recorded yet.';
  const [provenance, setProvenance] = useState<WidgetProvenance | null>(null);
  const [provenanceError, setProvenanceError] = useState<string | null>(null);
  const [loadingProvenance, setLoadingProvenance] = useState(true);
  useEffect(() => {
    let cancelled = false;
    setLoadingProvenance(true);
    setProvenanceError(null);
    provenanceApi
      .getWidget(dashboardId, widget.id)
      .then(result => {
        if (cancelled) return;
        setProvenance(result);
      })
      .catch(err => {
        if (cancelled) return;
        setProvenanceError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingProvenance(false);
      });
    return () => {
      cancelled = true;
    };
  }, [dashboardId, widget.id]);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 backdrop-blur-sm">
      <div className="flex max-h-[80vh] w-[min(90vw,52rem)] flex-col rounded-md border border-border bg-card shadow-2xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3 bg-muted/30">
          <div className="min-w-0">
            <p className="text-sm font-semibold truncate">{widget.title}</p>
            <p className="text-[10px] mono uppercase tracking-wider text-muted-foreground truncate">{widget.type} · id <span className="text-foreground/80">{widget.id}</span></p>
          </div>
          <button onClick={onClose} className="p-1 rounded hover:bg-muted hover:text-foreground transition-colors">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="flex-1 overflow-auto p-4 space-y-4">
          <ProvenancePanel
            loading={loadingProvenance}
            error={provenanceError}
            provenance={provenance}
            onOpenDebug={provenance?.links.has_pipeline_traces || widget.datasource ? onOpenDebug : undefined}
          />
          {provenance && provenance.llm_participation !== 'none' && (
            <WidgetModelOverrideEditor
              dashboardId={dashboardId}
              widget={widget}
              providers={providers}
              dashboardPolicy={dashboardPolicy}
              onChange={onDashboardChange}
            />
          )}
          {provenance && provenance.llm_participation === 'none' && (
            <div className="rounded-md border border-dashed border-border/60 bg-muted/20 p-3 text-[11px] mono text-muted-foreground">
              No LLM in this widget — model controls hidden.
            </div>
          )}
          <Section title="Runtime data" copyable={dataJson}>
            <pre className="max-h-72 overflow-auto rounded bg-muted/40 border border-border/60 p-2 text-[11px] mono">{dataJson}</pre>
          </Section>
          <Section title="Last workflow run" copyable={runJson}>
            <pre className="max-h-72 overflow-auto rounded bg-muted/40 border border-border/60 p-2 text-[11px] mono">{runJson}</pre>
          </Section>
          <Section title="Widget config" copyable={JSON.stringify(widget, null, 2)}>
            <pre className="max-h-72 overflow-auto rounded bg-muted/40 border border-border/60 p-2 text-[11px] mono">{JSON.stringify(widget, null, 2)}</pre>
          </Section>
        </div>
      </div>
    </div>
  );
}

function ProvenancePanel({
  loading,
  error,
  provenance,
  onOpenDebug,
}: {
  loading: boolean;
  error: string | null;
  provenance: WidgetProvenance | null;
  onOpenDebug?: () => void;
}) {
  return (
    <div className="rounded-md border border-border/60 bg-muted/20 p-3">
      <div className="mb-2 flex items-center justify-between">
        <p className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// provenance</p>
        {provenance && (
          <LlmBadge participation={provenance.llm_participation} />
        )}
      </div>
      {loading && (
        <p className="text-[11px] text-muted-foreground mono">Loading…</p>
      )}
      {!loading && error && (
        <p className="text-[11px] text-destructive mono break-words">Could not load provenance: {error}</p>
      )}
      {!loading && !error && provenance && (
        <div className="space-y-2 text-[11px] text-foreground">
          <ProvenanceRow label="Widget">
            <span className="mono">{provenance.widget_kind}</span>
            <span className="text-muted-foreground"> · </span>
            <span className="mono text-foreground/80">{provenance.widget_id}</span>
          </ProvenanceRow>
          <ProvenanceDatasourceRow provenance={provenance} />
          {provenance.provider && (
            <ProvenanceRow label="Provider">
              <span className="mono">{provenance.provider.provider_name}</span>
              <span className="text-muted-foreground"> ({provenance.provider.provider_kind})</span>
              <span className="text-muted-foreground"> · model </span>
              <span className="mono text-foreground/80">{provenance.provider.model}</span>
              {provenance.provider.model_source && (
                <ModelSourceBadge source={provenance.provider.model_source} />
              )}
              {provenance.provider.required_caps && provenance.provider.required_caps.length > 0 && (
                <span className="ml-2 text-[9px] mono uppercase tracking-wider text-muted-foreground">
                  caps: {provenance.provider.required_caps.join(', ')}
                </span>
              )}
            </ProvenanceRow>
          )}
          <ProvenanceRow label="Pipeline">
            <span className="mono">{provenance.tail.step_count} step(s)</span>
            {provenance.tail.has_llm_postprocess && (
              <span className="ml-2 rounded-sm border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-primary">llm_postprocess</span>
            )}
            {provenance.tail.has_mcp_call && (
              <span className="ml-2 rounded-sm border border-neon-amber/40 bg-neon-amber/10 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-neon-amber">mcp_call</span>
            )}
          </ProvenanceRow>
          {provenance.last_run ? (
            <ProvenanceRow label="Last run">
              <span className="mono">{provenance.last_run.status}</span>
              {typeof provenance.last_run.duration_ms === 'number' && (
                <span className="text-muted-foreground"> · {provenance.last_run.duration_ms}ms</span>
              )}
              {provenance.last_run.error && (
                <span className="ml-2 text-destructive truncate">{provenance.last_run.error}</span>
              )}
            </ProvenanceRow>
          ) : (
            <ProvenanceRow label="Last run">
              <span className="text-muted-foreground italic">not captured</span>
            </ProvenanceRow>
          )}
          <div className="flex flex-wrap items-center gap-2 pt-1">
            {provenance.links.datasource_definition_id && (
              <a
                href={`#/workbench/${provenance.links.datasource_definition_id}`}
                className="rounded-sm border border-border bg-muted/40 px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-foreground hover:border-primary/40 hover:text-primary transition-colors"
              >
                Open in Workbench
              </a>
            )}
            {provenance.links.workflow_id && (
              <a
                href={`#/operations?workflow=${encodeURIComponent(provenance.links.workflow_id)}`}
                className="rounded-sm border border-border bg-muted/40 px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-foreground hover:border-primary/40 hover:text-primary transition-colors"
              >
                Open in Operations
              </a>
            )}
            {onOpenDebug && (
              <button
                onClick={onOpenDebug}
                className="rounded-sm border border-border bg-muted/40 px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-foreground hover:border-primary/40 hover:text-primary transition-colors"
              >
                Pipeline Debug…
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function LlmBadge({ participation }: { participation: WidgetProvenance['llm_participation'] }) {
  const map: Record<WidgetProvenance['llm_participation'], { label: string; tone: string; title: string }> = {
    none: {
      label: 'no LLM',
      tone: 'border-border bg-muted/40 text-muted-foreground',
      title: 'This widget runs a deterministic pipeline only.',
    },
    provider_source: {
      label: 'LLM source',
      tone: 'border-primary/40 bg-primary/15 text-primary',
      title: 'A provider prompt produces the data shown by this widget.',
    },
    llm_postprocess: {
      label: 'LLM postprocess',
      tone: 'border-primary/40 bg-primary/15 text-primary',
      title: 'A deterministic source is shaped by one or more llm_postprocess steps.',
    },
    widget_text_generation: {
      label: 'LLM-generated text',
      tone: 'border-primary/40 bg-primary/15 text-primary',
      title: 'Text widget content was generated by the LLM directly.',
    },
    unknown: {
      label: 'LLM unknown',
      tone: 'border-neon-amber/40 bg-neon-amber/15 text-neon-amber',
      title: 'Datasource workflow could not be resolved — LLM participation cannot be inferred.',
    },
  };
  const entry = map[participation];
  return (
    <span title={entry.title} className={`rounded-sm border px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider ${entry.tone}`}>
      {entry.label}
    </span>
  );
}

function ProvenanceRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-2">
      <span className="w-20 shrink-0 text-[9px] mono uppercase tracking-[0.18em] text-muted-foreground">{label}</span>
      <span className="flex-1 min-w-0 truncate">{children}</span>
    </div>
  );
}

function ProvenanceDatasourceRow({ provenance }: { provenance: WidgetProvenance }) {
  const ds = provenance.datasource;
  if (!ds) {
    return (
      <ProvenanceRow label="Source">
        <span className="text-muted-foreground italic">no datasource bound</span>
      </ProvenanceRow>
    );
  }
  const source = ds.source;
  let label: React.ReactNode;
  switch (source.kind) {
    case 'mcp_tool':
      label = (
        <>
          <span className="mono">mcp_tool</span>
          <span className="text-muted-foreground"> · </span>
          <span className="mono">{source.server_id}::{source.tool_name}</span>
        </>
      );
      break;
    case 'builtin_tool':
      label = (
        <>
          <span className="mono">builtin_tool</span>
          <span className="text-muted-foreground"> · </span>
          <span className="mono">{source.tool_name}</span>
        </>
      );
      break;
    case 'provider_prompt':
      label = (
        <>
          <span className="mono">provider_prompt</span>
          <span className="text-muted-foreground"> · </span>
          <span className="truncate" title={source.prompt_preview}>“{source.prompt_preview}”</span>
        </>
      );
      break;
    case 'compose':
      label = (
        <>
          <span className="mono">compose</span>
          <span className="text-muted-foreground"> · {source.inputs.length} input(s)</span>
        </>
      );
      break;
    case 'missing':
      label = (
        <span className="text-neon-amber">missing workflow {source.workflow_id}</span>
      );
      break;
    case 'unknown':
      label = <span className="text-neon-amber italic">unknown source shape</span>;
      break;
    default:
      label = <span className="text-muted-foreground">—</span>;
  }
  return (
    <>
      <ProvenanceRow label="Source">{label}</ProvenanceRow>
      {ds.datasource_name && (
        <ProvenanceRow label="Datasource">
          <span className="mono">{ds.datasource_name}</span>
          {ds.binding_source && (
            <span className="ml-2 text-[9px] mono uppercase tracking-wider text-muted-foreground">via {ds.binding_source}</span>
          )}
        </ProvenanceRow>
      )}
      {ds.refresh_cron && (
        <ProvenanceRow label="Schedule">
          <span className="mono">{ds.refresh_cron}</span>
          {ds.pause_state === 'paused' && (
            <span className="ml-2 rounded-sm border border-amber-500/40 bg-amber-500/10 px-1.5 py-0.5 mono text-[10px] uppercase tracking-wider text-amber-300">
              paused
            </span>
          )}
        </ProvenanceRow>
      )}
    </>
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
        <p className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// {title}</p>
        {copyable && (
          <button onClick={onCopy} className="text-[10px] mono uppercase tracking-wider text-muted-foreground hover:text-primary transition-colors">
            {copied ? 'Copied' : 'Copy'}
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

function AlertDot({ status }: { status?: WidgetAlertStatus }) {
  if (!status || status.count <= 0) return null;
  const tone =
    status.severity === 'critical'
      ? 'bg-destructive glow-destructive'
      : status.severity === 'warning'
        ? 'bg-neon-amber'
        : 'bg-primary';
  return (
    <span
      title={`${status.count} unacknowledged alert${status.count === 1 ? '' : 's'} (${status.severity})`}
      className={`mr-1 inline-flex h-2 w-2 rounded-full ${tone}`}
      aria-label="Active alerts"
    />
  );
}

function CachedBadge({ capturedAt }: { capturedAt: number }) {
  const ageMs = Math.max(0, Date.now() - capturedAt);
  const ageLabel = formatCachedAge(ageMs);
  const isoLabel = new Date(capturedAt).toISOString();
  return (
    <span
      title={`Showing cached value from ${isoLabel}; live refresh in progress`}
      className="rounded-sm border border-neon-amber/30 bg-neon-amber/15 px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider text-neon-amber"
    >
      cached · {ageLabel}
    </span>
  );
}

function formatCachedAge(ms: number): string {
  if (ms < 60_000) return `${Math.max(1, Math.round(ms / 1000))}s`;
  if (ms < 60 * 60_000) return `${Math.round(ms / 60_000)}m`;
  if (ms < 24 * 60 * 60_000) return `${Math.round(ms / (60 * 60_000))}h`;
  return `${Math.round(ms / (24 * 60 * 60_000))}d`;
}

// ─── W43: Dashboard / widget LLM policy editors ─────────────────────────────

const CAPABILITY_OPTIONS: { value: WidgetCapability; label: string; hint: string }[] = [
  {
    value: 'structured_json_object',
    label: 'JSON object',
    hint: 'Provider must honour response_format=json_object (OpenAI / Anthropic / DeepSeek / Kimi).',
  },
  {
    value: 'streaming',
    label: 'Streaming',
    hint: 'Provider must support SSE streaming (OpenRouter / Custom OpenAI; not Ollama today).',
  },
  {
    value: 'tool_calling',
    label: 'Tool calling',
    hint: 'Provider must accept OpenAI-style tools array.',
  },
];

function enabledProviders(providers: LLMProvider[]): LLMProvider[] {
  return providers.filter(
    (p) => p.is_enabled && !p.is_unsupported,
  );
}

function DashboardModelPolicyControl({
  dashboard,
  providers,
  onChange,
}: {
  dashboard: Dashboard;
  providers: LLMProvider[];
  onChange?: (dashboard: Dashboard) => void;
}) {
  const [open, setOpen] = useState(false);
  const policy = dashboard.model_policy ?? null;
  // W46: show the real provider name (with model) rather than a UUID
  // prefix — and cap the visible label width so a long model id can't
  // blow out the toolbar row.
  const providerName = policy
    ? providers.find(p => p.id === policy.provider_id)?.name ?? 'unknown provider'
    : null;
  const summary = policy ? `${providerName} · ${policy.model}` : 'app default';
  const tooltip = policy
    ? `Dashboard default LLM: ${providerName} · ${policy.model}`
    : 'Default LLM for widgets on this dashboard (app active provider)';
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="flex max-w-[14rem] items-center gap-1 rounded-md border border-border bg-muted/30 px-2.5 py-1 text-xs hover:bg-muted hover:border-primary/40 transition-colors mono uppercase tracking-wider"
        title={tooltip}
      >
        <span className="flex-shrink-0">Model ·</span>
        <span className="min-w-0 truncate">{summary}</span>
      </button>
      {open && (
        <PolicyEditorModal
          title="Dashboard default model"
          providers={providers}
          initial={
            policy
              ? {
                  provider_id: policy.provider_id,
                  model: policy.model,
                  required_caps: policy.required_caps ?? [],
                }
              : null
          }
          onClose={() => setOpen(false)}
          onSubmit={async (next) => {
            const updated = await dashboardApi.setModelPolicy(
              dashboard.id,
              next
                ? {
                    provider_id: next.provider_id,
                    model: next.model,
                    required_caps: next.required_caps,
                  }
                : null,
            );
            onChange?.(updated);
            setOpen(false);
          }}
          hint="Widgets that touch an LLM use this provider/model unless they have their own override."
        />
      )}
    </>
  );
}

/** W47: dashboard-level assistant language override. Renders next to
 *  the model policy control and uses the shared [`AssistantLanguagePicker`]
 *  so the catalog is identical across app / dashboard / session
 *  scopes. */
function DashboardLanguagePolicyControl({
  dashboard,
  onChange,
}: {
  dashboard: Dashboard;
  onChange?: (dashboard: Dashboard) => void;
}) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const policy = dashboard.language_policy ?? null;
  const summary = (() => {
    if (!policy) return 'app default';
    if (policy.mode === 'auto') return 'auto';
    return policy.tag;
  })();
  const tooltip = policy
    ? `Dashboard assistant language: ${summary}`
    : 'Assistant language for this dashboard (app default)';
  return (
    <>
      <button
        type="button"
        onClick={() => {
          setError(null);
          setOpen(true);
        }}
        className="flex max-w-[14rem] items-center gap-1 rounded-md border border-border bg-muted/30 px-2.5 py-1 text-xs hover:bg-muted hover:border-primary/40 transition-colors mono uppercase tracking-wider"
        title={tooltip}
      >
        <span className="flex-shrink-0">Lang ·</span>
        <span className="min-w-0 truncate">{summary}</span>
      </button>
      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4 backdrop-blur-sm"
          onClick={() => setOpen(false)}
        >
          <div
            className="w-full max-w-md space-y-3 rounded-md border border-border bg-card p-4 shadow-2xl"
            onClick={event => event.stopPropagation()}
          >
            <div>
              <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// language</p>
              <h2 className="mt-0.5 text-base font-semibold tracking-tight">Dashboard language override</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                Applies to chat sessions scoped to this dashboard and to LLM-backed
                widget pipelines. Choose "Inherit" to fall back to the app default.
              </p>
            </div>
            <AssistantLanguagePicker
              value={policy}
              allowInherit
              label="Language"
              onChange={async next => {
                try {
                  const updated = await dashboardApi.setLanguagePolicy(dashboard.id, next);
                  onChange?.(updated);
                  setError(null);
                  setOpen(false);
                } catch (err) {
                  setError(err instanceof Error ? err.message : String(err));
                }
              }}
            />
            {error && <p className="text-[11px] text-destructive">{error}</p>}
            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => setOpen(false)}
                className="rounded-md border border-border px-2.5 py-1.5 text-xs hover:bg-muted"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

/** W50: dashboard-level pause/resume toggle. Aggregates the schedule
 *  state of every distinct workflow referenced by widgets on this
 *  dashboard, surfaces the worst-case label, and lets the operator
 *  pause/resume all of them in one click. The detailed per-workflow
 *  editor lives in the Operations cockpit. */
function DashboardScheduleControl({ dashboard }: { dashboard: Dashboard }) {
  const [summaries, setSummaries] = useState<WorkflowSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<'pause' | 'resume' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const widgetWorkflowIds = useMemo(() => {
    const ids = new Set<string>();
    for (const widget of dashboard.layout) {
      const id = widget.datasource?.workflow_id;
      if (id) ids.add(id);
    }
    return ids;
  }, [dashboard.layout]);

  const reload = useCallback(async () => {
    setError(null);
    try {
      const all = await operationsApi.listWorkflowSummaries();
      setSummaries(all.filter(s => widgetWorkflowIds.has(s.id)));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [widgetWorkflowIds]);

  useEffect(() => {
    reload();
  }, [reload]);

  // Aggregate label for the trigger button. We pick the most
  // "actionable" state so the user immediately knows what they would be
  // changing.
  const aggregate: ScheduleDisplayState | 'none' = useMemo(() => {
    if (summaries.length === 0) return 'none';
    const order: ScheduleDisplayState[] = [
      'invalid',
      'paused_by_user',
      'manual_only',
      'disabled',
      'not_scheduled',
      'active',
    ];
    for (const candidate of order) {
      if (summaries.some(s => s.schedule.display_state === candidate)) {
        return candidate;
      }
    }
    return summaries[0]?.schedule.display_state ?? 'none';
  }, [summaries]);

  const anyPaused = summaries.some(s => s.schedule.pause_state === 'paused');
  const anyActive = summaries.some(
    s => s.schedule.pause_state === 'active' && s.schedule.trigger_kind === 'cron',
  );

  async function handlePauseAll() {
    setBusy('pause');
    setError(null);
    try {
      const next = await scheduleApi.pauseDashboard(dashboard.id);
      const byId = new Map(next.map(s => [s.id, s]));
      setSummaries(prev => prev.map(s => byId.get(s.id) ?? s));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleResumeAll() {
    setBusy('resume');
    setError(null);
    try {
      const next = await scheduleApi.resumeDashboard(dashboard.id);
      const byId = new Map(next.map(s => [s.id, s]));
      setSummaries(prev => prev.map(s => byId.get(s.id) ?? s));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  function handleSummaryChange(updated: WorkflowSummary) {
    setSummaries(prev => prev.map(s => (s.id === updated.id ? updated : s)));
  }

  if (summaries.length === 0 && !loading) {
    return null;
  }

  const buttonLabel = (() => {
    if (loading) return 'Schedules · …';
    if (aggregate === 'none') return 'Schedules · —';
    if (anyPaused && !anyActive) return 'Schedules · Paused';
    if (anyPaused) return 'Schedules · Mixed';
    return 'Schedules · Active';
  })();
  const buttonTone = anyPaused
    ? 'border-amber-500/40 bg-amber-500/10 text-amber-300 hover:bg-amber-500/20'
    : 'border-border bg-muted/30 hover:bg-muted hover:border-primary/40';

  return (
    <>
      <button
        type="button"
        onClick={() => {
          setOpen(true);
          reload();
        }}
        className={`flex items-center gap-1 rounded-md border px-2.5 py-1 text-xs mono uppercase tracking-wider transition-colors ${buttonTone}`}
        title="Pause / resume / change cadence for this dashboard's widgets"
      >
        {buttonLabel}
      </button>
      {open && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4 backdrop-blur-sm"
          onClick={() => setOpen(false)}
        >
          <div
            className="w-full max-w-2xl max-h-[80vh] overflow-auto space-y-3 rounded-md border border-border bg-card p-4 shadow-2xl"
            onClick={e => e.stopPropagation()}
          >
            <div>
              <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// schedules</p>
              <h2 className="mt-0.5 text-base font-semibold tracking-tight">Refresh schedule</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                Pause stops automatic refresh for every workflow this dashboard
                consumes. Manual refresh still works on each widget. Paused
                schedules survive app restart.
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={handlePauseAll}
                disabled={busy !== null || !anyActive}
                className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs mono uppercase tracking-wider text-amber-300 hover:bg-amber-500/20 transition-colors disabled:opacity-50"
              >
                {busy === 'pause' ? 'Pausing…' : 'Pause all'}
              </button>
              <button
                type="button"
                onClick={handleResumeAll}
                disabled={busy !== null || !anyPaused}
                className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-3 py-1.5 text-xs mono uppercase tracking-wider text-emerald-300 hover:bg-emerald-500/20 transition-colors disabled:opacity-50"
              >
                {busy === 'resume' ? 'Resuming…' : 'Resume all'}
              </button>
              <button
                type="button"
                onClick={reload}
                disabled={busy !== null}
                className="rounded-md border border-border bg-card px-3 py-1.5 text-xs mono uppercase tracking-wider hover:bg-muted transition-colors"
              >
                Refresh state
              </button>
            </div>
            {error && <p className="text-xs text-destructive">{error}</p>}
            {loading ? (
              <p className="text-xs text-muted-foreground">Loading…</p>
            ) : summaries.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                No workflows are bound to this dashboard's widgets yet.
              </p>
            ) : (
              <ul className="space-y-3">
                {summaries.map(summary => (
                  <li key={summary.id} className="rounded-md border border-border bg-muted/20 p-3">
                    <div className="mb-2 flex items-start justify-between gap-2">
                      <div>
                        <p className="text-sm font-medium">{summary.name}</p>
                        {summary.description && (
                          <p className="text-[11px] text-muted-foreground">{summary.description}</p>
                        )}
                      </div>
                    </div>
                    <ScheduleEditor summary={summary} onChange={handleSummaryChange} />
                  </li>
                ))}
              </ul>
            )}
            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => setOpen(false)}
                className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

function WidgetModelOverrideEditor({
  dashboardId,
  widget,
  providers,
  dashboardPolicy,
  onChange,
}: {
  dashboardId: string;
  widget: Widget;
  providers: LLMProvider[];
  dashboardPolicy: DashboardModelPolicy | null;
  onChange?: (dashboard: Dashboard) => void;
}) {
  const override = widget.datasource?.model_override ?? null;
  const [open, setOpen] = useState(false);
  const fallbackSource = override
    ? 'widget override'
    : dashboardPolicy
      ? 'dashboard default'
      : 'app active provider';
  return (
    <div className="rounded-md border border-border/60 bg-muted/20 p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <p className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// widget model</p>
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">
          inherits: {fallbackSource}
        </span>
      </div>
      <p className="text-[11px] text-foreground">
        {override ? (
          <>
            Override <span className="mono text-foreground/80">{override.model}</span> on provider
            <span className="ml-1 mono text-foreground/80">{override.provider_id}</span>.
            {override.required_caps && override.required_caps.length > 0 && (
              <span className="ml-2 text-muted-foreground">caps: {override.required_caps.join(', ')}</span>
            )}
          </>
        ) : (
          <span className="text-muted-foreground italic">
            No override. The widget uses the inherited model above.
          </span>
        )}
      </p>
      <div className="mt-2 flex gap-2">
        <button
          type="button"
          onClick={() => setOpen(true)}
          className="rounded-sm border border-border bg-muted/40 px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-foreground hover:border-primary/40 hover:text-primary transition-colors"
        >
          {override ? 'Edit override…' : 'Set override…'}
        </button>
        {override && (
          <button
            type="button"
            onClick={async () => {
              const updated = await dashboardApi.setWidgetModelOverride(
                dashboardId,
                widget.id,
                null,
              );
              onChange?.(updated);
            }}
            className="rounded-sm border border-border bg-muted/40 px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-muted-foreground hover:border-destructive/40 hover:text-destructive transition-colors"
          >
            Clear
          </button>
        )}
      </div>
      {open && (
        <PolicyEditorModal
          title={`Override model for "${widget.title}"`}
          providers={providers}
          initial={
            override
              ? {
                  provider_id: override.provider_id,
                  model: override.model,
                  required_caps: override.required_caps ?? [],
                }
              : null
          }
          onClose={() => setOpen(false)}
          onSubmit={async (next) => {
            const payload: WidgetModelOverride | null = next
              ? {
                  provider_id: next.provider_id,
                  model: next.model,
                  required_caps: next.required_caps,
                }
              : null;
            const updated = await dashboardApi.setWidgetModelOverride(
              dashboardId,
              widget.id,
              payload,
            );
            onChange?.(updated);
            setOpen(false);
          }}
          hint="This override applies to this widget only and beats the dashboard default."
        />
      )}
    </div>
  );
}

function ModelSourceBadge({ source }: { source: 'widget_override' | 'dashboard_default' | 'app_active_provider' }) {
  const map: Record<typeof source, { label: string; tone: string }> = {
    widget_override: {
      label: 'widget',
      tone: 'border-primary/40 bg-primary/15 text-primary',
    },
    dashboard_default: {
      label: 'dashboard',
      tone: 'border-border bg-muted/40 text-foreground',
    },
    app_active_provider: {
      label: 'app default',
      tone: 'border-border bg-muted/40 text-muted-foreground',
    },
  };
  const entry = map[source];
  return (
    <span
      title={`Model source: ${source.replace(/_/g, ' ')}`}
      className={`ml-2 rounded-sm border px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider ${entry.tone}`}
    >
      {entry.label}
    </span>
  );
}

interface PolicyDraft {
  provider_id: string;
  model: string;
  required_caps: WidgetCapability[];
}

function PolicyEditorModal({
  title,
  providers,
  initial,
  hint,
  onClose,
  onSubmit,
}: {
  title: string;
  providers: LLMProvider[];
  initial: PolicyDraft | null;
  hint?: string;
  onClose: () => void;
  onSubmit: (next: PolicyDraft | null) => Promise<void>;
}) {
  const eligible = enabledProviders(providers);
  const [providerId, setProviderId] = useState<string>(
    initial?.provider_id ?? eligible[0]?.id ?? '',
  );
  const [model, setModel] = useState<string>(
    initial?.model
      ?? eligible.find((p) => p.id === (initial?.provider_id ?? eligible[0]?.id))?.default_model
      ?? '',
  );
  const [caps, setCaps] = useState<WidgetCapability[]>(initial?.required_caps ?? []);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const provider = eligible.find((p) => p.id === providerId);
  const knownModels = provider?.models ?? [];
  const handleProviderChange = (id: string) => {
    setProviderId(id);
    const next = eligible.find((p) => p.id === id);
    if (next) setModel(next.default_model);
  };
  const toggleCap = (cap: WidgetCapability) => {
    setCaps((prev) => (prev.includes(cap) ? prev.filter((c) => c !== cap) : [...prev, cap]));
  };
  const handleSubmit = async (next: PolicyDraft | null) => {
    setError(null);
    setSaving(true);
    try {
      await onSubmit(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-background/80 backdrop-blur-sm">
      <div className="flex w-[min(90vw,30rem)] flex-col rounded-md border border-border bg-card shadow-2xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3 bg-muted/30">
          <p className="text-sm font-semibold truncate">{title}</p>
          <button onClick={onClose} className="p-1 rounded hover:bg-muted hover:text-foreground transition-colors">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="p-4 space-y-3 text-xs">
          {hint && <p className="text-muted-foreground">{hint}</p>}
          {eligible.length === 0 && (
            <p className="text-destructive">
              No enabled providers. Add one in Provider Settings before pinning a model.
            </p>
          )}
          <label className="block">
            <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">Provider</span>
            <select
              value={providerId}
              onChange={(e) => handleProviderChange(e.target.value)}
              className="mt-1 w-full rounded border border-border bg-background px-2 py-1 text-xs"
              disabled={eligible.length === 0}
            >
              {eligible.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">Model</span>
            <input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              list={knownModels.length > 0 ? `models-${providerId}` : undefined}
              className="mt-1 w-full rounded border border-border bg-background px-2 py-1 text-xs mono"
              placeholder={provider?.default_model ?? 'model id'}
            />
            {knownModels.length > 0 && (
              <datalist id={`models-${providerId}`}>
                {knownModels.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            )}
          </label>
          <div>
            <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">Required capabilities</span>
            <div className="mt-1 space-y-1">
              {CAPABILITY_OPTIONS.map((opt) => (
                <label key={opt.value} className="flex items-start gap-2 text-[11px]">
                  <input
                    type="checkbox"
                    checked={caps.includes(opt.value)}
                    onChange={() => toggleCap(opt.value)}
                    className="mt-0.5"
                  />
                  <span>
                    <span className="mono">{opt.label}</span>
                    <span className="ml-2 text-muted-foreground">{opt.hint}</span>
                  </span>
                </label>
              ))}
            </div>
          </div>
          {error && (
            <p className="text-destructive text-[11px] mono break-words">{error}</p>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3 bg-muted/20">
          {initial && (
            <button
              type="button"
              onClick={() => handleSubmit(null)}
              disabled={saving}
              className="rounded-md border border-border px-3 py-1 text-xs hover:bg-destructive/15 hover:border-destructive/40 hover:text-destructive transition-colors disabled:opacity-50"
            >
              Clear policy
            </button>
          )}
          <button
            type="button"
            onClick={onClose}
            disabled={saving}
            className="rounded-md border border-border px-3 py-1 text-xs hover:bg-muted/40 transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => {
              if (!providerId || !model.trim()) {
                setError('Provider and model are required.');
                return;
              }
              handleSubmit({ provider_id: providerId, model: model.trim(), required_caps: caps });
            }}
            disabled={saving || eligible.length === 0}
            className="rounded-md border border-primary/40 bg-primary/15 px-3 py-1 text-xs text-primary hover:bg-primary/25 transition-colors disabled:opacity-50"
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </div>
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
    ? 'bg-neon-lime/15 text-neon-lime border-neon-lime/30'
    : status === 'error'
      ? 'bg-destructive/15 text-destructive border-destructive/40'
      : status === 'running'
        ? 'bg-primary/15 text-primary border-primary/30'
        : status === 'fallback'
          ? 'bg-neon-amber/15 text-neon-amber border-neon-amber/30'
          : 'bg-muted text-muted-foreground border-border';
  return (
    <span title={title} className={`rounded-sm border px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider ${tone}`}>
      {status}
    </span>
  );
}

function WidgetMenu({
  onRename,
  onDuplicate,
  onDelete,
  onInspect,
  onOpenAlerts,
  onDebugPipeline,
}: {
  onRename: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onInspect: () => void;
  onOpenAlerts?: () => void;
  onDebugPipeline?: () => void;
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
        <div className="absolute right-0 top-7 z-40 w-44 rounded-md border border-border bg-popover py-1 shadow-xl">
          <MenuItem label="Rename" onClick={() => { setOpen(false); onRename(); }} />
          <MenuItem label="Duplicate" onClick={() => { setOpen(false); onDuplicate(); }} />
          <MenuItem label="View raw data" onClick={() => { setOpen(false); onInspect(); }} />
          {onDebugPipeline && (
            <MenuItem label="Debug pipeline…" onClick={() => { setOpen(false); onDebugPipeline(); }} />
          )}
          {onOpenAlerts && (
            <MenuItem label="Alerts…" onClick={() => { setOpen(false); onOpenAlerts(); }} />
          )}
          <div className="my-1 h-px bg-border" />
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
    <div className="flex items-center justify-between gap-2 border-t border-border/60 bg-muted/20 px-3 py-1 text-[10px] mono uppercase tracking-wider">
      <span className={errored ? 'text-destructive' : 'text-muted-foreground'}>
        {errored ? `failed ${last}` : `updated ${last}`}
      </span>
      {durationMs !== undefined && durationMs > 0 && (
        <span className="tabular text-muted-foreground/70">{Math.round(durationMs)}ms</span>
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

function WidgetRenderer({ widget, data, streamState }: { widget: Dashboard['layout'][number]; data?: WidgetRuntimeData; streamState?: WidgetStreamState }) {
  switch (widget.type) {
    case 'chart': return <ChartWidget config={widget.config} data={data?.kind === 'chart' ? data : undefined} />;
    case 'text': return <TextWidget config={widget.config} data={data?.kind === 'text' ? data : undefined} streamState={streamState} />;
    case 'table': return <TableWidget config={widget.config} data={data?.kind === 'table' ? data : undefined} />;
    case 'gauge': return <GaugeWidget config={widget.config} data={data?.kind === 'gauge' ? data : undefined} />;
    case 'image': return <ImageWidget config={widget.config} data={data?.kind === 'image' ? data : undefined} />;
    case 'stat': return <StatWidget config={widget.config} data={data?.kind === 'stat' ? data : undefined} />;
    case 'logs': return <LogsWidget config={widget.config} data={data?.kind === 'logs' ? data : undefined} />;
    case 'bar_gauge': return <BarGaugeWidget config={widget.config} data={data?.kind === 'bar_gauge' ? data : undefined} />;
    case 'status_grid': return <StatusGridWidget config={widget.config} data={data?.kind === 'status_grid' ? data : undefined} />;
    case 'heatmap': return <HeatmapWidget config={widget.config} data={data?.kind === 'heatmap' ? data : undefined} />;
    case 'gallery': return <GalleryWidget config={widget.config} data={data?.kind === 'gallery' ? data : undefined} />;
    default: return <div className="text-muted-foreground text-sm">Unknown widget type</div>;
  }
}

// W40: per-widget shell rendered inside the responsive grid. Wrapped in
// `memo` so a single widget's refresh / cached-at tick does not force
// every sibling to rerender. Parent callbacks all accept `widgetId`
// instead of being widget-bound closures, which keeps their identity
// stable across grid renders and lets the shallow `memo` compare work
// as expected.
interface WidgetCellProps {
  widget: Widget;
  data?: WidgetRuntimeData;
  error?: string;
  run?: WorkflowRun;
  refreshing: boolean;
  cachedAt?: number;
  /** W42: live streaming state for this widget refresh. */
  streamState?: WidgetStreamState;
  alertStatus?: WidgetAlertStatus;
  isEditingTitle: boolean;
  onRefresh: (widgetId: string) => void;
  onStartRename: (widgetId: string) => void;
  onStopRename: () => void;
  onRenameSave: (widgetId: string, nextTitle: string) => void;
  onDuplicate: (widgetId: string) => void;
  onDelete: (widgetId: string) => void;
  onInspect: (widgetId: string) => void;
  onOpenAlerts?: (widgetId: string) => void;
  onDebug: (widgetId: string) => void;
}

const WidgetCell = memo(function WidgetCell(props: WidgetCellProps) {
  const {
    widget,
    data,
    error,
    run,
    refreshing,
    cachedAt,
    streamState,
    alertStatus,
    isEditingTitle,
    onRefresh,
    onStartRename,
    onStopRename,
    onRenameSave,
    onDuplicate,
    onDelete,
    onInspect,
    onOpenAlerts,
    onDebug,
  } = props;
  const fallback = isFallback(data);
  return (
    <>
      <div className="widget-drag-handle flex items-center justify-between px-3 py-1.5 border-b border-border/60 cursor-move bg-muted/30">
        {isEditingTitle ? (
          <TitleEditor
            initial={widget.title}
            onSave={value => { onRenameSave(widget.id, value); onStopRename(); }}
            onCancel={onStopRename}
          />
        ) : (
          <span
            className="text-xs font-semibold truncate tracking-tight"
            onDoubleClick={event => { event.stopPropagation(); onStartRename(widget.id); }}
            title="Double-click to rename"
          >
            {widget.title}
          </span>
        )}
        <div className="flex items-center gap-1">
          <AlertDot status={alertStatus} />
          {streamState && <StreamBadge state={streamState} />}
          {cachedAt !== undefined && <CachedBadge capturedAt={cachedAt} />}
          <WorkflowBadge widget={widget} run={run} fallback={fallback} />
          <button
            className="p-1 rounded hover:bg-muted hover:text-primary disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            title="Refresh widget data"
            disabled={refreshing}
            onMouseDown={event => event.stopPropagation()}
            onClick={() => onRefresh(widget.id)}
          >
            <svg className={`w-3.5 h-3.5 text-muted-foreground ${refreshing ? 'animate-spin text-primary' : ''}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
          </button>
          <WidgetMenu
            onRename={() => onStartRename(widget.id)}
            onDuplicate={() => onDuplicate(widget.id)}
            onDelete={() => onDelete(widget.id)}
            onInspect={() => onInspect(widget.id)}
            onOpenAlerts={onOpenAlerts ? () => onOpenAlerts(widget.id) : undefined}
            onDebugPipeline={widget.datasource ? () => onDebug(widget.id) : undefined}
          />
        </div>
      </div>
      <div className="p-3 flex-1 overflow-auto min-h-0">
        {error ? (
          <WidgetError message={error} />
        ) : (
          <WidgetRenderer widget={widget} data={data} streamState={streamState} />
        )}
      </div>
      <WidgetFooter widget={widget} run={run} />
    </>
  );
});

// W42: compact chrome badge for active streaming/reasoning state. Only
// renders while a `streamState` is present — final committed runtime
// data clears the state and removes the badge.
function StreamBadge({ state }: { state: WidgetStreamState }) {
  const map: Record<WidgetStreamState['status'], { label: string; tone: string; title: string; spin: boolean }> = {
    starting: {
      label: 'starting',
      tone: 'border-primary/40 bg-primary/10 text-primary',
      title: 'Widget refresh started',
      spin: true,
    },
    reasoning: {
      label: 'reasoning',
      tone: 'border-primary/40 bg-primary/15 text-primary',
      title: 'LLM is reasoning…',
      spin: true,
    },
    streaming: {
      label: 'streaming',
      tone: 'border-primary/40 bg-primary/15 text-primary',
      title: 'Streaming partial text from the provider',
      spin: false,
    },
    waiting: {
      label: 'waiting',
      tone: 'border-neon-amber/40 bg-neon-amber/15 text-neon-amber',
      title: state.statusHint ?? 'Waiting for provider response',
      spin: true,
    },
    failed: {
      label: 'failed',
      tone: 'border-destructive/40 bg-destructive/15 text-destructive',
      title: state.error ?? 'Refresh failed',
      spin: false,
    },
  };
  const entry = map[state.status];
  return (
    <span
      title={entry.title}
      className={`inline-flex items-center gap-1 rounded-sm border px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider ${entry.tone}`}
    >
      {entry.spin && (
        <span className="inline-block h-1.5 w-1.5 rounded-full bg-current animate-pulse" />
      )}
      {entry.label}
    </span>
  );
}

function WidgetError({ message }: { message?: string }) {
  return (
    <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-center text-xs text-destructive">
      <span className="mono uppercase tracking-wider text-[10px] text-destructive/70">// error</span>
      <span>{message}</span>
    </div>
  );
}
