// W35: Workflow Operations Cockpit. Read-only operator surface over the
// existing workflow runtime — no parallel queue. Lists workflow
// summaries, recent runs, schedule health, and supports retry / honest
// "cancel unsupported" feedback. Re-execution flows through the same
// `execute_workflow` Tauri command the dashboard uses.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import type {
  RunStatusValue,
  SchedulerHealth,
  WorkflowEventEnvelope,
  WorkflowOwnerRef,
  WorkflowRunCancelOutcome,
  WorkflowRunDetail,
  WorkflowRunSummary,
  WorkflowSummary,
  WorkflowRunFilter,
} from '../../lib/api';
import { operationsApi } from '../../lib/api';
import { ScheduleEditor, ScheduleStateBadge } from '../schedule/ScheduleEditor';

const WORKFLOW_EVENT_CHANNEL = 'workflow:event';

interface Props {
  onClose: () => void;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
  onJumpToDatasource?: (datasourceDefinitionId: string) => void;
  initialFilter?: WorkflowRunFilter;
  /** W35: preselect a run id (e.g. when arriving from AlertsView).
   * The view will load and focus that run's detail pane on mount. */
  initialRunId?: string;
}

const STATUS_TONE: Record<RunStatusValue, string> = {
  idle: 'bg-muted text-muted-foreground border-border',
  running: 'bg-primary/15 text-primary border-primary/40',
  success: 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40',
  error: 'bg-destructive/15 text-destructive border-destructive/40',
  skipped: 'bg-muted text-muted-foreground border-border',
};

function formatDuration(ms?: number): string {
  if (ms === undefined || ms === null) return '—';
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

function formatTimestamp(ms: number): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleString();
}

function summarizeOwner(owner: WorkflowOwnerRef): string {
  const parts: string[] = [];
  if (owner.datasource_name) parts.push(`ds: ${owner.datasource_name}`);
  const widgetCount = owner.dashboards.reduce((sum, d) => sum + d.widgets.length, 0);
  if (widgetCount > 0) {
    parts.push(`${widgetCount} widget${widgetCount === 1 ? '' : 's'}`);
  }
  if (!parts.length) return 'standalone';
  return parts.join(' · ');
}

