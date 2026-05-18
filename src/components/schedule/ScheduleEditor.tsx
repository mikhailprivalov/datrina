// W50: shared schedule editor used by the dashboard header (compact),
// Operations cockpit (rich), and Datasource Workbench (rich). Wraps the
// pause/resume + cron commands so every surface renders identical
// labels and validation errors.

import { useMemo, useState } from 'react';
import type {
  ScheduleDisplayState,
  SchedulePreset,
  WorkflowScheduleSummary,
  WorkflowSummary,
} from '../../lib/api';
import { SCHEDULE_PRESETS, scheduleApi } from '../../lib/api';

const DISPLAY_LABEL: Record<ScheduleDisplayState, string> = {
  active: 'Active',
  paused_by_user: 'Paused',
  manual_only: 'Manual only',
  invalid: 'Invalid cron',
  disabled: 'Disabled',
  not_scheduled: 'Not scheduled',
};

const DISPLAY_TONE: Record<ScheduleDisplayState, string> = {
  active: 'border-emerald-500/40 bg-emerald-500/10 text-emerald-300',
  paused_by_user: 'border-amber-500/40 bg-amber-500/10 text-amber-300',
  manual_only: 'border-border bg-muted/30 text-muted-foreground',
  invalid: 'border-destructive/40 bg-destructive/10 text-destructive',
  disabled: 'border-border bg-muted/30 text-muted-foreground',
  not_scheduled: 'border-border bg-muted/30 text-muted-foreground',
};

export function ScheduleStateBadge({ state }: { state: ScheduleDisplayState }) {
  return (
    <span
      className={`rounded-sm border px-1.5 py-0.5 text-[10px] mono font-semibold uppercase tracking-wider ${DISPLAY_TONE[state]}`}
    >
      {DISPLAY_LABEL[state]}
    </span>
  );
}

interface Props {
  summary: WorkflowSummary;
  /** Called after every successful mutation with the new typed summary
   *  so the parent can keep its local copy in sync without re-fetching. */
  onChange: (next: WorkflowSummary) => void;
  /** When `true`, hides the description column so the editor fits inside
   *  a compact toolbar. */
  compact?: boolean;
}

function matchPreset(cron?: string): SchedulePreset | null {
  if (!cron) return SCHEDULE_PRESETS.find(p => p.cron === null) ?? null;
  return SCHEDULE_PRESETS.find(p => p.cron === cron) ?? null;
}

