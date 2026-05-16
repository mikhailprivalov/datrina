import { useEffect, useMemo, useState } from 'react';
import { alertApi } from '../../lib/api';
import type {
  AgentAction,
  AlertCondition,
  AlertSeverity,
  PresenceExpectation,
  ThresholdOp,
  WidgetAlert,
  WidgetRuntimeData,
} from '../../lib/api';

interface Props {
  dashboardId: string;
  widgetId: string;
  widgetTitle: string;
  lastData?: WidgetRuntimeData;
  onClose: () => void;
  onSaved: (alerts: WidgetAlert[]) => void;
}

const SEVERITIES: AlertSeverity[] = ['info', 'warning', 'critical'];
const THRESHOLD_OPS: ThresholdOp[] = ['gt', 'lt', 'gte', 'lte', 'eq', 'neq'];
const PRESENCES: PresenceExpectation[] = ['present', 'absent', 'empty', 'non_empty'];

function newAlert(): WidgetAlert {
  return {
    id: cryptoRandomId(),
    name: 'New alert',
    condition: { kind: 'threshold', path: 'value', op: 'gt', value: 0 },
    severity: 'warning',
    message_template: 'Value {value} crossed threshold {threshold} at {path}',
    cooldown_seconds: 600,
    enabled: true,
  };
}

