import { useCallback, useEffect, useMemo, useState } from 'react';
import { costApi } from '../../lib/api';
import type { CostSummary, ModelPricingOverride } from '../../lib/api';

interface Props {
  onClose: () => void;
}

export function CostsView({ onClose }: Props) {
  const [summary, setSummary] = useState<CostSummary | null>(null);
  const [overrides, setOverrides] = useState<ModelPricingOverride[]>([]);
  const [overrideDraft, setOverrideDraft] = useState<string>('');
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const next = await costApi.getSummary(30);
      setSummary(next);
      const fresh = await costApi.getPricingOverrides();
      setOverrides(fresh);
      setOverrideDraft(JSON.stringify(fresh, null, 2));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handleSaveOverrides = async () => {
    setError(null);
    setStatus(null);
    setPending(true);
    try {
      const parsed = JSON.parse(overrideDraft || '[]');
      if (!Array.isArray(parsed)) {
        throw new Error('Pricing overrides must be a JSON array.');
      }
      const saved = await costApi.setPricingOverrides(parsed as ModelPricingOverride[]);
      setOverrides(saved);
      setOverrideDraft(JSON.stringify(saved, null, 2));
      setStatus('Saved');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPending(false);
    }
  };

  const maxBucketCost = useMemo(() => {
    if (!summary?.last_30_days?.length) return 0;
    return summary.last_30_days.reduce((acc, b) => Math.max(acc, b.cost_usd), 0);
  }, [summary]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex max-h-[88vh] w-[min(92vw,64rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-3">
          <div>
            <h2 className="text-sm font-semibold text-foreground">Provider costs</h2>
            <p className="text-[11px] text-muted-foreground">
              Token + dollar spend across all chat sessions. Editable per-model
              pricing overrides live in <code className="font-mono">pricing_overrides.json</code>.
            </p>
          </div>
          <button onClick={onClose} className="p-1 rounded hover:bg-muted text-muted-foreground" aria-label="Close">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {(status || error) && (
          <div className={`border-b border-border px-5 py-2 text-xs ${error ? 'text-destructive' : 'text-emerald-600 dark:text-emerald-400'}`}>
            {error ?? status}
          </div>
        )}

        <div className="flex-1 overflow-auto p-5 space-y-5">
          <section className="rounded-lg border border-border bg-background/50 p-3">
            <div className="flex items-baseline justify-between mb-2">
              <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Last 30 days
              </h3>
              <span className="text-xs text-muted-foreground">
                today: <span className="text-foreground font-medium">${summary?.today_cost_usd.toFixed(4) ?? '0.0000'}</span>
              </span>
            </div>
            {summary && summary.last_30_days.length > 0 ? (
              <div className="flex items-end gap-1 h-32">
                {summary.last_30_days.map(bucket => {
                  const height = maxBucketCost > 0
                    ? Math.max(2, Math.round((bucket.cost_usd / maxBucketCost) * 100))
                    : 2;
                  const date = new Date(bucket.day_start_ms);
                  return (
                    <div
                      key={bucket.day_start_ms}
                      className="flex-1 flex flex-col items-center justify-end gap-1"
                      title={`${date.toLocaleDateString()} · $${bucket.cost_usd.toFixed(4)}`}
                    >
                      <div
                        className="w-full bg-primary/70 rounded-sm"
                        style={{ height: `${height}%` }}
                      />
                      <span className="text-[8px] text-muted-foreground/70">
                        {date.getUTCDate()}
                      </span>
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">No spend recorded yet.</p>
            )}
          </section>

          <section className="rounded-lg border border-border bg-background/50">
            <div className="border-b border-border/60 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              Top sessions
            </div>
            {summary?.top_sessions.length ? (
              <ul className="divide-y divide-border/40">
                {summary.top_sessions.map(entry => (
                  <li key={entry.session_id} className="flex items-center justify-between px-3 py-2 text-xs">
                    <div className="min-w-0 flex-1">
                      <span className={`mr-2 inline-block rounded px-1.5 py-0.5 text-[9px] uppercase tracking-wide ${entry.mode === 'build' ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400' : 'bg-blue-500/10 text-blue-600 dark:text-blue-400'}`}>
                        {entry.mode}
                      </span>
                      <span className="truncate text-foreground">{entry.title}</span>
                      <span className="ml-2 text-muted-foreground">
                        {entry.input_tokens + entry.output_tokens + entry.reasoning_tokens} tokens
                      </span>
                    </div>
                    <span className="font-mono text-foreground">${entry.cost_usd.toFixed(4)}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="px-3 py-2 text-xs text-muted-foreground">No session has spent anything yet.</p>
            )}
          </section>

          <section className="rounded-lg border border-border bg-background/50">
            <div className="border-b border-border/60 px-3 py-2 flex items-center justify-between">
              <div>
                <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Pricing overrides</h3>
                <p className="text-[10px] text-muted-foreground mt-0.5">
                  Edit the rates Datrina uses to compute cost. Pattern is a case-insensitive substring on model id.
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button
                  onClick={refresh}
                  className="rounded-md border border-border px-2 py-1 text-[11px] hover:bg-muted"
                >
                  Reload
                </button>
                <button
                  onClick={handleSaveOverrides}
                  disabled={pending}
                  className="rounded-md bg-primary text-primary-foreground px-3 py-1 text-[11px] hover:bg-primary/90 disabled:opacity-50"
                >
                  {pending ? 'Saving…' : 'Save'}
                </button>
              </div>
            </div>
            <textarea
              value={overrideDraft}
              onChange={event => setOverrideDraft(event.target.value)}
              spellCheck={false}
              className="w-full h-44 resize-y bg-background px-3 py-2 text-[11px] font-mono focus:outline-none"
              placeholder={`[\n  {\n    "model_pattern": "kimi-k2",\n    "input_usd_per_1m": 0.6,\n    "output_usd_per_1m": 2.5\n  }\n]`}
            />
            {overrides.length === 0 && (
              <p className="border-t border-border/60 px-3 py-2 text-[10px] text-muted-foreground">
                Empty — Datrina falls back to its built-in pricing table for every model.
              </p>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