export function OperationsView({ onClose, onJumpToWidget, onJumpToDatasource, initialFilter, initialRunId }: Props) {
  const [summaries, setSummaries] = useState<WorkflowSummary[]>([]);
  const [scheduler, setScheduler] = useState<SchedulerHealth | null>(null);
  const [runs, setRuns] = useState<WorkflowRunSummary[]>([]);
  const [runDetail, setRunDetail] = useState<WorkflowRunDetail | null>(null);
  const [selectedWorkflowId, setSelectedWorkflowId] = useState<string | null>(
    initialFilter?.workflow_id ?? null,
  );
  const [selectedRunId, setSelectedRunId] = useState<string | null>(initialRunId ?? null);
  const [statusFilter, setStatusFilter] = useState<RunStatusValue | 'all'>(
    initialFilter?.status ?? 'all',
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [cancelOutcome, setCancelOutcome] = useState<WorkflowRunCancelOutcome | null>(null);
  const [loading, setLoading] = useState(true);
  // Used to debounce live-event-driven refreshes so a burst of node
  // events doesn't fire one `listRuns` call per node finished.
  const pendingRefreshRef = useRef<number | null>(null);

  const loadAll = useCallback(async () => {
    try {
      setError(null);
      const [s, h] = await Promise.all([
        operationsApi.listWorkflowSummaries(),
        operationsApi.schedulerHealth(),
      ]);
      setSummaries(s);
      setScheduler(h);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load workflows');
    } finally {
      setLoading(false);
    }
  }, []);

  const loadRuns = useCallback(async () => {
    try {
      const filter: WorkflowRunFilter = {
        ...(selectedWorkflowId ? { workflow_id: selectedWorkflowId } : {}),
        ...(statusFilter !== 'all' ? { status: statusFilter } : {}),
        limit: 100,
      };
      const data = await operationsApi.listRuns(filter);
      setRuns(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load runs');
    }
  }, [selectedWorkflowId, statusFilter]);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  useEffect(() => {
    loadRuns();
  }, [loadRuns]);

  useEffect(() => {
    const unsubscribe = listen<WorkflowEventEnvelope>(WORKFLOW_EVENT_CHANNEL, () => {
      if (pendingRefreshRef.current !== null) return;
      pendingRefreshRef.current = window.setTimeout(() => {
        pendingRefreshRef.current = null;
        loadAll();
        loadRuns();
      }, 500);
    });
    return () => {
      unsubscribe.then(dispose => dispose()).catch(() => {});
      if (pendingRefreshRef.current !== null) {
        window.clearTimeout(pendingRefreshRef.current);
        pendingRefreshRef.current = null;
      }
    };
  }, [loadAll, loadRuns]);

  useEffect(() => {
    if (!selectedRunId) {
      setRunDetail(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const detail = await operationsApi.getRunDetail(selectedRunId);
        if (!cancelled) setRunDetail(detail);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to load run detail');
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedRunId]);

  const totalWarnings = scheduler?.warnings.length ?? 0;

  const visibleRuns = useMemo(() => {
    if (statusFilter === 'all') return runs;
    return runs.filter(r => r.status === statusFilter);
  }, [runs, statusFilter]);

  const handleRetry = useCallback(async (runId: string) => {
    setBusy(runId);
    setError(null);
    try {
      const newRun = await operationsApi.retryRun(runId);
      setSelectedRunId(newRun.id);
      await loadAll();
      await loadRuns();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Retry failed');
    } finally {
      setBusy(null);
    }
  }, [loadAll, loadRuns]);

  const handleCancel = useCallback(async (runId: string) => {
    setBusy(runId);
    setError(null);
    try {
      const outcome = await operationsApi.cancelRun(runId);
      setCancelOutcome(outcome);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Cancel failed');
    } finally {
      setBusy(null);
    }
  }, []);

  return (
    <div className="flex h-full flex-col bg-background">
      <div className="flex items-center justify-between border-b border-border px-4 py-3 bg-muted/20">
        <div>
          <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// operations</p>
          <h2 className="mt-0.5 text-sm font-semibold tracking-tight">Workflow Operations</h2>
          <p className="text-xs text-muted-foreground">
            {summaries.length} workflow{summaries.length === 1 ? '' : 's'} · {scheduler?.scheduled_workflow_ids.length ?? 0} scheduled · {totalWarnings} warning{totalWarnings === 1 ? '' : 's'}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => { loadAll(); loadRuns(); }}
            className="rounded-md border border-border bg-card px-3 py-1.5 text-xs mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
          >
            Refresh
          </button>
          <button
            onClick={onClose}
            className="rounded-md border border-border bg-card px-3 py-1.5 text-xs mono uppercase tracking-wider hover:bg-muted transition-colors"
          >
            Close
          </button>
        </div>
      </div>

      {error && (
        <div className="border-b border-destructive/40 bg-destructive/5 px-4 py-2 text-xs text-destructive">
          {error}
        </div>
      )}
      {cancelOutcome && (
        <div className="border-b border-amber-500/40 bg-amber-500/5 px-4 py-2 text-xs text-amber-200">
          {cancelOutcome.cancelled
            ? `Cancelled run ${cancelOutcome.run_id}.`
            : `Cancellation unavailable: ${cancelOutcome.reason}`}
          <button
            onClick={() => setCancelOutcome(null)}
            className="ml-2 text-amber-200 underline-offset-2 hover:underline"
          >
            dismiss
          </button>
        </div>
      )}

      <div className="grid flex-1 min-h-0 grid-cols-12 gap-0 overflow-hidden">
        {/* Workflow list */}
        <aside className="col-span-3 border-r border-border overflow-auto scrollbar-thin">
          <div className="px-3 py-2 sticky top-0 bg-muted/40 backdrop-blur border-b border-border">
            <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground">// workflows</p>
          </div>
          {loading ? (
            <p className="px-3 py-3 text-xs text-muted-foreground">Loading…</p>
          ) : summaries.length === 0 ? (
            <p className="px-3 py-3 text-xs text-muted-foreground">No workflows persisted yet.</p>
          ) : (
            <ul>
              <li>
                <button
                  onClick={() => setSelectedWorkflowId(null)}
                  className={`w-full text-left px-3 py-2 text-xs border-l-2 transition-colors ${
                    selectedWorkflowId === null
                      ? 'route-active border-l-primary'
                      : 'border-l-transparent hover:bg-muted/40'
                  }`}
                >
                  All workflows
                </button>
              </li>
              {summaries.map(s => {
                const last = s.last_run;
                const statusTone = last ? STATUS_TONE[last.status] : STATUS_TONE.idle;
                return (
                  <li key={s.id}>
                    <button
                      onClick={() => setSelectedWorkflowId(s.id)}
                      className={`w-full text-left px-3 py-2 text-xs border-l-2 transition-colors ${
                        selectedWorkflowId === s.id
                          ? 'route-active border-l-primary'
                          : 'border-l-transparent hover:bg-muted/40'
                      }`}
                    >
                      <div className="flex items-center gap-2">
                        <span
                          className={`inline-block h-2 w-2 rounded-full ${last ? statusTone : 'bg-muted'}`}
                          aria-hidden
                        />
                        <span className="flex-1 truncate font-medium">{s.name}</span>
                        {s.schedule.pause_state === 'paused' && (
                          <span className="rounded-sm border border-amber-500/40 bg-amber-500/10 px-1 py-0.5 text-[9px] mono uppercase tracking-wider text-amber-300">
                            paused
                          </span>
                        )}
                        {s.schedule.is_scheduled && (
                          <span className="rounded-sm border border-primary/30 bg-primary/10 px-1 py-0.5 text-[9px] mono uppercase tracking-wider text-primary">
                            cron
                          </span>
                        )}
                        {!s.is_enabled && (
                          <span className="rounded-sm border border-border bg-muted px-1 py-0.5 text-[9px] mono uppercase tracking-wider text-muted-foreground">
                            off
                          </span>
                        )}
                      </div>
                      <p className="mt-1 truncate text-[11px] text-muted-foreground">
                        {summarizeOwner(s.owner)}
                      </p>
                      {last && (
                        <p className="mt-0.5 truncate text-[10px] text-muted-foreground">
                          last: {formatTimestamp(last.started_at)} · {formatDuration(last.duration_ms ?? undefined)}
                        </p>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        {/* Run list */}
        <section className="col-span-5 border-r border-border overflow-auto scrollbar-thin">
          <div className="sticky top-0 bg-muted/40 backdrop-blur border-b border-border px-3 py-2 flex items-center justify-between gap-3">
            <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground">// runs</p>
            <div className="flex items-center gap-1">
              {(['all', 'running', 'success', 'error'] as const).map(opt => (
                <button
                  key={opt}
                  onClick={() => setStatusFilter(opt)}
                  className={`rounded-sm border px-2 py-0.5 text-[10px] mono uppercase tracking-wider transition-colors ${
                    statusFilter === opt
                      ? 'border-primary/60 bg-primary/20 text-primary'
                      : 'border-border bg-card text-muted-foreground hover:border-primary/40'
                  }`}
                >
                  {opt}
                </button>
              ))}
            </div>
          </div>
          {visibleRuns.length === 0 ? (
            <p className="px-3 py-3 text-xs text-muted-foreground">No runs match this filter.</p>
          ) : (
            <ul className="divide-y divide-border/60">
              {visibleRuns.map(run => (
                <li
                  key={run.id}
                  className={`px-3 py-2 cursor-pointer transition-colors ${
                    selectedRunId === run.id ? 'bg-muted/40' : 'hover:bg-muted/20'
                  }`}
                  onClick={() => setSelectedRunId(run.id)}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className={`rounded-sm border px-1.5 py-0.5 mono text-[10px] font-semibold uppercase tracking-wider ${STATUS_TONE[run.status]}`}
                    >
                      {run.status}
                    </span>
                    <span className="flex-1 truncate text-xs text-muted-foreground">
                      {formatTimestamp(run.started_at)}
                    </span>
                    <span className="text-[10px] mono text-muted-foreground">
                      {formatDuration(run.duration_ms ?? undefined)}
                    </span>
                  </div>
                  {run.error && (
                    <p className="mt-1 truncate text-[11px] text-destructive">{run.error}</p>
                  )}
                  <div className="mt-1 flex items-center gap-1">
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleRetry(run.id);
                      }}
                      disabled={busy === run.id}
                      className="rounded-sm border border-border bg-card px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider hover:border-primary/40 transition-colors disabled:opacity-50"
                    >
                      Retry
                    </button>
                    {run.status === 'running' && (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          handleCancel(run.id);
                        }}
                        disabled={busy === run.id}
                        className="rounded-sm border border-amber-500/40 bg-card px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider text-amber-200 hover:bg-amber-500/10 transition-colors disabled:opacity-50"
                      >
                        Cancel
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        {/* Detail pane */}
        <section className="col-span-4 overflow-auto scrollbar-thin">
          <div className="sticky top-0 bg-muted/40 backdrop-blur border-b border-border px-3 py-2">
            <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground">// detail</p>
          </div>
          <div className="p-3 space-y-3 text-xs">
            {scheduler && scheduler.warnings.length > 0 && (
              <div className="rounded-md border border-amber-500/40 bg-amber-500/5 p-2">
                <p className="mono text-[10px] uppercase tracking-[0.16em] text-amber-200 mb-1">
                  // scheduler warnings
                </p>
                <ul className="space-y-1">
                  {scheduler.warnings.map((w, i) => (
                    <li key={`${w.workflow_id}-${i}`} className="text-[11px] text-amber-200">
                      <span className="mono text-[10px] uppercase tracking-wider">{w.kind}</span>{' '}
                      <span className="text-foreground">{w.workflow_name}</span>: {w.message}
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {runDetail ? (
              <RunDetailPane
                detail={runDetail}
                onJumpToWidget={onJumpToWidget}
                onJumpToDatasource={onJumpToDatasource}
                onRetry={() => handleRetry(runDetail.run.id)}
                onCancel={() => handleCancel(runDetail.run.id)}
                busy={busy === runDetail.run.id}
              />
            ) : selectedWorkflowId ? (
              <WorkflowSummaryPane
                summary={summaries.find(s => s.id === selectedWorkflowId) ?? null}
                onJumpToWidget={onJumpToWidget}
                onJumpToDatasource={onJumpToDatasource}
                onSummaryChange={next =>
                  setSummaries(prev => prev.map(s => (s.id === next.id ? next : s)))
                }
              />
            ) : (
              <p className="text-muted-foreground">Select a workflow on the left or a run in the middle pane.</p>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

function WorkflowSummaryPane({
  summary,
  onJumpToWidget,
  onJumpToDatasource,
  onSummaryChange,
}: {
  summary: WorkflowSummary | null;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
  onJumpToDatasource?: (datasourceDefinitionId: string) => void;
  onSummaryChange?: (next: WorkflowSummary) => void;
}) {
  if (!summary) return <p className="text-muted-foreground">Workflow not found.</p>;
  const trigger = summary.trigger?.kind ?? 'manual';
  return (
    <div className="space-y-3">
      <div>
        <p className="mono text-[10px] uppercase tracking-[0.16em] text-primary">// workflow</p>
        <h3 className="text-sm font-semibold">{summary.name}</h3>
        {summary.description && <p className="text-[11px] text-muted-foreground mt-0.5">{summary.description}</p>}
      </div>

      <dl className="grid grid-cols-2 gap-1 text-[11px]">
        <dt className="text-muted-foreground">Enabled</dt>
        <dd>{summary.is_enabled ? 'yes' : 'no'}</dd>
        <dt className="text-muted-foreground">Trigger</dt>
        <dd className="mono">{trigger}</dd>
        {summary.schedule.cron && (
          <>
            <dt className="text-muted-foreground">Cron</dt>
            <dd className="mono break-all">
              {summary.schedule.cron}
              {!summary.schedule.cron_is_valid && (
                <span className="ml-2 text-destructive">invalid</span>
              )}
            </dd>
          </>
        )}
        <dt className="text-muted-foreground">State</dt>
        <dd>
          <ScheduleStateBadge state={summary.schedule.display_state} />
        </dd>
        <dt className="text-muted-foreground">Updated</dt>
        <dd>{formatTimestamp(summary.updated_at)}</dd>
      </dl>

      <div className="rounded-md border border-border bg-card/40 p-2">
        <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground mb-2">// schedule</p>
        <ScheduleEditor
          summary={summary}
          onChange={next => onSummaryChange?.(next)}
        />
      </div>

      <OwnerBlock
        owner={summary.owner}
        onJumpToWidget={onJumpToWidget}
        onJumpToDatasource={onJumpToDatasource}
      />
    </div>
  );
}

function RunDetailPane({
  detail,
  onJumpToWidget,
  onJumpToDatasource,
  onRetry,
  onCancel,
  busy,
}: {
  detail: WorkflowRunDetail;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
  onJumpToDatasource?: (datasourceDefinitionId: string) => void;
  onRetry: () => void;
  onCancel: () => void;
  busy: boolean;
}) {
  const { run } = detail;
  const duration = run.finished_at ? run.finished_at - run.started_at : undefined;
  return (
    <div className="space-y-3">
      <div className="flex items-start justify-between gap-2">
        <div>
          <p className="mono text-[10px] uppercase tracking-[0.16em] text-primary">// run</p>
          <h3 className="text-sm font-semibold">{detail.workflow_name}</h3>
          <p className="mono text-[10px] text-muted-foreground break-all">{run.id}</p>
        </div>
        <span
          className={`rounded-sm border px-1.5 py-0.5 mono text-[10px] font-semibold uppercase tracking-wider ${STATUS_TONE[run.status]}`}
        >
          {run.status}
        </span>
      </div>

      <dl className="grid grid-cols-2 gap-1 text-[11px]">
        <dt className="text-muted-foreground">Started</dt>
        <dd>{formatTimestamp(run.started_at)}</dd>
        <dt className="text-muted-foreground">Finished</dt>
        <dd>{run.finished_at ? formatTimestamp(run.finished_at) : '—'}</dd>
        <dt className="text-muted-foreground">Duration</dt>
        <dd>{formatDuration(duration)}</dd>
      </dl>

      {run.error && (
        <div className="rounded-md border border-destructive/40 bg-destructive/5 p-2">
          <p className="mono text-[10px] uppercase tracking-[0.16em] text-destructive mb-1">// error</p>
          <pre className="whitespace-pre-wrap break-words text-[11px] text-destructive">{run.error}</pre>
        </div>
      )}

      <div className="flex items-center gap-2">
        <button
          onClick={onRetry}
          disabled={busy}
          className="rounded-md border border-border bg-card px-2 py-1 text-[11px] mono uppercase tracking-wider hover:border-primary/40 transition-colors disabled:opacity-50"
        >
          Retry
        </button>
        {run.status === 'running' && (
          <button
            onClick={onCancel}
            disabled={busy}
            className="rounded-md border border-amber-500/40 bg-card px-2 py-1 text-[11px] mono uppercase tracking-wider text-amber-200 hover:bg-amber-500/10 transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
        )}
      </div>

      {run.node_results && (
        <details className="rounded-md border border-border bg-card/40 p-2">
          <summary className="cursor-pointer mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground">
            // node_results
          </summary>
          <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words text-[10px]">
            {JSON.stringify(run.node_results, null, 2)}
          </pre>
        </details>
      )}

      <OwnerBlock
        owner={detail.owner}
        onJumpToWidget={onJumpToWidget}
        onJumpToDatasource={onJumpToDatasource}
      />
    </div>
  );
}

function OwnerBlock({
  owner,
  onJumpToWidget,
  onJumpToDatasource,
}: {
  owner: WorkflowOwnerRef;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
  onJumpToDatasource?: (datasourceDefinitionId: string) => void;
}) {
  const hasOwnership =
    owner.datasource_definition_id || owner.dashboards.length > 0;
  if (!hasOwnership) {
    return (
      <div className="rounded-md border border-border bg-card/40 p-2">
        <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground mb-1">// owners</p>
        <p className="text-[11px] text-muted-foreground">No widget or datasource references this workflow.</p>
      </div>
    );
  }
  return (
    <div className="rounded-md border border-border bg-card/40 p-2">
      <p className="mono text-[10px] uppercase tracking-[0.16em] text-muted-foreground mb-1">// owners</p>
      {owner.datasource_definition_id && (
        <p className="text-[11px]">
          Datasource:{' '}
          <button
            onClick={() => onJumpToDatasource?.(owner.datasource_definition_id!)}
            disabled={!onJumpToDatasource}
            className="text-primary hover:underline disabled:text-foreground disabled:no-underline"
          >
            {owner.datasource_name ?? owner.datasource_definition_id}
          </button>
        </p>
      )}
      {owner.dashboards.map(d => (
        <div key={d.dashboard_id} className="mt-1">
          <p className="text-[11px] text-muted-foreground">{d.dashboard_name}</p>
          <ul className="ml-3 mt-0.5 space-y-0.5">
            {d.widgets.map(w => (
              <li key={w.widget_id} className="text-[11px] flex items-center gap-2">
                <button
                  onClick={() => onJumpToWidget?.(d.dashboard_id, w.widget_id)}
                  disabled={!onJumpToWidget}
                  className="text-primary hover:underline disabled:text-foreground disabled:no-underline"
                >
                  {w.widget_title}
                </button>
                <span className="mono text-[9px] uppercase tracking-wider text-muted-foreground">
                  {w.widget_kind}
                </span>
                {!w.explicit_binding && (
                  <span className="mono text-[9px] uppercase tracking-wider text-amber-200">legacy</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
