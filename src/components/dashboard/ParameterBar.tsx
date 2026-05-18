import { useCallback, useEffect, useMemo, useState } from 'react';
import { dashboardApi } from '../../lib/api';
import type {
  DashboardParameter,
  DashboardParameterState,
  ParameterOption,
  ParameterValue,
} from '../../lib/api';

interface Props {
  dashboardId: string;
  parameters: DashboardParameter[];
  /** Called with affected widget ids after a parameter value is committed. */
  onAffectedWidgets: (widgetIds: string[]) => void;
  /** W34: optional initial selections (e.g. decoded from URL hash). */
  initialSelections?: Record<string, ParameterValue>;
  /** W34: notified whenever any selection commits so the host can sync. */
  onSelectionChange?: (values: Record<string, ParameterValue>) => void;
}

const INTERVAL_PRESETS = ['1m', '5m', '15m', '1h', '6h', '24h'];

const TIME_RANGE_PRESETS: Array<{ label: string; ms: number }> = [
  { label: 'Last 5m', ms: 5 * 60 * 1000 },
  { label: 'Last 15m', ms: 15 * 60 * 1000 },
  { label: 'Last 1h', ms: 60 * 60 * 1000 },
  { label: 'Last 6h', ms: 6 * 60 * 60 * 1000 },
  { label: 'Last 24h', ms: 24 * 60 * 60 * 1000 },
  { label: 'Last 7d', ms: 7 * 24 * 60 * 60 * 1000 },
];

function valueToString(value: ParameterValue | undefined): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number') return String(value);
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (Array.isArray(value)) return value.map(valueToString).join(',');
  if (typeof value === 'object' && 'from' in value && 'to' in value) {
    return `${value.from}-${value.to}`;
  }
  return '';
}

function isQueryKind(kind: DashboardParameter['kind']): boolean {
  return kind === 'mcp_query' || kind === 'http_query' || kind === 'datasource_query';
}

export function ParameterBar({
  dashboardId,
  parameters,
  onAffectedWidgets,
  initialSelections,
  onSelectionChange,
}: Props) {
  const [states, setStates] = useState<DashboardParameterState[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingName, setPendingName] = useState<string | null>(null);
  const [refreshingName, setRefreshingName] = useState<string | null>(null);
  // Initial-selection application is one-shot per dashboard mount to avoid
  // re-applying stale hash values after the user clicks something.
  const [appliedInitial, setAppliedInitial] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await dashboardApi.listParameters(dashboardId);
      setStates(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load parameters');
    } finally {
      setLoading(false);
    }
  }, [dashboardId]);

  useEffect(() => {
    if (parameters.length === 0) {
      setStates([]);
      setLoading(false);
      return;
    }
    void load();
  }, [load, parameters.length]);

  // W34: emit a denormalized selection map whenever it changes so the
  // host can sync to URL hash without re-fetching.
  useEffect(() => {
    if (!onSelectionChange) return;
    const map: Record<string, ParameterValue> = {};
    for (const state of states) {
      if (state.value !== undefined) map[state.parameter.name] = state.value;
    }
    onSelectionChange(map);
  }, [states, onSelectionChange]);

  const commitValue = useCallback(
    async (param: DashboardParameter, value: ParameterValue) => {
      setPendingName(param.name);
      setError(null);
      try {
        const result = await dashboardApi.setParameterValue(dashboardId, param.name, value);
        setStates(prev => {
          // Apply the new value to the changed param, then merge any
          // re-resolved downstream states so cascading selectors update
          // without an extra round-trip. Preserve the previously selected
          // value on downstream params whose backend errored, so a
          // transient failure doesn't blank the dropdown.
          const downstreamByName = new Map<string, DashboardParameterState>();
          for (const s of result.downstream ?? []) {
            downstreamByName.set(s.parameter.name, s);
          }
          return prev.map(state => {
            if (state.parameter.name === param.name) {
              return { ...state, value };
            }
            const next = downstreamByName.get(state.parameter.name);
            if (!next) return state;
            const preservedValue = next.value !== undefined
              ? next.value
              : next.options_error
              ? state.value
              : next.options[0]?.value;
            return {
              ...next,
              value: preservedValue,
              // If the backend cleared options because of an error, hold
              // onto the previous options so the user can still see what
              // was selectable before — matches W34's "preserve previous
              // selection when a refresh temporarily fails" goal.
              options: next.options_error && next.options.length === 0 ? state.options : next.options,
            };
          });
        });
        onAffectedWidgets(result.affected_widget_ids);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to set parameter');
      } finally {
        setPendingName(null);
      }
    },
    [dashboardId, onAffectedWidgets],
  );

  // Apply caller-provided initial selections (e.g. URL hash) once the
  // first listParameters response lands. Only commit values that differ
  // from what the backend already persisted, so we don't churn the DB on
  // every navigation back to a dashboard.
  useEffect(() => {
    if (appliedInitial) return;
    if (loading || states.length === 0) return;
    if (!initialSelections) {
      setAppliedInitial(true);
      return;
    }
    const persisted: Record<string, ParameterValue> = {};
    for (const state of states) {
      if (state.value !== undefined) persisted[state.parameter.name] = state.value;
    }
    const pending: Array<{ param: DashboardParameter; value: ParameterValue }> = [];
    for (const state of states) {
      const incoming = initialSelections[state.parameter.name];
      if (incoming === undefined) continue;
      if (JSON.stringify(incoming) === JSON.stringify(persisted[state.parameter.name])) {
        continue;
      }
      pending.push({ param: state.parameter, value: incoming });
    }
    setAppliedInitial(true);
    void (async () => {
      for (const { param, value } of pending) {
        await commitValue(param, value);
      }
    })();
  }, [appliedInitial, loading, states, initialSelections, commitValue]);

  const refreshOptions = useCallback(
    async (paramName: string) => {
      setRefreshingName(paramName);
      try {
        const next = await dashboardApi.refreshParameterOptions(dashboardId, paramName);
        setStates(prev =>
          prev.map(state =>
            state.parameter.name === paramName
              ? {
                  ...next,
                  // Keep the user's current selection visible even if the
                  // server reports a fresh list — the dropdown will snap
                  // to the matching option on render.
                  value: state.value ?? next.value,
                  options: next.options.length === 0 && next.options_error ? state.options : next.options,
                }
              : state,
          ),
        );
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to refresh options');
      } finally {
        setRefreshingName(null);
      }
    },
    [dashboardId],
  );

  const visibleStates = useMemo(
    () => states.filter(state => parameters.some(p => p.name === state.parameter.name)),
    [states, parameters],
  );

  if (parameters.length === 0) return null;

  return (
    <div className="sticky top-0 z-10 mb-2 flex flex-wrap items-center gap-3 rounded-md border border-border bg-card/90 px-3 py-2 shadow-sm backdrop-blur">
      <span className="hidden md:inline mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground">// vars</span>
      {loading ? (
        <span className="text-xs text-muted-foreground">Loading parameters…</span>
      ) : (
        visibleStates.map(state => (
          <ParameterControl
            key={state.parameter.id}
            state={state}
            disabled={pendingName === state.parameter.name}
            refreshing={refreshingName === state.parameter.name}
            onCommit={value => commitValue(state.parameter, value)}
            onRefresh={isQueryKind(state.parameter.kind) ? () => refreshOptions(state.parameter.name) : undefined}
          />
        ))
      )}
      {error && <span className="text-xs text-destructive">{error}</span>}
    </div>
  );
}

