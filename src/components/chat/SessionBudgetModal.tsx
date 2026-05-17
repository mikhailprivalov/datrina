import { useState } from 'react';
import { costApi } from '../../lib/api';
import type { ChatSession } from '../../lib/api';

interface Props {
  sessionId: string;
  currentMaxCostUsd: number | null;
  currentSpentUsd: number;
  onClose: () => void;
  onSaved: (updated: ChatSession) => void;
}

export function SessionBudgetModal({
  sessionId,
  currentMaxCostUsd,
  currentSpentUsd,
  onClose,
  onSaved,
}: Props) {
  const [value, setValue] = useState(
    currentMaxCostUsd != null ? currentMaxCostUsd.toFixed(2) : '',
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async (clear: boolean) => {
    setError(null);
    setPending(true);
    try {
      const parsed = clear ? null : parseFloat(value);
      if (!clear) {
        if (!Number.isFinite(parsed!) || parsed! < 0) {
          setError('Enter a non-negative number, or click "Remove cap"');
          setPending(false);
          return;
        }
        if (parsed! < currentSpentUsd) {
          setError(
            `Cap below current spend ($${currentSpentUsd.toFixed(4)}) — request would be denied immediately.`,
          );
        }
      }
      const updated = await costApi.setSessionBudget(sessionId, parsed);
      onSaved(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 backdrop-blur-sm">
      <div className="w-[28rem] rounded-md bg-card border border-border shadow-2xl p-5">
        <div className="flex items-start justify-between mb-3">
          <div>
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// budget</p>
            <h3 className="mt-0.5 text-sm font-semibold tracking-tight">Session cost cap</h3>
            <p className="text-[11px] text-muted-foreground mt-1">
              Hard cap in USD. The next provider request is denied once
              this session's running total reaches the cap.
            </p>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-muted text-muted-foreground"
            aria-label="Close"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="space-y-3">
          <div>
            <label className="block text-[11px] font-medium text-muted-foreground mb-1">
              Cap (USD)
            </label>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">$</span>
              <input
                type="number"
                step="0.01"
                min="0"
                value={value}
                onChange={event => setValue(event.target.value)}
                placeholder="e.g. 0.50"
                className="flex-1 rounded-md border border-border bg-background px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
              />
            </div>
          </div>
          <p className="text-[11px] text-muted-foreground">
            Spent so far in this session: <span className="font-medium text-foreground">${currentSpentUsd.toFixed(4)}</span>
          </p>
          {error && <p className="text-[11px] text-destructive">{error}</p>}
        </div>
        <div className="flex items-center justify-end gap-2 mt-4">
          {currentMaxCostUsd != null && (
            <button
              onClick={() => handleSave(true)}
              disabled={pending}
              className="text-[12px] text-muted-foreground hover:text-foreground px-2 py-1.5 rounded-md hover:bg-muted disabled:opacity-50"
            >
              Remove cap
            </button>
          )}
          <button
            onClick={onClose}
            className="text-[12px] px-3 py-1.5 rounded-md border border-border hover:bg-muted"
          >
            Cancel
          </button>
          <button
            onClick={() => handleSave(false)}
            disabled={pending}
            className="text-[12px] mono uppercase tracking-wider font-semibold px-3 py-1.5 rounded-md bg-primary text-primary-foreground border border-primary hover:glow-primary disabled:opacity-50 transition-all"
          >
            {pending ? 'Saving…' : 'Save cap'}
          </button>
        </div>
      </div>
    </div>
  );
}
