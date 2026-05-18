// W32: Pipeline Studio.
//
// Typed editor for a list of `PipelineStep` values. Replaces the JSON
// textarea in Workbench and powers the inspect/replay loop in
// PipelineDebugModal. The Studio never persists by itself — the caller
// owns the save flow (datasource update / widget binding). It also
// never executes provider / MCP steps directly; replay is deterministic
// and goes through the Rust `replay_pipeline` command.

import { useEffect, useMemo, useState } from 'react';
import {
  debugApi,
  type PipelineStep,
  type PipelineReplayResult,
} from '../../lib/api';
import {
  PIPELINE_STEP_KINDS,
  PIPELINE_STEP_SPECS,
  validatePipeline,
  type PipelineStepKind,
} from '../../lib/pipeline/registry';
import { StepEditor } from './StepEditor';

interface Props {
  steps: PipelineStep[];
  onChange: (steps: PipelineStep[]) => void;
  /** Inline sample the replay runs against. Used when `traceRef` is unset. */
  sample?: unknown;
  /** Reference a stored W23 trace as the replay input source. */
  traceRef?: { widget_id: string; captured_at: number };
  /** Caller-supplied title for the source row (e.g. "Last run output"). */
  sampleLabel?: string;
  /** When true, the embedded "Save" button text becomes "Save". The Studio
   * does not actually save — the parent reads `steps` via `onChange`. This
   * flag only toggles UI affordance. */
  showSaveHint?: boolean;
}

interface StudioRow {
  /** Stable id for React keys, reorder DnD, and "disabled" state. */
  rowId: string;
  step: PipelineStep;
  enabled: boolean;
}

let rowCounter = 0;
const nextRowId = () => `row-${++rowCounter}-${Date.now().toString(36)}`;

function toRows(steps: PipelineStep[]): StudioRow[] {
  return steps.map(step => ({ rowId: nextRowId(), step, enabled: true }));
}

function enabledSteps(rows: StudioRow[]): PipelineStep[] {
  return rows.filter(r => r.enabled).map(r => r.step);
}

function clonePipelineStep(step: PipelineStep): PipelineStep {
  return JSON.parse(JSON.stringify(step)) as PipelineStep;
}

