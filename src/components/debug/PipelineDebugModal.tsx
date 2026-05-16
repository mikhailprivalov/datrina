import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  debugApi,
  type PipelineStepTrace,
  type PipelineTrace,
  type SampleValue,
  type SourceSummary,
  type TraceEntry,
} from '../../lib/api';

interface Props {
  dashboardId: string;
  widgetId: string;
  widgetTitle: string;
  initialCaptureTraces: boolean;
  onClose: () => void;
  onCaptureChange?: (capture: boolean) => void;
}

export function PipelineDebugModal({
  dashboardId,
  widgetId,
  widgetTitle,
  initialCaptureTraces,
  onClose,
  onCaptureChange,
}: Props) {
  const [history, setHistory] = useState<TraceEntry[]>([]);
  const [selectedAt, setSelectedAt] = useState<number | null>(null);
  const [activeTrace, setActiveTrace] = useState<PipelineTrace | null>(null);
  const [loading, setLoading] = useState(true);
  const [running, setRunning] = useState(false);
  const [capture, setCapture] = useState(initialCaptureTraces);
  const [error, setError] = useState<string | null>(null);

  const refreshHistory = useCallback(async () => {
    try {
      const entries = await debugApi.listTraces(widgetId);
      setHistory(entries);
      return entries;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load traces');
      return [];
    }
  }, [widgetId]);

  // On first open: ensure capture is enabled so the next refresh becomes
  // visible, then load persisted history and immediately run a one-off
  // traced refresh so the modal is never blank.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      try {
        if (!initialCaptureTraces) {
          await debugApi.setCaptureTraces(dashboardId, widgetId, true);
          if (!cancelled) {
            setCapture(true);
            onCaptureChange?.(true);
          }
        }
        const entries = await refreshHistory();
        if (cancelled) return;
        if (entries.length > 0) {
          setSelectedAt(entries[0].captured_at);
          setActiveTrace(entries[0].trace);
        } else {
          await runTrace(/*persist=*/ false);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to initialize debug view');
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dashboardId, widgetId]);

  const runTrace = useCallback(
    async (persist: boolean) => {
      setRunning(true);
      setError(null);
      try {
        const trace = await debugApi.traceWidget(dashboardId, widgetId);
        setActiveTrace(trace);
        if (persist) {
          // Backend trace_widget_pipeline does not persist on its own;
          // re-run via refresh-with-capture would, but for an explicit
          // "Run with trace" button we don't want to mutate persistence.
          // Just surface the live trace.
        }
        if (persist) {
          await refreshHistory();
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Trace run failed');
      } finally {
        setRunning(false);
      }
    },
    [dashboardId, widgetId, refreshHistory]
  );

  const handleSelectHistory = (capturedAt: number) => {
    const entry = history.find(h => h.captured_at === capturedAt);
    if (!entry) return;
    setSelectedAt(capturedAt);
    setActiveTrace(entry.trace);
  };

  const handleCaptureToggle = async () => {
    const next = !capture;
    try {
      await debugApi.setCaptureTraces(dashboardId, widgetId, next);
      setCapture(next);
      onCaptureChange?.(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update capture setting');
    }
  };

  const emptyAtStep = useMemo(() => findFirstEmptyStep(activeTrace), [activeTrace]);
  const lastTraceAgo = useMemo(() => {
    if (!activeTrace) return null;
    return formatAgo(Date.now() - activeTrace.finished_at);
  }, [activeTrace]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex max-h-[90vh] w-[min(95vw,64rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium truncate">Debug pipeline — {widgetTitle}</p>
            <p className="text-[11px] text-muted-foreground truncate">
              {lastTraceAgo ? `Last trace ${lastTraceAgo} ago` : 'No trace yet'}
              {' · '}
              <label className="inline-flex items-center gap-1">
                <input
                  type="checkbox"
                  checked={capture}
                  onChange={handleCaptureToggle}
                  className="h-3 w-3"
                />
                <span>Capture on refresh</span>
              </label>
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => runTrace(false)}
              disabled={running}
              className="rounded-md border border-border px-2.5 py-1 text-xs hover:bg-muted disabled:opacity-50"
              title="Run a one-off traced refresh now"
            >
              {running ? 'Running…' : 'Run with trace'}
            </button>
            <button onClick={onClose} className="p-1 rounded hover:bg-muted">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        </div>

        {error && (
          <div className="border-b border-border bg-destructive/10 px-4 py-2 text-xs text-destructive">
            {error}
          </div>
        )}

        <div className="flex-1 overflow-auto">
          {loading ? (
            <div className="flex h-32 items-center justify-center text-xs text-muted-foreground">
              Loading…
            </div>
          ) : activeTrace ? (
            <div className="flex flex-col gap-3 p-4">
              <SourceHeader summary={activeTrace.source_summary} />
              {emptyAtStep !== null && (
                <div className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-[12px] text-amber-700 dark:text-amber-400">
                  ⚠ Data became empty at step {emptyAtStep + 1}. Inspect that step's input
                  vs. its config — the upstream shape likely doesn't match the path or filter.
                </div>
              )}
              {activeTrace.error && !emptyAtStep && (
                <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-[12px] text-destructive">
                  {activeTrace.error}
                </div>
              )}
              {activeTrace.steps.length === 0 ? (
                <div className="rounded-md border border-border bg-background/60 px-3 py-2 text-[12px] text-muted-foreground">
                  This widget has no pipeline transforms — the final value comes straight
                  from the source above.
                </div>
              ) : (
                <ol className="flex flex-col gap-2">
                  {activeTrace.steps.map(step => (
                    <li key={step.index}>
                      <StepRow step={step} />
                    </li>
                  ))}
                </ol>
              )}
              <FinalValuePanel value={activeTrace.final_value} />
            </div>
          ) : (
            <div className="flex h-32 items-center justify-center text-xs text-muted-foreground">
              No trace available.
            </div>
          )}
        </div>

        {history.length > 0 && (
          <div className="border-t border-border bg-background/40 px-4 py-2">
            <p className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
              Recent traces (ring buffer, last 5)
            </p>
            <div className="flex flex-wrap gap-2">
              {history.map(entry => {
                const isActive = selectedAt === entry.captured_at;
                return (
                  <button
                    key={entry.captured_at}
                    onClick={() => handleSelectHistory(entry.captured_at)}
                    className={`rounded border px-2 py-0.5 text-[11px] font-mono ${
                      isActive
                        ? 'border-primary bg-primary/10 text-primary'
                        : 'border-border hover:bg-muted'
                    }`}
                  >
                    {formatTimestamp(entry.captured_at)}
                    {entry.trace.error ? ' · err' : ''}
                  </button>
                );
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function StepRow({ step }: { step: PipelineStepTrace }) {
  const [open, setOpen] = useState(false);
  const failed = !!step.error;
  const outputEmpty = isEmptySample(step.output_sample);
  const tone = failed
    ? 'border-destructive/50 bg-destructive/5'
    : outputEmpty
      ? 'border-amber-500/40 bg-amber-500/5'
      : 'border-border';
  const status = failed ? '✗' : '✓';
  const statusColor = failed
    ? 'text-destructive'
    : outputEmpty
      ? 'text-amber-600 dark:text-amber-400'
      : 'text-emerald-600 dark:text-emerald-400';

  return (
    <div className={`rounded-md border ${tone}`}>
      <button
        onClick={() => setOpen(o => !o)}
        className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-xs hover:bg-muted/30"
      >
        <span className="flex items-center gap-2 truncate">
          <span className="font-mono text-muted-foreground">Step {step.index + 1}</span>
          <span className="font-medium">{step.kind}</span>
          <span className="text-muted-foreground truncate">
            {summarizeConfig(step.config_json)}
          </span>
        </span>
        <span className="flex items-center gap-2 text-[11px] font-mono">
          <span className={statusColor}>{status}</span>
          <span className="text-muted-foreground">{step.duration_ms} ms</span>
        </span>
      </button>
      {open && (
        <div className="border-t border-border/50 px-3 py-2 text-[11px] font-mono space-y-2">
          {step.error && (
            <div className="rounded bg-destructive/10 px-2 py-1 text-destructive">
              {step.error}
            </div>
          )}
          <SamplePane label="in" sample={step.input_sample} />
          <SamplePane label="out" sample={step.output_sample} emptyHint={outputEmpty} />
          <details className="text-muted-foreground">
            <summary className="cursor-pointer">step config</summary>
            <pre className="mt-1 max-h-48 overflow-auto rounded bg-background/70 p-2 text-[10px]">
              {JSON.stringify(step.config_json, null, 2)}
            </pre>
          </details>
        </div>
      )}
    </div>
  );
}

function SamplePane({
  label,
  sample,
  emptyHint,
}: {
  label: string;
  sample: SampleValue;
  emptyHint?: boolean;
}) {
  return (
    <div>
      <div className="mb-0.5 flex items-center gap-2 text-[10px] uppercase tracking-wide text-muted-foreground">
        <span>{label}</span>
        <span>· {sample.kind}</span>
        {sample.size_hint.items !== undefined && <span>· {sample.size_hint.items} items</span>}
        {sample.size_hint.bytes !== undefined && <span>· {sample.size_hint.bytes} bytes</span>}
        {emptyHint && <span className="text-amber-600 dark:text-amber-400">· empty</span>}
      </div>
      <pre className="max-h-40 overflow-auto rounded bg-background/70 p-2 text-[10px]">
        {previewJson(sample.preview)}
      </pre>
    </div>
  );
}

function SourceHeader({ summary }: { summary: SourceSummary }) {
  let description: string;
  let detail: string | null = null;
  switch (summary.kind) {
    case 'mcp_tool':
      description = `mcp_tool ${summary.server_id}.${summary.tool_name}`;
      detail = summary.arguments ? JSON.stringify(summary.arguments) : null;
      break;
    case 'builtin_tool':
      description = `builtin_tool ${summary.tool_name}`;
      detail = summary.arguments ? JSON.stringify(summary.arguments) : null;
      break;
    case 'provider_prompt':
      description = 'provider_prompt';
      detail = summary.prompt;
      break;
    case 'unknown':
      description = 'unknown source';
      break;
  }
  return (
    <div className="rounded-md border border-border bg-background/40 px-3 py-2 text-[11px] font-mono">
      <p className="text-muted-foreground">Source: <span className="text-foreground">{description}</span></p>
      {detail && (
        <pre className="mt-1 max-h-24 overflow-auto whitespace-pre-wrap break-all text-[10px] text-muted-foreground">
          {detail}
        </pre>
      )}
    </div>
  );
}

function FinalValuePanel({ value }: { value: unknown }) {
  return (
    <div className="rounded-md border border-border bg-background/40 px-3 py-2">
      <p className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">
        Final value
      </p>
      <pre className="max-h-48 overflow-auto rounded bg-background/70 p-2 text-[10px] font-mono">
        {previewJson(value)}
      </pre>
    </div>
  );
}

function isEmptySample(sample: SampleValue): boolean {
  if (sample.kind === 'null') return true;
  if (sample.kind === 'array_head' && (sample.size_hint.items ?? 0) === 0) return true;
  return false;
}

function findFirstEmptyStep(trace: PipelineTrace | null): number | null {
  if (!trace) return null;
  for (const step of trace.steps) {
    if (step.error) return step.index;
    if (isEmptySample(step.output_sample)) return step.index;
  }
  return null;
}

function summarizeConfig(config: unknown): string {
  if (!config || typeof config !== 'object') return '';
  const cfg = config as Record<string, unknown>;
  const parts: string[] = [];
  for (const key of ['path', 'field', 'by', 'count', 'template', 'group_by', 'metric', 'op', 'value', 'to']) {
    if (cfg[key] === undefined) continue;
    const v = cfg[key];
    if (typeof v === 'string') {
      parts.push(`${key}=${v.length > 30 ? v.slice(0, 30) + '…' : v}`);
    } else if (typeof v === 'number' || typeof v === 'boolean') {
      parts.push(`${key}=${v}`);
    }
    if (parts.length >= 3) break;
  }
  return parts.length ? `{ ${parts.join(', ')} }` : '';
}

function previewJson(value: unknown): string {
  if (value === undefined) return '(none)';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function formatTimestamp(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString();
}

function formatAgo(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h`;
}
