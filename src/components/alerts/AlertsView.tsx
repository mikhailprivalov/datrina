import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import type { AlertEvent, AlertSeverity, Dashboard } from '../../lib/api';
import { ALERT_EVENT_CHANNEL, alertApi } from '../../lib/api';

interface Props {
  dashboards: Dashboard[];
  onJumpToWidget: (dashboardId: string, widgetId: string) => void;
  onClose: () => void;
}

const SEVERITY_RANK: Record<AlertSeverity, number> = {
  critical: 0,
  warning: 1,
  info: 2,
};

const SEVERITY_TONE: Record<AlertSeverity, string> = {
  critical: 'bg-destructive/15 text-destructive border-destructive/30',
  warning: 'bg-neon-amber/15 text-neon-amber border-neon-amber/30',
  info: 'bg-primary/15 text-primary border-primary/30',
};

export function AlertsView({ dashboards, onJumpToWidget, onClose }: Props) {
  const [events, setEvents] = useState<AlertEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  const reload = async () => {
    try {
      setError(null);
      const data = await alertApi.listEvents(!showAll, 300);
      setEvents(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load alerts');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showAll]);

  useEffect(() => {
    const unsubscribe = listen<AlertEvent>(ALERT_EVENT_CHANNEL, evt => {
      setEvents(prev => [evt.payload, ...prev]);
    });
    return () => {
      unsubscribe.then(dispose => dispose()).catch(() => {});
    };
  }, []);

  const grouped = useMemo(() => groupByWidget(events), [events]);
  const dashboardName = (id: string) =>
    dashboards.find(d => d.id === id)?.name ?? '(deleted dashboard)';
  const widgetTitle = (dashboardId: string, widgetId: string) =>
    dashboards
      .find(d => d.id === dashboardId)
      ?.layout.find(w => w.id === widgetId)?.title ?? widgetId;

  const handleAck = async (id: string) => {
    try {
      await alertApi.acknowledge(id);
      setEvents(prev =>
        prev.map(e => (e.id === id ? { ...e, acknowledged_at: Date.now() } : e))
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to acknowledge');
    }
  };

  return (
    <div className="flex h-full flex-col bg-background">
      <div className="flex items-center justify-between border-b border-border px-4 py-3 bg-muted/20">
        <div>
          <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// alerts</p>
          <h2 className="mt-0.5 text-sm font-semibold tracking-tight">Alerts</h2>
          <p className="text-xs text-muted-foreground">
            {showAll ? 'All recent events' : 'Unacknowledged events only'}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowAll(v => !v)}
            className="rounded-md border border-border bg-card px-3 py-1.5 text-xs mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
          >
            {showAll ? 'Show unack' : 'Show all'}
          </button>
          <button
            onClick={onClose}
            className="rounded-md border border-border bg-card px-3 py-1.5 text-xs mono uppercase tracking-wider hover:bg-muted transition-colors"
          >
            Close
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-auto p-4 scrollbar-thin">
        {error && (
          <div className="mb-3 rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
            {error}
          </div>
        )}
        {loading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : grouped.length === 0 ? (
          <p className="text-sm text-muted-foreground">No alerts {showAll ? 'recorded' : 'unacknowledged'}.</p>
        ) : (
          <ul className="space-y-4">
            {grouped.map(group => (
              <li
                key={group.widgetId}
                className="rounded-md border border-border bg-card"
              >
                <div className="flex items-center justify-between border-b border-border/60 px-3 py-2 bg-muted/20">
                  <div className="min-w-0">
                    <p className="text-sm font-medium truncate">
                      {widgetTitle(group.dashboardId, group.widgetId)}
                    </p>
                    <p className="text-[11px] text-muted-foreground truncate">
                      {dashboardName(group.dashboardId)}
                    </p>
                  </div>
                  <button
                    onClick={() => onJumpToWidget(group.dashboardId, group.widgetId)}
                    className="rounded-md border border-border bg-card px-2 py-1 text-[11px] mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
                  >
                    Jump
                  </button>
                </div>
                <ul className="divide-y divide-border/60">
                  {group.events.map(event => (
                    <li
                      key={event.id}
                      className="flex items-start justify-between gap-3 px-3 py-2"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2 text-[11px]">
                          <span
                            className={`rounded-sm border px-1.5 py-0.5 mono text-[10px] font-semibold uppercase tracking-wider ${SEVERITY_TONE[event.severity]}`}
                          >
                            {event.severity}
                          </span>
                          <span className="text-muted-foreground">
                            {new Date(event.fired_at).toLocaleString()}
                          </span>
                          {event.triggered_session_id && (
                            <span className="rounded-sm border border-border bg-muted/60 px-1.5 py-0.5 text-[9px] mono font-semibold uppercase tracking-wider text-muted-foreground">
                              agent run
                            </span>
                          )}
                        </div>
                        <p className="mt-1 text-sm">{event.message}</p>
                      </div>
                      {!event.acknowledged_at && (
                        <button
                          onClick={() => handleAck(event.id)}
                          className="shrink-0 rounded-md border border-border bg-card px-2 py-1 text-[11px] mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
                        >
                          Ack
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function groupByWidget(events: AlertEvent[]) {
  const byWidget = new Map<string, { dashboardId: string; widgetId: string; events: AlertEvent[]; topSeverity: AlertSeverity; latest: number }>();
  for (const event of events) {
    const key = `${event.dashboard_id}::${event.widget_id}`;
    const existing = byWidget.get(key);
    if (existing) {
      existing.events.push(event);
      if (SEVERITY_RANK[event.severity] < SEVERITY_RANK[existing.topSeverity]) {
        existing.topSeverity = event.severity;
      }
      if (event.fired_at > existing.latest) {
        existing.latest = event.fired_at;
      }
    } else {
      byWidget.set(key, {
        dashboardId: event.dashboard_id,
        widgetId: event.widget_id,
        events: [event],
        topSeverity: event.severity,
        latest: event.fired_at,
      });
    }
  }
  const groups = Array.from(byWidget.values());
  groups.sort((a, b) => {
    const sev = SEVERITY_RANK[a.topSeverity] - SEVERITY_RANK[b.topSeverity];
    if (sev !== 0) return sev;
    return b.latest - a.latest;
  });
  return groups;
}