export function PipelineStudio({
  steps,
  onChange,
  sample,
  traceRef,
  sampleLabel,
}: Props) {
  const [rows, setRows] = useState<StudioRow[]>(() => toRows(steps));
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [advancedText, setAdvancedText] = useState(() =>
    JSON.stringify(steps, null, 2),
  );
  const [advancedError, setAdvancedError] = useState<string | null>(null);
  const [replay, setReplay] = useState<PipelineReplayResult | null>(null);
  const [replayBusy, setReplayBusy] = useState(false);
  const [replayError, setReplayError] = useState<string | null>(null);

  // When the parent hands us a new pipeline (e.g. user switches selected
  // datasource), reseed local rows.
  useEffect(() => {
    setRows(toRows(steps));
    setAdvancedText(JSON.stringify(steps, null, 2));
    setReplay(null);
    setReplayError(null);
    setAdvancedError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [steps]);

  const persist = (next: StudioRow[]) => {
    setRows(next);
    const enabled = enabledSteps(next);
    onChange(enabled);
    setAdvancedText(JSON.stringify(enabled, null, 2));
  };

  const validation = useMemo(
    () => validatePipeline(enabledSteps(rows)),
    [rows],
  );

  const addStep = (kind: PipelineStepKind) => {
    const seed = PIPELINE_STEP_SPECS[kind].defaultStep();
    persist([...rows, { rowId: nextRowId(), step: seed, enabled: true }]);
  };

  const updateStep = (rowId: string, next: PipelineStep) => {
    persist(rows.map(r => (r.rowId === rowId ? { ...r, step: next } : r)));
  };

  const removeStep = (rowId: string) => {
    persist(rows.filter(r => r.rowId !== rowId));
  };

  const duplicateStep = (rowId: string) => {
    const idx = rows.findIndex(r => r.rowId === rowId);
    if (idx < 0) return;
    const copy: StudioRow = {
      rowId: nextRowId(),
      step: clonePipelineStep(rows[idx].step),
      enabled: rows[idx].enabled,
    };
    const next = [...rows.slice(0, idx + 1), copy, ...rows.slice(idx + 1)];
    persist(next);
  };

  const toggleEnabled = (rowId: string) => {
    const next = rows.map(r =>
      r.rowId === rowId ? { ...r, enabled: !r.enabled } : r,
    );
    persist(next);
  };

  const move = (rowId: string, delta: -1 | 1) => {
    const idx = rows.findIndex(r => r.rowId === rowId);
    const target = idx + delta;
    if (idx < 0 || target < 0 || target >= rows.length) return;
    const next = [...rows];
    [next[idx], next[target]] = [next[target], next[idx]];
    persist(next);
  };

  const applyAdvanced = () => {
    try {
      const parsed = JSON.parse(advancedText);
      if (!Array.isArray(parsed)) {
        setAdvancedError('Pipeline JSON must be an array of typed steps.');
        return;
      }
      const validation = validatePipeline(parsed as PipelineStep[]);
      if (validation) {
        setAdvancedError(`Step ${validation.index + 1}: ${validation.message}`);
        return;
      }
      setAdvancedError(null);
      persist(toRows(parsed as PipelineStep[]));
      setAdvancedOpen(false);
    } catch (err) {
      setAdvancedError(err instanceof Error ? err.message : 'Invalid JSON');
    }
  };

  const runReplay = async () => {
    if (validation) {
      setReplayError(`Step ${validation.index + 1}: ${validation.message}`);
      return;
    }
    setReplayBusy(true);
    setReplayError(null);
    try {
      const steps = enabledSteps(rows);
      const result = await debugApi.replayPipeline({
        steps,
        sample: traceRef ? undefined : sample,
        from_widget_trace: traceRef,
      });
      setReplay(result);
    } catch (err) {
      setReplayError(err instanceof Error ? err.message : 'Replay failed');
    } finally {
      setReplayBusy(false);
    }
  };

  const canReplay = traceRef !== undefined || sample !== undefined;
  const sampleLabelText =
    sampleLabel ?? (traceRef ? 'Stored trace input' : 'Inline sample');

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">
          Pipeline ({rows.length} step{rows.length === 1 ? '' : 's'})
        </span>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setAdvancedOpen(o => !o)}
            className="rounded border border-border px-2 py-1 text-[11px] hover:bg-muted/40 transition-colors"
            title="Edit the pipeline as JSON (advanced)"
          >
            {advancedOpen ? 'Hide JSON' : 'JSON'}
          </button>
          <button
            type="button"
            onClick={runReplay}
            disabled={replayBusy || !canReplay || rows.length === 0}
            className="rounded border border-accent/40 bg-accent/10 px-2 py-1 text-[11px] text-accent hover:bg-accent/20 transition-colors disabled:opacity-50"
            title={canReplay ? 'Run pipeline against the sample' : 'Provide a sample or trace first'}
          >
            {replayBusy ? 'Replaying…' : 'Replay'}
          </button>
        </div>
      </div>

      {validation && (
        <div className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1 text-[11px] text-destructive">
          Step {validation.index + 1}: {validation.message}
        </div>
      )}

      <ol className="space-y-2">
        {rows.map((row, idx) => (
          <li key={row.rowId}>
            <StepRow
              row={row}
              index={idx}
              total={rows.length}
              onChange={next => updateStep(row.rowId, next)}
              onRemove={() => removeStep(row.rowId)}
              onDuplicate={() => duplicateStep(row.rowId)}
              onToggleEnabled={() => toggleEnabled(row.rowId)}
              onMoveUp={() => move(row.rowId, -1)}
              onMoveDown={() => move(row.rowId, 1)}
              firstEmpty={
                replay?.first_empty_step_index === idx && row.enabled
              }
            />
          </li>
        ))}
      </ol>

      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">
          + add
        </span>
        {PIPELINE_STEP_KINDS.map(kind => {
          const spec = PIPELINE_STEP_SPECS[kind];
          return (
            <button
              key={kind}
              type="button"
              onClick={() => addStep(kind)}
              title={spec.description}
              className={`rounded border px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider transition-colors ${
                spec.advanced
                  ? 'border-neon-amber/40 text-neon-amber hover:bg-neon-amber/10'
                  : 'border-border text-muted-foreground hover:bg-muted/40 hover:text-foreground'
              }`}
            >
              {spec.label}
            </button>
          );
        })}
      </div>

      {advancedOpen && (
        <div className="space-y-1 rounded-md border border-amber-500/30 bg-amber-500/5 p-2">
          <p className="text-[11px] text-amber-300">
            Advanced JSON editor. Useful for paste/import; round-trips through
            the same validator. Click <strong>Apply JSON</strong> to commit.
          </p>
          <textarea
            rows={Math.min(12, Math.max(4, advancedText.split('\n').length))}
            value={advancedText}
            onChange={e => setAdvancedText(e.target.value)}
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-[11px] font-mono"
          />
          {advancedError && (
            <div className="text-[11px] text-destructive">{advancedError}</div>
          )}
          <div className="flex items-center justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                setAdvancedText(JSON.stringify(enabledSteps(rows), null, 2));
                setAdvancedError(null);
              }}
              className="rounded border border-border px-2 py-1 text-[11px] hover:bg-muted/40 transition-colors"
            >
              Revert
            </button>
            <button
              type="button"
              onClick={applyAdvanced}
              className="rounded border border-primary/40 bg-primary/15 px-2 py-1 text-[11px] text-primary hover:bg-primary/25 transition-colors"
            >
              Apply JSON
            </button>
          </div>
        </div>
      )}

      {replayError && (
        <div className="rounded border border-destructive/40 bg-destructive/10 px-2 py-1 text-[11px] text-destructive whitespace-pre-line">
          {replayError}
        </div>
      )}

      {replay && (
        <ReplayPanel result={replay} sampleLabel={sampleLabelText} />
      )}
    </div>
  );
}

