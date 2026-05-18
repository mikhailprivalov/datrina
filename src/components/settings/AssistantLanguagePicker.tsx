import { useEffect, useMemo, useState } from 'react';
import {
  type AssistantLanguageOption,
  type AssistantLanguagePolicy,
  languageApi,
} from '../../lib/api';

interface Props {
  /** Current resolved policy for this scope. `null` is rendered as
   *  "auto" so the dropdown always has a selected value. */
  value: AssistantLanguagePolicy | null;
  /** Called after a successful change. `null` clears overrides (only
   *  meaningful for dashboard/session scope — at the app level the
   *  effective value drops back to "auto"). */
  onChange: (next: AssistantLanguagePolicy | null) => void | Promise<void>;
  /** Scope-aware label shown above the picker. */
  label: string;
  /** Optional caption shown under the picker. */
  hint?: string;
  /** When true the picker shows an explicit "Clear override (inherit)"
   *  option so dashboard / session callers can fall back to a wider
   *  scope. App-level callers omit this; clearing the app default just
   *  means switching to `Auto`. */
  allowInherit?: boolean;
  /** When true, the change handler is invoked but the picker keeps
   *  rendering the previous value until the parent passes a new one
   *  back. Used when the parent persists the change and re-renders. */
  pendingControlled?: boolean;
  disabled?: boolean;
}

type Choice =
  | { kind: 'inherit' }
  | { kind: 'auto' }
  | { kind: 'explicit'; tag: string };

const INHERIT_VALUE = '__inherit__';
const AUTO_VALUE = '__auto__';

function policyToChoice(
  value: AssistantLanguagePolicy | null,
  allowInherit: boolean,
): Choice {
  if (value === null || value === undefined) {
    return allowInherit ? { kind: 'inherit' } : { kind: 'auto' };
  }
  if (value.mode === 'auto') return { kind: 'auto' };
  return { kind: 'explicit', tag: value.tag };
}

function choiceToSelectValue(choice: Choice): string {
  switch (choice.kind) {
    case 'inherit':
      return INHERIT_VALUE;
    case 'auto':
      return AUTO_VALUE;
    case 'explicit':
      return choice.tag;
  }
}

function selectValueToPolicy(
  raw: string,
  allowInherit: boolean,
): AssistantLanguagePolicy | null {
  if (raw === INHERIT_VALUE) {
    if (!allowInherit) return { mode: 'auto' };
    return null;
  }
  if (raw === AUTO_VALUE) return { mode: 'auto' };
  return { mode: 'explicit', tag: raw };
}

export function AssistantLanguagePicker({
  value,
  onChange,
  label,
  hint,
  allowInherit = false,
  pendingControlled = false,
  disabled = false,
}: Props) {
  const [options, setOptions] = useState<AssistantLanguageOption[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  useEffect(() => {
    let cancelled = false;
    languageApi
      .list()
      .then(catalog => {
        if (!cancelled) {
          setOptions(catalog.options);
          setLoadError(null);
        }
      })
      .catch(err => {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const selected = useMemo(
    () => choiceToSelectValue(policyToChoice(value, allowInherit)),
    [value, allowInherit],
  );

  const handleSelect = async (event: React.ChangeEvent<HTMLSelectElement>) => {
    const next = selectValueToPolicy(event.target.value, allowInherit);
    if (!pendingControlled) {
      setPending(true);
    }
    try {
      await onChange(next);
    } finally {
      if (!pendingControlled) {
        setPending(false);
      }
    }
  };

  const isDisabled = disabled || pending || options === null;

  return (
    <div className="space-y-1.5">
      <label className="mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
        {label}
      </label>
      <select
        value={selected}
        onChange={handleSelect}
        disabled={isDisabled}
        className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs text-foreground transition-colors hover:border-primary/40 focus:border-primary focus:outline-none disabled:opacity-60"
      >
        {allowInherit && (
          <option value={INHERIT_VALUE}>Inherit (use wider default)</option>
        )}
        <option value={AUTO_VALUE}>Auto (follow user prompt)</option>
        {options?.map(option => (
          <option key={option.tag} value={option.tag}>
            {option.label} · {option.native_label} ({option.tag})
          </option>
        ))}
      </select>
      {hint && <p className="text-[11px] text-muted-foreground">{hint}</p>}
      {loadError && (
        <p className="text-[11px] text-destructive">
          Language catalog failed to load: {loadError}
        </p>
      )}
    </div>
  );
}