function cryptoRandomId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID();
  return `alert-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export function AlertEditorModal({
  dashboardId,
  widgetId,
  widgetTitle,
  lastData,
  onClose,
  onSaved,
}: Props) {
  const [alerts, setAlerts] = useState<WidgetAlert[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [isLoading, setLoading] = useState(true);
  const [isSaving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    alertApi
      .getForWidget(widgetId)
      .then(data => {
        if (cancelled) return;
        setAlerts(data);
        setActiveId(data[0]?.id ?? null);
      })
      .catch(err => setError(err instanceof Error ? err.message : 'Failed to load alerts'))
      .finally(() => setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [widgetId]);

  const active = useMemo(
    () => alerts.find(a => a.id === activeId) ?? null,
    [alerts, activeId]
  );

  const updateActive = (patch: Partial<WidgetAlert>) => {
    if (!active) return;
    setAlerts(prev =>
      prev.map(a => (a.id === active.id ? { ...a, ...patch } : a))
    );
  };

  const updateCondition = (cond: AlertCondition) => updateActive({ condition: cond });

  const addAlert = () => {
    const next = newAlert();
    setAlerts(prev => [...prev, next]);
    setActiveId(next.id);
  };

  const removeAlert = (id: string) => {
    setAlerts(prev => prev.filter(a => a.id !== id));
    if (activeId === id) setActiveId(null);
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await alertApi.setForWidget({
        dashboard_id: dashboardId,
        widget_id: widgetId,
        alerts,
      });
      onSaved(saved);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save alerts');
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    if (!active) return;
    setTestResult(null);
    try {
      if (!lastData) {
        setTestResult('No recent runtime data captured for this widget — refresh once and try again.');
        return;
      }
      const result = await alertApi.testCondition(active.condition, lastData);
      const resolved = JSON.stringify(result.resolved_value);
      setTestResult(
        result.fired
          ? `Would fire. Resolved value: ${resolved}`
          : `Would NOT fire. Resolved value: ${resolved}`
      );
    } catch (err) {
      setTestResult(err instanceof Error ? err.message : 'Test failed');
    }
  };

  const toggleAgentAction = (enabled: boolean) => {
    if (!active) return;
    updateActive({
      agent_action: enabled
        ? {
            mode: 'build',
            prompt_template:
              'Alert on widget "{widget}": {message}. Suggest next steps. value={value} path={path} threshold={threshold}',
            max_runs_per_day: 5,
            allow_apply: false,
          }
        : undefined,
    });
  };

  const updateAgentAction = (patch: Partial<AgentAction>) => {
    if (!active?.agent_action) return;
    updateActive({ agent_action: { ...active.agent_action, ...patch } });
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex h-[min(85vh,42rem)] w-[min(95vw,60rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div>
            <p className="text-sm font-semibold">Alerts — {widgetTitle}</p>
            <p className="text-[11px] text-muted-foreground">
              Each alert is evaluated after every successful refresh.
            </p>
          </div>
          <button onClick={onClose} className="rounded p-1 hover:bg-muted" aria-label="Close">
            <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        {error && (
          <div className="mx-4 mt-3 rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
            {error}
          </div>
        )}
        <div className="flex flex-1 overflow-hidden">
          <div className="w-60 flex-shrink-0 border-r border-border overflow-auto scrollbar-thin">
            {isLoading ? (
              <p className="px-3 py-2 text-xs text-muted-foreground">Loading…</p>
            ) : alerts.length === 0 ? (
              <p className="px-3 py-2 text-xs text-muted-foreground">No alerts yet.</p>
            ) : (
              <ul>
                {alerts.map(alert => (
                  <li key={alert.id}>
                    <button
                      onClick={() => setActiveId(alert.id)}
                      className={`block w-full truncate px-3 py-2 text-left text-xs ${
                        activeId === alert.id ? 'bg-muted' : 'hover:bg-muted/50'
                      }`}
                    >
                      <span className={alert.enabled ? '' : 'opacity-50'}>{alert.name}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <button
              onClick={addAlert}
              className="block w-full px-3 py-2 text-left text-xs text-primary hover:bg-muted/50"
            >
              + Add alert
            </button>
          </div>
          <div className="flex-1 overflow-auto p-4 scrollbar-thin">
            {!active ? (
              <p className="text-sm text-muted-foreground">Select an alert on the left or add one.</p>
            ) : (
              <div className="space-y-3">
                <Field label="Name">
                  <input
                    value={active.name}
                    onChange={e => updateActive({ name: e.target.value })}
                    className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
                  />
                </Field>
                <div className="grid grid-cols-2 gap-3">
                  <Field label="Severity">
                    <select
                      value={active.severity}
                      onChange={e => updateActive({ severity: e.target.value as AlertSeverity })}
                      className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
                    >
                      {SEVERITIES.map(s => (
                        <option key={s} value={s}>
                          {s}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field label="Cooldown (seconds)">
                    <input
                      type="number"
                      min={0}
                      value={active.cooldown_seconds}
                      onChange={e =>
                        updateActive({ cooldown_seconds: Math.max(0, Number(e.target.value) || 0) })
                      }
                      className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
                    />
                  </Field>
                </div>
                <ConditionEditor condition={active.condition} onChange={updateCondition} />
                <Field label="Message template">
                  <textarea
                    value={active.message_template}
                    onChange={e => updateActive({ message_template: e.target.value })}
                    rows={2}
                    className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
                  />
                  <p className="mt-1 text-[10px] text-muted-foreground">
                    Placeholders: <code>{'{value}'}</code> <code>{'{path}'}</code> <code>{'{threshold}'}</code>
                  </p>
                </Field>
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={active.enabled}
                    onChange={e => updateActive({ enabled: e.target.checked })}
                  />
                  Enabled
                </label>
                <div className="rounded-lg border border-border bg-background/50 p-3">
                  <label className="flex items-center gap-2 text-sm font-medium">
                    <input
                      type="checkbox"
                      checked={Boolean(active.agent_action)}
                      onChange={e => toggleAgentAction(e.target.checked)}
                    />
                    Autonomous agent trigger
                  </label>
                  {active.agent_action && (
                    <div className="mt-2 space-y-2">
                      <Field label="Prompt template">
                        <textarea
                          value={active.agent_action.prompt_template}
                          onChange={e => updateAgentAction({ prompt_template: e.target.value })}
                          rows={3}
                          className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
                        />
                        <p className="mt-1 text-[10px] text-muted-foreground">
                          Placeholders: <code>{'{widget}'}</code> <code>{'{message}'}</code> <code>{'{value}'}</code> <code>{'{path}'}</code> <code>{'{threshold}'}</code>
                        </p>
                      </Field>
                      <div className="grid grid-cols-2 gap-3">
                        <Field label="Mode">
                          <select
                            value={active.agent_action.mode}
                            onChange={e =>
                              updateAgentAction({ mode: e.target.value as 'build' | 'context' })
                            }
                            className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
                          >
                            <option value="build">build</option>
                            <option value="context">context</option>
                          </select>
                        </Field>
                        <Field label="Max runs / day">
                          <input
                            type="number"
                            min={1}
                            value={active.agent_action.max_runs_per_day}
                            onChange={e =>
                              updateAgentAction({
                                max_runs_per_day: Math.max(1, Number(e.target.value) || 1),
                              })
                            }
                            className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
                          />
                        </Field>
                      </div>
                      <label className="flex items-center gap-2 text-xs text-muted-foreground">
                        <input
                          type="checkbox"
                          checked={active.agent_action.allow_apply}
                          onChange={e => updateAgentAction({ allow_apply: e.target.checked })}
                        />
                        Allow auto-apply (default off — agent only suggests)
                      </label>
                    </div>
                  )}
                </div>
                <div className="flex items-center justify-between border-t border-border/60 pt-3">
                  <button
                    onClick={() => removeAlert(active.id)}
                    className="rounded-md border border-destructive/40 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10"
                  >
                    Delete alert
                  </button>
                  <div className="flex items-center gap-2">
                    <button
                      onClick={handleTest}
                      className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted"
                    >
                      Test against last data
                    </button>
                  </div>
                </div>
                {testResult && (
                  <div className="rounded-md border border-border bg-background/40 px-3 py-2 text-xs">
                    {testResult}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
          <button
            onClick={onClose}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={isSaving}
            className="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90 disabled:opacity-60"
          >
            {isSaving ? 'Saving…' : 'Save alerts'}
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block text-xs">
      <span className="mb-1 block text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function ConditionEditor({
  condition,
  onChange,
}: {
  condition: AlertCondition;
  onChange: (next: AlertCondition) => void;
}) {
  return (
    <div className="rounded-lg border border-border bg-background/40 p-3 space-y-2">
      <Field label="Condition kind">
        <select
          value={condition.kind}
          onChange={e => onChange(emptyCondition(e.target.value as AlertCondition['kind']))}
          className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
        >
          <option value="threshold">Threshold</option>
          <option value="path_present">Path presence</option>
          <option value="status_equals">Status equals</option>
          <option value="custom">Custom (resolve_path truthiness)</option>
        </select>
      </Field>
      {condition.kind === 'threshold' && (
        <div className="grid grid-cols-3 gap-2">
          <Field label="Path">
            <input
              value={condition.path}
              onChange={e => onChange({ ...condition, path: e.target.value })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
            />
          </Field>
          <Field label="Op">
            <select
              value={condition.op}
              onChange={e => onChange({ ...condition, op: e.target.value as ThresholdOp })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
            >
              {THRESHOLD_OPS.map(op => (
                <option key={op} value={op}>
                  {op}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Threshold value">
            <input
              value={
                typeof condition.value === 'string'
                  ? condition.value
                  : JSON.stringify(condition.value)
              }
              onChange={e => onChange({ ...condition, value: parseScalar(e.target.value) })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
            />
          </Field>
        </div>
      )}
      {condition.kind === 'path_present' && (
        <div className="grid grid-cols-2 gap-2">
          <Field label="Path">
            <input
              value={condition.path}
              onChange={e => onChange({ ...condition, path: e.target.value })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
            />
          </Field>
          <Field label="Expected">
            <select
              value={condition.expected}
              onChange={e =>
                onChange({ ...condition, expected: e.target.value as PresenceExpectation })
              }
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
            >
              {PRESENCES.map(p => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </Field>
        </div>
      )}
      {condition.kind === 'status_equals' && (
        <div className="grid grid-cols-2 gap-2">
          <Field label="Path">
            <input
              value={condition.path}
              onChange={e => onChange({ ...condition, path: e.target.value })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
            />
          </Field>
          <Field label="Status string">
            <input
              value={condition.status}
              onChange={e => onChange({ ...condition, status: e.target.value })}
              className="w-full rounded border border-border bg-background px-2 py-1 text-sm"
            />
          </Field>
        </div>
      )}
      {condition.kind === 'custom' && (
        <Field label="JMESPath-like expression (truthy => fire)">
          <input
            value={condition.jmespath_expr}
            onChange={e => onChange({ ...condition, jmespath_expr: e.target.value })}
            className="w-full rounded border border-border bg-background px-2 py-1 text-sm font-mono"
          />
        </Field>
      )}
    </div>
  );
}

function emptyCondition(kind: AlertCondition['kind']): AlertCondition {
  switch (kind) {
    case 'threshold':
      return { kind: 'threshold', path: 'value', op: 'gt', value: 0 };
    case 'path_present':
      return { kind: 'path_present', path: 'value', expected: 'present' };
    case 'status_equals':
      return { kind: 'status_equals', path: 'status', status: 'down' };
    case 'custom':
      return { kind: 'custom', jmespath_expr: 'value' };
  }
}

function parseScalar(raw: string): unknown {
  const trimmed = raw.trim();
  if (trimmed === '') return '';
  if (trimmed === 'true') return true;
  if (trimmed === 'false') return false;
  if (trimmed === 'null') return null;
  const n = Number(trimmed);
  if (!Number.isNaN(n) && trimmed.match(/^-?[0-9.]+$/)) return n;
  try {
    return JSON.parse(trimmed);
  } catch {
    return trimmed;
  }
}