interface ControlProps {
  state: DashboardParameterState;
  disabled: boolean;
  refreshing: boolean;
  onCommit: (value: ParameterValue) => void;
  onRefresh?: () => void;
}

function ParameterControl({ state, disabled, refreshing, onCommit, onRefresh }: ControlProps) {
  const { parameter, value, options, options_error } = state;
  const label = parameter.label || parameter.name;
  const tooltip = parameter.description ?? parameter.name;

  return (
    <label className="flex items-center gap-2 text-xs" title={tooltip}>
      <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">${label}</span>
      {renderInput(parameter, value, options, disabled, onCommit)}
      {refreshing && <span className="mono text-[10px] text-muted-foreground">…</span>}
      {options_error && (
        <span
          className="mono text-[10px] text-destructive max-w-[160px] truncate"
          title={options_error}
        >
          ! {options_error}
        </span>
      )}
      {onRefresh && (
        <button
          type="button"
          onClick={onRefresh}
          disabled={refreshing || disabled}
          className="mono text-[10px] text-muted-foreground hover:text-primary disabled:opacity-50"
          title="Re-resolve options from backend"
        >
          ↻
        </button>
      )}
    </label>
  );
}

// W46: cap intrinsic select/input width so a single long option label
// (e.g. a URL or filepath) cannot blow out the sticky parameter row.
const PARAM_INPUT_CLASS = 'max-w-[16rem] truncate rounded-md border border-border bg-muted/40 px-2 py-1 text-xs tabular focus:border-primary/60';

