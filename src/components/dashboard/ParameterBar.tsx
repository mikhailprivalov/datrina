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

export function ParameterBar({ dashboardId, parameters, onAffectedWidgets }: Props) {
  const [states, setStates] = useState<DashboardParameterState[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingName, setPendingName] = useState<string | null>(null);

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

  const commitValue = useCallback(
    async (param: DashboardParameter, value: ParameterValue) => {
      setPendingName(param.name);
      setError(null);
      try {
        const result = await dashboardApi.setParameterValue(dashboardId, param.name, value);
        setStates(prev =>
          prev.map(state =>
            state.parameter.name === param.name ? { ...state, value } : state,
          ),
        );
        onAffectedWidgets(result.affected_widget_ids);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to set parameter');
      } finally {
        setPendingName(null);
      }
    },
    [dashboardId, onAffectedWidgets],
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
            onCommit={value => commitValue(state.parameter, value)}
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
  onCommit: (value: ParameterValue) => void;
}

function ParameterControl({ state, disabled, onCommit }: ControlProps) {
  const { parameter, value, options } = state;
  const label = parameter.label || parameter.name;
  const tooltip = parameter.description ?? parameter.name;

  return (
    <label className="flex items-center gap-2 text-xs" title={tooltip}>
      <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">${label}</span>
      {renderInput(parameter, value, options, disabled, onCommit)}
    </label>
  );
}

const PARAM_INPUT_CLASS = 'rounded-md border border-border bg-muted/40 px-2 py-1 text-xs tabular focus:border-primary/60';

function renderInput(
  parameter: DashboardParameter,
  value: ParameterValue | undefined,
  options: ParameterOption[],
  disabled: boolean,
  onCommit: (value: ParameterValue) => void,
) {
  if (parameter.kind === 'static_list') {
    if (parameter.multi) {
      const currentValues = Array.isArray(value)
        ? value.map(valueToString)
        : value !== undefined
        ? [valueToString(value)]
        : [];
      const opts = parameter.options;
      return (
        <select
          multiple
          disabled={disabled}
          value={currentValues}
          onChange={e => {
            const selected = Array.from(e.target.selectedOptions).map(o => o.value);
            const mapped: ParameterValue[] = selected
              .map(s => opts.find(opt => valueToString(opt.value) === s)?.value)
              .filter((v): v is ParameterValue => v !== undefined);
            onCommit(mapped);
          }}
          className={`min-w-[120px] ${PARAM_INPUT_CLASS}`}
        >
          {opts.map(opt => (
            <option key={valueToString(opt.value)} value={valueToString(opt.value)}>
              {opt.label}
            </option>
          ))}
        </select>
      );
    }
    const current = valueToString(value);
    return (
      <select
        disabled={disabled}
        value={current}
        onChange={e => {
          const match = parameter.options.find(opt => valueToString(opt.value) === e.target.value);
          if (match) onCommit(match.value);
        }}
        className={`min-w-[100px] ${PARAM_INPUT_CLASS}`}
      >
        {options.map(opt => (
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

  // mcp_query / http_query: option resolution lives server-side; v1 only
  // shows the persisted selection as a plain text input fallback.
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