function StepRow({
  row,
  index,
  total,
  onChange,
  onRemove,
  onDuplicate,
  onToggleEnabled,
  onMoveUp,
  onMoveDown,
  firstEmpty,
}: {
  row: StudioRow;
  index: number;
  total: number;
  onChange: (next: PipelineStep) => void;
  onRemove: () => void;
  onDuplicate: () => void;
  onToggleEnabled: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  firstEmpty: boolean;
}) {
  const spec = PIPELINE_STEP_SPECS[row.step.kind];
  const stepError = row.enabled ? spec?.validate(row.step) ?? null : null;
  const tone = !row.enabled
    ? 'border-border bg-muted/20 opacity-60'
    : firstEmpty
      ? 'border-neon-amber/50 bg-neon-amber/5'
      : stepError
        ? 'border-destructive/40 bg-destructive/5'
        : 'border-border bg-background/40';
  return (
    <div className={`rounded-md border ${tone}`}>
      <div className="flex items-center justify-between gap-2 border-b border-border/40 px-2 py-1.5">
        <div className="flex items-center gap-2 min-w-0">
          <span className="mono text-[10px] text-muted-foreground">
            {String(index + 1).padStart(2, '0')}
          </span>
          <span className="text-xs font-medium truncate">
            {spec?.label ?? row.step.kind}
          </span>
          {spec?.advanced && (
            <span className="rounded border border-neon-amber/40 bg-neon-amber/10 px-1 text-[9px] mono uppercase tracking-wider text-neon-amber">
              advanced
            </span>
          )}
          {!row.enabled && (
            <span className="rounded border border-border px-1 text-[9px] mono uppercase tracking-wider text-muted-foreground">
              disabled
            </span>
          )}
          {firstEmpty && (
            <span className="rounded border border-neon-amber/40 bg-neon-amber/10 px-1 text-[9px] mono uppercase tracking-wider text-neon-amber">
              first-empty
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={onMoveUp}
            disabled={index === 0}
            className="rounded px-1 text-[11px] text-muted-foreground hover:bg-muted/40 disabled:opacity-30"
            title="Move up"
          >
            ↑
          </button>
          <button
            type="button"
            onClick={onMoveDown}
            disabled={index === total - 1}
            className="rounded px-1 text-[11px] text-muted-foreground hover:bg-muted/40 disabled:opacity-30"
            title="Move down"
          >
            ↓
          </button>
          <button
            type="button"
            onClick={onToggleEnabled}
            className="rounded px-1 text-[11px] text-muted-foreground hover:bg-muted/40"
            title={row.enabled ? 'Skip this step in replay/save' : 'Re-enable this step'}
          >
            {row.enabled ? '◉' : '○'}
          </button>
          <button
            type="button"
            onClick={onDuplicate}
            className="rounded px-1 text-[11px] text-muted-foreground hover:bg-muted/40"
            title="Duplicate"
          >
            ⎘
          </button>
          <button
            type="button"
            onClick={onRemove}
            className="rounded px-1 text-[11px] text-destructive/80 hover:bg-destructive/10"
            title="Remove"
          >
            ✕
          </button>
        </div>
      </div>
      <div className="px-2 py-2">
        <StepEditor step={row.step} onChange={onChange} />
        {stepError && (
          <div className="mt-1 text-[11px] text-destructive">{stepError}</div>
        )}
      </div>
    </div>
  );
}

function ReplayPanel({
  result,
  sampleLabel,
}: {
  result: PipelineReplayResult;
  sampleLabel: string;
}) {
  const duration = result.finished_at - result.started_at;
  return (
    <div className="space-y-2 rounded-md border border-border bg-card/40 p-2">
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px]">
        <span className="mono uppercase tracking-wider text-muted-foreground">
          Replay result
        </span>
        <span className={result.error ? 'text-destructive' : 'text-emerald-400'}>
          {result.error ? `error · ${result.error}` : `ok · ${duration}ms`}
        </span>
      </div>
      {result.first_empty_step_index !== undefined && (
        <div className="rounded border border-neon-amber/40 bg-neon-amber/5 px-2 py-1 text-[11px] text-neon-amber">
          Data became empty at step {result.first_empty_step_index + 1}. Inspect its
          input vs. config.
        </div>
      )}
      <details>
        <summary className="cursor-pointer text-[11px] text-muted-foreground">
          Initial value · {sampleLabel}
        </summary>
        <pre className="mt-1 max-h-40 overflow-auto rounded bg-background/60 p-2 text-[10px] font-mono">
          {previewValue(result.initial_value)}
        </pre>
      </details>
      <details open>
        <summary className="cursor-pointer text-[11px] text-muted-foreground">
          Final value
        </summary>
        <pre className="mt-1 max-h-40 overflow-auto rounded bg-background/60 p-2 text-[10px] font-mono">
          {previewValue(result.final_value)}
        </pre>
      </details>
      {result.steps.length > 0 && (
        <details>
          <summary className="cursor-pointer text-[11px] text-muted-foreground">
            Per-step samples ({result.steps.length})
          </summary>
          <ol className="mt-1 space-y-1">
            {result.steps.map(step => (
              <li
                key={step.index}
                className="rounded border border-border/50 bg-background/50 px-2 py-1 text-[10px] font-mono"
              >
                <div className="flex items-center justify-between">
                  <span>
                    {step.index + 1}. {step.kind}
                  </span>
                  <span className="text-muted-foreground">
                    {step.duration_ms}ms
                    {step.error ? ' · err' : ''}
                  </span>
                </div>
                <div className="mt-0.5 text-[10px] text-muted-foreground">
                  in · {step.input_sample.kind}
                  {step.input_sample.size_hint.items !== undefined
                    ? ` · ${step.input_sample.size_hint.items} items`
                    : ''}
                  {' → '}out · {step.output_sample.kind}
                  {step.output_sample.size_hint.items !== undefined
                    ? ` · ${step.output_sample.size_hint.items} items`
                    : ''}
                </div>
                {step.error && (
                  <div className="mt-0.5 text-[10px] text-destructive">
                    {step.error}
                  </div>
                )}
              </li>
            ))}
          </ol>
        </details>
      )}
    </div>
  );
}

function previewValue(value: unknown): string {
  if (value === undefined) return '(none)';
  try {
    const text = JSON.stringify(value, null, 2);
    return text.length > 8_000 ? `${text.slice(0, 8_000)}\n…(truncated)` : text;
  } catch {
    return String(value);
  }
}