export function ScheduleEditor({ summary, onChange, compact = false }: Props) {
  const schedule = summary.schedule;
  const initialPreset = matchPreset(schedule.cron_normalized ?? schedule.cron);
  const [presetId, setPresetId] = useState<string>(initialPreset?.id ?? 'custom');
  const [customCron, setCustomCron] = useState<string>(schedule.cron ?? '');
  const [busy, setBusy] = useState<'pause' | 'resume' | 'cron' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const isPaused = schedule.pause_state === 'paused';
  const isCronTrigger = schedule.trigger_kind === 'cron';
  const canManageCron = summary.is_enabled;

  const presetIsCustom = useMemo(() => {
    const matched = matchPreset(schedule.cron_normalized ?? schedule.cron);
    return matched === null;
  }, [schedule.cron, schedule.cron_normalized]);

  async function handlePause() {
    setBusy('pause');
    setError(null);
    try {
      const next = await scheduleApi.pauseWorkflow(summary.id);
      onChange(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleResume() {
    setBusy('resume');
    setError(null);
    try {
      const next = await scheduleApi.resumeWorkflow(summary.id);
      onChange(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function applyCron(nextCron: string | null) {
    setBusy('cron');
    setError(null);
    try {
      const next = await scheduleApi.setWorkflowCron(summary.id, nextCron);
      onChange(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  function handlePresetChange(nextId: string) {
    setPresetId(nextId);
    if (nextId === 'custom') return;
    const preset = SCHEDULE_PRESETS.find(p => p.id === nextId);
    if (!preset) return;
    setCustomCron(preset.cron ?? '');
    applyCron(preset.cron);
  }

  async function handleCustomSubmit(event?: React.FormEvent) {
    event?.preventDefault();
    const trimmed = customCron.trim();
    await applyCron(trimmed.length === 0 ? null : trimmed);
  }

  return (
    <div className="space-y-2 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <ScheduleStateBadge state={schedule.display_state} />
        {schedule.cron_normalized && (
          <span className="rounded-sm border border-border bg-muted/40 px-1.5 py-0.5 mono text-[10px] text-muted-foreground">
            {schedule.cron_normalized}
          </span>
        )}
        {isPaused && schedule.last_paused_at && (
          <span className="text-[10px] text-muted-foreground">
            paused {new Date(schedule.last_paused_at).toLocaleString()}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {isPaused ? (
          <button
            type="button"
            onClick={handleResume}
            disabled={busy !== null || !canManageCron}
            className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-2.5 py-1 text-[11px] mono uppercase tracking-wider text-emerald-300 hover:bg-emerald-500/20 transition-colors disabled:opacity-50"
            title="Resume automatic refresh"
          >
            {busy === 'resume' ? 'Resuming…' : 'Resume'}
          </button>
        ) : (
          <button
            type="button"
            onClick={handlePause}
            disabled={busy !== null || !isCronTrigger || !canManageCron}
            className="rounded-md border border-amber-500/40 bg-amber-500/10 px-2.5 py-1 text-[11px] mono uppercase tracking-wider text-amber-300 hover:bg-amber-500/20 transition-colors disabled:opacity-50"
            title={
              isCronTrigger
                ? 'Pause automatic refresh — manual refresh still works'
                : 'Workflow has no cron trigger; nothing to pause'
            }
          >
            {busy === 'pause' ? 'Pausing…' : 'Pause'}
          </button>
        )}
        <select
          value={presetIsCustom ? 'custom' : presetId}
          onChange={e => handlePresetChange(e.target.value)}
          disabled={busy !== null || isPaused || !canManageCron}
          className="rounded-md border border-border bg-background px-2 py-1 text-[11px] mono"
          title="Pick a cadence preset"
        >
          {SCHEDULE_PRESETS.map(p => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
          <option value="custom">Advanced (cron)…</option>
        </select>
      </div>
      {(presetId === 'custom' || presetIsCustom) && (
        <form onSubmit={handleCustomSubmit} className="flex flex-wrap items-center gap-2">
          <input
            type="text"
            value={customCron}
            onChange={e => setCustomCron(e.target.value)}
            placeholder="6-field cron, e.g. 0 */5 * * * *"
            className="flex-1 min-w-[16rem] rounded-md border border-border bg-background px-2 py-1 text-[11px] mono"
            disabled={busy !== null || isPaused || !canManageCron}
          />
          <button
            type="submit"
            disabled={busy !== null || isPaused || !canManageCron}
            className="rounded-md border border-primary/40 bg-primary/15 px-2.5 py-1 text-[11px] mono uppercase tracking-wider text-primary hover:bg-primary/25 transition-colors disabled:opacity-50"
          >
            {busy === 'cron' ? 'Saving…' : 'Save'}
          </button>
        </form>
      )}
      {!compact && schedule.last_pause_reason && (
        <p className="text-[10px] text-muted-foreground">
          last pause reason: {schedule.last_pause_reason}
        </p>
      )}
      {error && <p className="text-[11px] text-destructive">{error}</p>}
    </div>
  );
}

/** Single-row helper that renders just the badge + cron string. Useful
 *  in lists where the user picks a row to expand into the full editor. */
export function ScheduleSummaryLine({ summary }: { summary: WorkflowScheduleSummary }) {
  return (
    <span className="inline-flex items-center gap-2">
      <ScheduleStateBadge state={summary.display_state} />
      {summary.cron_normalized && (
        <span className="mono text-[10px] text-muted-foreground">
          {summary.cron_normalized}
        </span>
      )}
    </span>
  );
}