function renderInput(
  parameter: DashboardParameter,
  value: ParameterValue | undefined,
  options: ParameterOption[],
  disabled: boolean,
  onCommit: (value: ParameterValue) => void,
) {
  if (parameter.kind === 'static_list' || isQueryKind(parameter.kind)) {
    // Static lists ship their own options on the parameter declaration;
    // query-backed kinds get them via DashboardParameterState.options.
    const optionList: ParameterOption[] = parameter.kind === 'static_list'
      ? parameter.options
      : options;

    if (optionList.length === 0) {
      return (
        <span className="mono text-[10px] text-muted-foreground italic">
          {isQueryKind(parameter.kind) ? 'no options' : 'empty'}
        </span>
      );
    }
    if (parameter.multi) {
      const currentValues = Array.isArray(value)
        ? value.map(valueToString)
        : value !== undefined
        ? [valueToString(value)]
        : [];
      return (
        <select
          multiple
          disabled={disabled}
          value={currentValues}
          onChange={e => {
            const selected = Array.from(e.target.selectedOptions).map(o => o.value);
            const mapped: ParameterValue[] = selected
              .map(s => optionList.find(opt => valueToString(opt.value) === s)?.value)
              .filter((v): v is ParameterValue => v !== undefined);
            onCommit(mapped);
          }}
          className={`min-w-[120px] ${PARAM_INPUT_CLASS}`}
        >
          {optionList.map(opt => (
            <option key={valueToString(opt.value)} value={valueToString(opt.value)}>
              {opt.label}
            </option>
          ))}
        </select>
      );
    }
    const current = valueToString(value);
    // If the persisted value is not in the current option list (e.g. the
    // backend returned a fresh page that no longer includes it), render
    // a synthetic "preserved" option so the user still sees what was
    // previously selected instead of a confusing first-option auto-pick.
    const hasCurrent = optionList.some(opt => valueToString(opt.value) === current);
    return (
      <select
        disabled={disabled}
        value={current}
        onChange={e => {
          const match = optionList.find(opt => valueToString(opt.value) === e.target.value);
          if (match) onCommit(match.value);
        }}
        className={`min-w-[100px] ${PARAM_INPUT_CLASS}`}
      >
        {!hasCurrent && current !== '' && (
          <option value={current}>{current} (stale)</option>
        )}
        {optionList.map(opt => (
          <option key={valueToString(opt.value)} value={valueToString(opt.value)}>
            {opt.label}
          </option>
        ))}
      </select>
    );
  }

  if (parameter.kind === 'text_input') {
    return (
      <TextInputControl
        placeholder={parameter.placeholder}
        value={typeof value === 'string' ? value : ''}
        disabled={disabled}
        onCommit={onCommit}
      />
    );
  }

  if (parameter.kind === 'interval') {
    const presets = parameter.presets.length > 0 ? parameter.presets : INTERVAL_PRESETS;
    return (
      <select
        disabled={disabled}
        value={typeof value === 'string' ? value : presets[0] ?? ''}
        onChange={e => onCommit(e.target.value)}
        className={PARAM_INPUT_CLASS}
      >
        {presets.map(preset => (
          <option key={preset} value={preset}>
            {preset}
          </option>
        ))}
      </select>
    );
  }

  if (parameter.kind === 'time_range') {
    const current =
      value &&
      typeof value === 'object' &&
      !Array.isArray(value) &&
      'from' in value &&
      'to' in value
        ? value
        : { from: Date.now() - 60 * 60 * 1000, to: Date.now() };
    return (
      <select
        disabled={disabled}
        value={String(current.to - current.from)}
        onChange={e => {
          const ms = Number(e.target.value);
          const to = Date.now();
          onCommit({ from: to - ms, to });
        }}
        className={PARAM_INPUT_CLASS}
      >
        {TIME_RANGE_PRESETS.map(preset => (
          <option key={preset.label} value={String(preset.ms)}>
            {preset.label}
          </option>
        ))}
      </select>
    );
  }

  if (parameter.kind === 'constant') {
    return <span className="rounded-md border border-dashed border-border bg-muted/30 px-2 py-1 text-xs mono text-muted-foreground">
      {valueToString(value ?? parameter.value)}
    </span>;
  }

  return (
    <TextInputControl
      placeholder={`set ${parameter.name}`}
      value={typeof value === 'string' ? value : valueToString(value)}
      disabled={disabled}
      onCommit={onCommit}
    />
  );
}

interface TextInputControlProps {
  placeholder?: string;
  value: string;
  disabled: boolean;
  onCommit: (value: ParameterValue) => void;
}

function TextInputControl({ placeholder, value, disabled, onCommit }: TextInputControlProps) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);

  return (
    <input
      type="text"
      placeholder={placeholder}
      value={draft}
      disabled={disabled}
      onChange={e => setDraft(e.target.value)}
      onBlur={() => {
        if (draft !== value) onCommit(draft);
      }}
      onKeyDown={e => {
        if (e.key === 'Enter') {
          e.preventDefault();
          if (draft !== value) onCommit(draft);
        } else if (e.key === 'Escape') {
          setDraft(value);
        }
      }}
      className={PARAM_INPUT_CLASS}
    />
  );
}
