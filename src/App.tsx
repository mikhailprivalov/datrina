import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Sidebar } from './components/layout/Sidebar';
import { DashboardGrid } from './components/layout/DashboardGrid';
import { ChatPanel } from './components/layout/ChatPanel';
import { TopBar } from './components/layout/TopBar';
import { StatusBar } from './components/layout/StatusBar';
import { ProviderSettings } from './components/layout/ProviderSettings';
import { McpSettings } from './components/layout/McpSettings';
import { MemorySettings } from './components/layout/MemorySettings';
import { CostsView } from './components/layout/CostsView';
import { DatrinaLogo } from './components/branding/DatrinaLogo';
import { HistoryDrawer } from './components/dashboard/HistoryDrawer';
import { Playground } from './components/playground/Playground';
import { TemplateGallery } from './components/onboarding/TemplateGallery';
import { AlertsView } from './components/alerts/AlertsView';
import { AlertEditorModal } from './components/alerts/AlertEditorModal';
import { Workbench } from './components/datasource/Workbench';
import { SourceCatalog } from './components/sources/SourceCatalog';
import { OperationsView } from './components/operations/OperationsView';
import type { WidgetAlertStatus } from './components/layout/DashboardGrid';
import type { DashboardTemplate } from './lib/templates';
import { ALERT_EVENT_CHANNEL, WIDGET_STREAM_EVENT_CHANNEL, alertApi, configApi, dashboardApi, providerApi } from './lib/api';
import type { AlertEvent, AlertSeverity, BuildProposal, CreateProviderRequest, Dashboard, DashboardWidgetRefreshResult, LLMProvider, UpdateProviderRequest, Widget, WidgetRuntimeData, WidgetStreamEnvelope, WidgetStreamState, WorkflowEventEnvelope, WorkflowRun } from './lib/api';

function App() {
  const [dashboards, setDashboards] = useState<Dashboard[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [isChatOpen, setIsChatOpen] = useState(false);
  const [chatMode, setChatMode] = useState<'build' | 'context'>('context');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    if (typeof window === 'undefined') return 'light';
    const stored = window.localStorage.getItem('datrina:theme');
    if (stored === 'dark' || stored === 'light') return stored;
    return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  });

  useEffect(() => {
    const root = window.document.documentElement;
    if (theme === 'dark') {
      root.classList.add('dark');
      root.classList.remove('is-light');
    } else {
      root.classList.remove('dark');
      root.classList.add('is-light');
    }
    window.localStorage.setItem('datrina:theme', theme);
  }, [theme]);
  const [isReady, setIsReady] = useState(false);
  const [isProvidersReady, setIsProvidersReady] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [statusMessage, setStatusMessage] = useState('Ready');
  const [error, setError] = useState<string | null>(null);
  const [widgetData, setWidgetData] = useState<Record<string, WidgetRuntimeData | undefined>>({});
  // W36: per-widget snapshot timestamp. Non-null means the data shown
  // came from the last-known-good cache and a live refresh has not
  // produced fresh data yet. Cleared on every successful refresh.
  const [widgetCachedAt, setWidgetCachedAt] = useState<Record<string, number | undefined>>({});
  // W42: per-widget live streaming state. Populated by `widget:stream`
  // events; the final widget runtime value is still owned by
  // `widgetData` and only committed after the server acknowledges the
  // refresh succeeded.
  const [widgetStream, setWidgetStream] = useState<Record<string, WidgetStreamState | undefined>>({});
  const [widgetErrors, setWidgetErrors] = useState<Record<string, string | undefined>>({});
  const [workflowRuns, setWorkflowRuns] = useState<Record<string, WorkflowRun | undefined>>({});
  const [refreshingWidgetId, setRefreshingWidgetId] = useState<string | null>(null);
  const [providers, setProviders] = useState<LLMProvider[]>([]);
  const [activeProviderId, setActiveProviderId] = useState<string | null>(null);
  const [isProviderSettingsOpen, setIsProviderSettingsOpen] = useState(false);
  const [isMcpSettingsOpen, setIsMcpSettingsOpen] = useState(false);
  const [isMemorySettingsOpen, setIsMemorySettingsOpen] = useState(false);
  const [isCostsViewOpen, setIsCostsViewOpen] = useState(false);
  const [isHistoryOpen, setIsHistoryOpen] = useState(false);
  const [undoToast, setUndoToast] = useState<{ versionId: string; label: string } | null>(null);
  const [route, setRoute] = useState<'dashboards' | 'playground' | 'alerts' | 'workbench' | 'sources' | 'operations'>(() => {
    if (typeof window === 'undefined') return 'dashboards';
    if (window.location.hash === '#/playground') return 'playground';
    if (window.location.hash === '#/alerts') return 'alerts';
    if (window.location.hash === '#/workbench') return 'workbench';
    if (window.location.hash === '#/sources') return 'sources';
    if (window.location.hash.startsWith('#/operations')) return 'operations';
    return 'dashboards';
  });
  const [alertEvents, setAlertEvents] = useState<AlertEvent[]>([]);
  const [unacknowledgedAlertCount, setUnacknowledgedAlertCount] = useState(0);
  const [alertEditorWidgetId, setAlertEditorWidgetId] = useState<string | null>(null);
  const [isTemplateGalleryOpen, setIsTemplateGalleryOpen] = useState(false);
  const [pendingBuildPrompt, setPendingBuildPrompt] = useState<string | null>(null);
  // W28: bumping this forces ChatPanel to drop any reused session and
  // open a fresh draft. Each top-bar Build click, template launch, and
  // Playground "Use as widget" mints a new value.
  const [freshChatSessionKey, setFreshChatSessionKey] = useState(0);

  // W40: monotonic per-widget supersede counter. Every refresh issues a
  // ticket; a refresh result is only applied when its ticket is still
  // the latest one for that widget. This prevents a slow shared
  // workflow from clobbering a fresher run that landed in the
  // meantime (e.g. user clicks Refresh on one widget, then a batch
  // refresh from a parameter change completes after).
  const refreshTickets = useRef<Map<string, number>>(new Map());
  const issueRefreshTicket = useCallback((widgetIds: string[]) => {
    const tickets = new Map<string, number>();
    for (const id of widgetIds) {
      const next = (refreshTickets.current.get(id) ?? 0) + 1;
      refreshTickets.current.set(id, next);
      tickets.set(id, next);
    }
    return tickets;
  }, []);
  const isLatestTicket = useCallback(
    (widgetId: string, ticket: number) => refreshTickets.current.get(widgetId) === ticket,
    [],
  );

  const applyBatchedRefreshResults = useCallback(
    (results: DashboardWidgetRefreshResult[], tickets: Map<string, number>) => {
      const dataPatch: Record<string, WidgetRuntimeData> = {};
      const dataReset: string[] = [];
      const errorPatch: Record<string, string | undefined> = {};
      const cachedReset: string[] = [];
      for (const result of results) {
        const ticket = tickets.get(result.widget_id);
        if (ticket === undefined || !isLatestTicket(result.widget_id, ticket)) continue;
        if (result.data) {
          dataPatch[result.widget_id] = result.data;
          errorPatch[result.widget_id] = undefined;
          cachedReset.push(result.widget_id);
        } else if (result.error) {
          errorPatch[result.widget_id] = result.error;
        } else {
          errorPatch[result.widget_id] = `Widget refresh returned no runtime data: ${result.status}`;
        }
        // success path also drops the row from widgetData when no data
        // came back (we'd otherwise keep stale prior data with an
        // error banner on top).
        if (!result.data && (result.error || result.status !== 'ok')) {
          dataReset.push(result.widget_id);
        }
      }
      if (Object.keys(dataPatch).length > 0 || dataReset.length > 0) {
        setWidgetData(prev => {
          const next = { ...prev, ...dataPatch };
          for (const id of dataReset) next[id] = undefined;
          return next;
        });
      }
      if (Object.keys(errorPatch).length > 0) {
        setWidgetErrors(prev => ({ ...prev, ...errorPatch }));
      }
      if (cachedReset.length > 0) {
        setWidgetCachedAt(prev => {
          const next = { ...prev };
          for (const id of cachedReset) next[id] = undefined;
          return next;
        });
      }
    },
    [isLatestTicket],
  );

  const refreshDashboardWidgets = useCallback(async (dashboard: Dashboard) => {
    // W36: paint last-known-good data immediately so the dashboard
    // appears populated before any live refresh runs. The backend has
    // already pruned snapshots whose fingerprint no longer matches the
    // current widget, so anything that comes back is safe to display.
    try {
      const snapshots = await dashboardApi.listWidgetSnapshots(dashboard.id);
      if (snapshots.length > 0) {
        const dataPatch: Record<string, WidgetRuntimeData> = {};
        const cachedPatch: Record<string, number> = {};
        for (const snap of snapshots) {
          dataPatch[snap.widget_id] = snap.runtime_data;
          cachedPatch[snap.widget_id] = snap.captured_at;
        }
        setWidgetData(prev => ({ ...prev, ...dataPatch }));
        setWidgetCachedAt(prev => ({ ...prev, ...cachedPatch }));
      }
    } catch (err) {
      console.warn('Failed to hydrate widget snapshots:', err);
    }

    // W40: one Tauri call refreshes the whole grid. The backend
    // dedupes by workflow_id and runs independent workflows
    // concurrently, so an 8-widget dashboard backed by two shared
    // sources no longer pays for eight sequential refreshes.
    const widgetIds = dashboard.layout.map(w => w.id);
    if (widgetIds.length === 0) {
      setRefreshingWidgetId(null);
      return;
    }
    const tickets = issueRefreshTicket(widgetIds);
    setRefreshingWidgetId(widgetIds[0] ?? null);
    try {
      const results = await dashboardApi.refreshDashboardWidgets(dashboard.id);
      applyBatchedRefreshResults(results, tickets);
    } catch (err) {
      const message = errorMessage(err, 'Failed to refresh widgets');
      setWidgetErrors(prev => {
        const next = { ...prev };
        for (const id of widgetIds) {
          if (isLatestTicket(id, tickets.get(id) ?? -1)) next[id] = message;
        }
        return next;
      });
    } finally {
      setRefreshingWidgetId(null);
    }
  }, [applyBatchedRefreshResults, isLatestTicket, issueRefreshTicket]);

  const loadDashboards = useCallback(async () => {
    try {
      setError(null);
      const data = await dashboardApi.list();
      setDashboards(data);
      if (data.length > 0 && !activeId) {
        const first = data[0];
        setActiveId(first.id);
        await refreshDashboardWidgets(first);
      }
    } catch (err) {
      console.error('Failed to load dashboards:', err);
      setError(errorMessage(err, 'Failed to load dashboards'));
    } finally {
      setIsReady(true);
    }
  }, [activeId, refreshDashboardWidgets]);

  useEffect(() => {
    loadDashboards();
  }, [loadDashboards]);

  const loadProviders = useCallback(async () => {
    try {
      const data = await providerApi.list();
      const configuredActiveId = await configApi.get('active_provider_id');
      // W29: only honour `active_provider_id` — no silent "first enabled"
      // fallback. If the stored active provider is missing, disabled, or
      // marked unsupported by the migration shim, the chat/build UI
      // surfaces the typed correction state (open Provider Settings).
      const configuredProvider = configuredActiveId
        ? data.find(provider =>
            provider.id === configuredActiveId
            && provider.is_enabled
            && !provider.is_unsupported,
          )
        : undefined;
      const nextActiveId = configuredProvider?.id ?? null;

      setProviders(data);
      setActiveProviderId(nextActiveId);
    } catch (err) {
      console.error('Failed to load providers:', err);
      setError(errorMessage(err, 'Failed to load providers'));
    } finally {
      setIsProvidersReady(true);
    }
  }, []);

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  useEffect(() => {
    const unsubscribe = listen<WorkflowEventEnvelope>('workflow:event', event => {
      const workflowEvent = event.payload;
      if (workflowEvent.kind === 'run_started') {
        setWorkflowRuns(prev => ({
          ...prev,
          [workflowEvent.workflow_id]: {
            id: workflowEvent.run_id,
            started_at: workflowEvent.emitted_at,
            status: workflowEvent.status,
          },
        }));
        setStatusMessage('Workflow started...');
      } else if (workflowEvent.kind === 'run_finished') {
        setWorkflowRuns(prev => ({
          ...prev,
          [workflowEvent.workflow_id]: {
            id: workflowEvent.run_id,
            started_at: prev[workflowEvent.workflow_id]?.started_at ?? workflowEvent.emitted_at,
            finished_at: workflowEvent.emitted_at,
            status: workflowEvent.status,
            node_results: workflowEvent.payload as Record<string, unknown> | undefined,
            error: workflowEvent.error,
          },
        }));
        setStatusMessage(
          workflowEvent.status === 'success'
            ? 'Workflow finished'
            : `Workflow failed${workflowEvent.error ? `: ${workflowEvent.error}` : ''}`
        );
      }
    });

    return () => {
      unsubscribe.then(dispose => dispose()).catch(err => {
        console.error('Failed to unsubscribe from workflow events:', err);
      });
    };
  }, []);

  // W42: subscribe to widget refresh stream events. Updates only
  // overwrite per-widget partial state when the event's `refresh_run_id`
  // is at least as new as the one we are already tracking — anything
  // from an older run is dropped so a slow superseded refresh does not
  // overwrite the partials of a freshly started one. Final/failed
  // envelopes always clear the partial state for their run id.
  useEffect(() => {
    const unsubscribe = listen<WidgetStreamEnvelope>(WIDGET_STREAM_EVENT_CHANNEL, event => {
      const payload = event.payload;
      setWidgetStream(prev => {
        const existing = prev[payload.widget_id];
        // Drop deltas that belong to a refresh older than the one we
        // are tracking (sequence/run-id ordering safety from the doc).
        if (
          existing &&
          existing.runId !== payload.refresh_run_id &&
          (payload.kind === 'text_delta' ||
            payload.kind === 'reasoning_delta' ||
            payload.kind === 'status')
        ) {
          return prev;
        }
        switch (payload.kind) {
          case 'refresh_started':
            return {
              ...prev,
              [payload.widget_id]: {
                runId: payload.refresh_run_id,
                status: 'starting',
                partialText: '',
                reasoningText: '',
                hasReasoning: false,
              },
            };
          case 'reasoning_delta':
            return {
              ...prev,
              [payload.widget_id]: {
                runId: payload.refresh_run_id,
                status: 'reasoning',
                partialText: existing?.partialText ?? '',
                reasoningText: (existing?.reasoningText ?? '') + (payload.text ?? ''),
                hasReasoning: true,
                statusHint: existing?.statusHint,
              },
            };
          case 'text_delta':
            return {
              ...prev,
              [payload.widget_id]: {
                runId: payload.refresh_run_id,
                status: 'streaming',
                partialText: (existing?.partialText ?? '') + (payload.text ?? ''),
                reasoningText: existing?.reasoningText ?? '',
                hasReasoning: existing?.hasReasoning ?? false,
                statusHint: existing?.statusHint,
              },
            };
          case 'status':
            return {
              ...prev,
              [payload.widget_id]: {
                runId: payload.refresh_run_id,
                status: 'waiting',
                partialText: existing?.partialText ?? '',
                reasoningText: existing?.reasoningText ?? '',
                hasReasoning: existing?.hasReasoning ?? false,
                statusHint: payload.status,
              },
            };
          case 'final':
          case 'superseded': {
            // Drop partial state only when the terminal event belongs
            // to the run we are tracking; otherwise leave whatever the
            // newer run already published in place.
            if (existing && existing.runId !== payload.refresh_run_id) return prev;
            const next = { ...prev };
            delete next[payload.widget_id];
            return next;
          }
          case 'failed': {
            if (existing && existing.runId !== payload.refresh_run_id) return prev;
            return {
              ...prev,
              [payload.widget_id]: {
                runId: payload.refresh_run_id,
                status: 'failed',
                partialText: existing?.partialText ?? '',
                reasoningText: existing?.reasoningText ?? '',
                hasReasoning: existing?.hasReasoning ?? false,
                statusHint: existing?.statusHint,
                error: payload.error,
                partialOnFail: payload.partial_text ?? existing?.partialText,
              },
            };
          }
          default:
            return prev;
        }
      });
    });

    return () => {
      unsubscribe.then(dispose => dispose()).catch(err => {
        console.error('Failed to unsubscribe from widget stream events:', err);
      });
    };
  }, []);

  const activeDashboard = dashboards.find(d => d.id === activeId);
  // W29: strict active provider — no silent first-enabled fallback.
  const activeProvider = providers.find(provider =>
    provider.id === activeProviderId
    && provider.is_enabled
    && !provider.is_unsupported,
  );

  const handleSelectDashboard = async (id: string) => {
    setActiveId(id);
    setError(null);
    setIsBusy(true);
    setStatusMessage('Loading dashboard...');
    try {
      const dashboard = await dashboardApi.get(id);
      setDashboards(prev => upsertDashboard(prev, dashboard));
      setStatusMessage('Refreshing widgets...');
      await refreshDashboardWidgets(dashboard);
      setStatusMessage('Dashboard loaded');
    } catch (err) {
      console.error('Failed to load dashboard:', err);
      setError(errorMessage(err, 'Failed to load dashboard'));
      setStatusMessage('Dashboard load failed');
    } finally {
      setIsBusy(false);
      setRefreshingWidgetId(null);
    }
  };

  const handleCreate = async (template: 'blank' | 'local_mvp' = 'blank') => {
    try {
      setError(null);
      setIsBusy(true);
      setStatusMessage('Creating dashboard...');
      const d = await dashboardApi.create(
        template === 'local_mvp' ? 'Local MVP Dashboard' : 'New Dashboard',
        template === 'local_mvp' ? 'Deterministic local workflow connected to a widget.' : undefined,
        template
      );
      setDashboards(prev => [...prev, d]);
      setActiveId(d.id);
      if (template === 'local_mvp' && d.layout.length > 0) {
        setRefreshingWidgetId(d.layout[0].id);
        const result = await dashboardApi.refreshWidget(d.id, d.layout[0].id);
        if (result.data) {
          setWidgetData(prev => ({ ...prev, [d.layout[0].id]: result.data }));
          setStatusMessage('Local MVP slice ready');
        } else {
          setWidgetErrors(prev => ({
            ...prev,
            [d.layout[0].id]: result.error || `Widget refresh returned no runtime data: ${result.status}`,
          }));
          setStatusMessage('Dashboard created; widget refresh unavailable');
        }
      } else {
        setStatusMessage('Dashboard created');
      }
    } catch (err) {
      console.error('Failed to create dashboard:', err);
      setError(errorMessage(err, 'Failed to create dashboard'));
      setStatusMessage('Create failed');
    } finally {
      setIsBusy(false);
      setRefreshingWidgetId(null);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      setError(null);
      setIsBusy(true);
      setStatusMessage('Deleting dashboard...');
      await dashboardApi.delete(id);
      setDashboards(prev => prev.filter(d => d.id !== id));
      if (activeId === id) {
        const remaining = dashboards.filter(d => d.id !== id);
        setActiveId(remaining[0]?.id ?? null);
      }
      setStatusMessage('Dashboard deleted');
    } catch (err) {
      console.error('Failed to delete dashboard:', err);
      setError(errorMessage(err, 'Failed to delete dashboard'));
      setStatusMessage('Delete failed');
    } finally {
      setIsBusy(false);
    }
  };

  const handleLayoutCommit = async (layout: Widget[]) => {
    if (!activeDashboard) return;
    const hasChanged = layout.some((widget, index) => {
      const current = activeDashboard.layout[index];
      return current && (
        current.x !== widget.x ||
        current.y !== widget.y ||
        current.w !== widget.w ||
        current.h !== widget.h
      );
    });
    if (!hasChanged) return;

    const optimistic = { ...activeDashboard, layout };
    setDashboards(prev => upsertDashboard(prev, optimistic));
    setStatusMessage('Saving layout...');

    try {
      const updated = await dashboardApi.update(activeDashboard.id, { layout });
      setDashboards(prev => upsertDashboard(prev, updated));
      setStatusMessage('Layout saved');
    } catch (err) {
      console.error('Failed to save dashboard layout:', err);
      setError(errorMessage(err, 'Failed to save dashboard layout'));
      setDashboards(prev => upsertDashboard(prev, activeDashboard));
      setStatusMessage('Layout save failed');
    }
  };

  const handleAddWidget = async (widgetType: 'text' | 'gauge') => {
    if (!activeDashboard) return;
    setIsBusy(true);
    setError(null);
    setStatusMessage('Adding widget...');
    try {
      const updated = await dashboardApi.addWidget(activeDashboard.id, {
        widget_type: widgetType,
        title: widgetType === 'text' ? 'Local note' : 'Local metric',
        content: widgetType === 'text' ? '**Local note**\n\nSaved through Datrina apply command.' : undefined,
        value: widgetType === 'gauge' ? 64 : undefined,
      });
      setDashboards(prev => upsertDashboard(prev, updated));
      setStatusMessage('Widget added');
    } catch (err) {
      setError(errorMessage(err, 'Failed to add widget'));
      setStatusMessage('Add widget failed');
    } finally {
      setIsBusy(false);
    }
  };

  const handleApplyBuildProposal = async (proposal: BuildProposal, sessionId?: string) => {
    const dashboardIdBeforeApply = activeDashboard?.id;
    setIsBusy(true);
    setError(null);
    setStatusMessage('Applying build proposal...');
    try {
      const updated = await dashboardApi.applyBuildProposal({
        proposal,
        dashboard_id: dashboardIdBeforeApply,
        confirmed: true,
        session_id: sessionId,
      });
      setDashboards(prev => upsertDashboard(prev, updated));
      setActiveId(updated.id);
      setStatusMessage('Build proposal applied');

      // W19: surface a 10s Undo toast if the apply mutated an existing
      // dashboard. New dashboards have no pre-apply state to undo to, so
      // we skip the toast in that case.
      if (dashboardIdBeforeApply) {
        try {
          const versions = await dashboardApi.listVersions(updated.id);
          const undoTarget = versions.find(v => v.source === 'agent_apply');
          if (undoTarget) {
            setUndoToast({
              versionId: undoTarget.id,
              label: proposal.title?.trim() || 'Apply',
            });
          }
        } catch (err) {
          console.warn('Failed to list versions for Undo toast:', err);
        }
      }

      const newWidgets = updated.layout.slice(Math.max(0, updated.layout.length - proposal.widgets.length));
      const newWidgetIds = newWidgets.map(w => w.id);
      if (newWidgetIds.length > 0) {
        const tickets = issueRefreshTicket(newWidgetIds);
        setRefreshingWidgetId(newWidgetIds[0] ?? null);
        try {
          const results = await dashboardApi.refreshDashboardWidgets(updated.id, newWidgetIds);
          applyBatchedRefreshResults(results, tickets);
        } catch (err) {
          const message = errorMessage(err, 'Failed to refresh widget');
          setWidgetErrors(prev => {
            const next = { ...prev };
            for (const id of newWidgetIds) {
              if (isLatestTicket(id, tickets.get(id) ?? -1)) next[id] = message;
            }
            return next;
          });
        }
      }
    } catch (err) {
      setError(errorMessage(err, 'Failed to apply build proposal'));
      setStatusMessage('Build proposal apply failed');
    } finally {
      setIsBusy(false);
      setRefreshingWidgetId(null);
    }
  };

  const handleRestoreVersion = async (versionId: string) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Restoring version...');
    try {
      const restored = await dashboardApi.restoreVersion(versionId);
      setDashboards(prev => upsertDashboard(prev, restored));
      setActiveId(restored.id);
      setUndoToast(null);
      setStatusMessage('Version restored');
      const widgetIds = restored.layout.map(w => w.id);
      if (widgetIds.length > 0) {
        const tickets = issueRefreshTicket(widgetIds);
        try {
          const results = await dashboardApi.refreshDashboardWidgets(restored.id, widgetIds);
          applyBatchedRefreshResults(results, tickets);
        } catch (err) {
          console.warn('Failed to refresh widgets after restore:', err);
        }
      }
    } catch (err) {
      setError(errorMessage(err, 'Failed to restore version'));
      setStatusMessage('Restore failed');
    } finally {
      setIsBusy(false);
    }
  };

  useEffect(() => {
    if (!undoToast) return;
    const id = window.setTimeout(() => setUndoToast(null), 10_000);
    return () => window.clearTimeout(id);
  }, [undoToast]);

  useEffect(() => {
    const sync = () => {
      const hash = window.location.hash;
      if (hash === '#/playground') setRoute('playground');
      else if (hash === '#/alerts') setRoute('alerts');
      else if (hash === '#/workbench') setRoute('workbench');
      else if (hash === '#/sources') setRoute('sources');
      else if (hash.startsWith('#/operations')) setRoute('operations');
      else setRoute('dashboards');
    };
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  }, []);

  const navigateToPlayground = useCallback(() => {
    window.location.hash = '#/playground';
    setRoute('playground');
  }, []);

  const navigateToDashboards = useCallback(() => {
    window.location.hash = '';
    setRoute('dashboards');
  }, []);

  const navigateToAlerts = useCallback(() => {
    window.location.hash = '#/alerts';
    setRoute('alerts');
  }, []);

  const navigateToWorkbench = useCallback(() => {
    window.location.hash = '#/workbench';
    setRoute('workbench');
  }, []);

  const navigateToSources = useCallback(() => {
    window.location.hash = '#/sources';
    setRoute('sources');
  }, []);

  const navigateToOperations = useCallback((opts?: { workflowId?: string; runId?: string }) => {
    const params = new URLSearchParams();
    if (opts?.workflowId) params.set('workflow_id', opts.workflowId);
    if (opts?.runId) params.set('run_id', opts.runId);
    const query = params.toString();
    window.location.hash = query ? `#/operations?${query}` : '#/operations';
    setRoute('operations');
  }, []);

  // W21: keep the Sidebar badge + per-widget dot in sync. The count
  // comes from the backend, but the events array is also retained so we
  // can derive per-widget status without a second round-trip.
  const refreshAlertCount = useCallback(async () => {
    try {
      const count = await alertApi.countUnacknowledged();
      setUnacknowledgedAlertCount(count);
    } catch (err) {
      console.warn('Failed to load alert count:', err);
    }
  }, []);

  const refreshAlertEvents = useCallback(async () => {
    try {
      const data = await alertApi.listEvents(true, 200);
      setAlertEvents(data);
      setUnacknowledgedAlertCount(data.length);
    } catch (err) {
      console.warn('Failed to load alert events:', err);
    }
  }, []);

  useEffect(() => {
    refreshAlertEvents();
  }, [refreshAlertEvents]);

  useEffect(() => {
    const unsubscribe = listen<AlertEvent>(ALERT_EVENT_CHANNEL, evt => {
      setAlertEvents(prev => [evt.payload, ...prev]);
      setUnacknowledgedAlertCount(prev => prev + 1);
    });
    return () => {
      unsubscribe.then(dispose => dispose()).catch(() => {});
    };
  }, []);

  // Per-widget alert status derived from current unack events. Recomputed
  // cheaply since `alertEvents` is bounded at 200.
  const widgetAlertStatus = useMemo(() => {
    const out: Record<string, WidgetAlertStatus> = {};
    for (const event of alertEvents) {
      if (event.acknowledged_at) continue;
      const existing = out[event.widget_id];
      if (!existing) {
        out[event.widget_id] = { count: 1, severity: event.severity };
      } else {
        existing.count += 1;
        if (severityRank(event.severity) < severityRank(existing.severity)) {
          existing.severity = event.severity;
        }
      }
    }
    return out;
  }, [alertEvents]);

  // W28: seeded Build entrypoints (templates, Playground, fork-from-message)
  // must land in a *fresh* Build chat, not in the latest matching session.
  // Bumping freshChatSessionKey tells ChatPanel to drop any reused session.
  const openBuildChatWithPrompt = useCallback((prompt: string) => {
    setPendingBuildPrompt(prompt);
    setChatMode('build');
    setIsChatOpen(true);
    setFreshChatSessionKey(k => k + 1);
  }, []);

  // W28: top-bar Build button. If a Build chat is already open we just
  // surface it. If it isn't, force a fresh Build draft so the user does
  // not silently land inside the last persisted Build session for this
  // dashboard.
  const openBuildChatFromTopBar = useCallback(() => {
    setChatMode('build');
    if (!isChatOpen) {
      setFreshChatSessionKey(k => k + 1);
    }
    setIsChatOpen(true);
  }, [isChatOpen]);

  // W28: "Fork to fresh Build chat" — explicit honest path for retrying
  // a Build prompt without inheriting the prior session's derived plan
  // state, cost totals, or proposal pipeline.
  const forkToFreshBuildChat = useCallback((prompt: string) => {
    openBuildChatWithPrompt(prompt);
  }, [openBuildChatWithPrompt]);

  const handleTemplateSelect = useCallback(async (template: DashboardTemplate) => {
    if (template.launch === 'playground') {
      navigateToPlayground();
      return;
    }
    try {
      setIsBusy(true);
      setStatusMessage('Creating dashboard from template...');
      const dashboard = await dashboardApi.create(template.title, template.description, 'blank');
      setDashboards(prev => [...prev, dashboard]);
      setActiveId(dashboard.id);
      navigateToDashboards();
      if (template.launch === 'build_chat' && template.prompt) {
        openBuildChatWithPrompt(template.prompt);
      } else {
        // W28: even template launches without a seeded prompt should
        // open a fresh Build draft instead of reusing the last Build
        // session for the just-created dashboard.
        setChatMode('build');
        setIsChatOpen(true);
        setFreshChatSessionKey(k => k + 1);
      }
      setStatusMessage('Template ready — describe what you need in chat');
    } catch (err) {
      setError(errorMessage(err, 'Failed to create dashboard from template'));
      setStatusMessage('Template launch failed');
    } finally {
      setIsBusy(false);
    }
  }, [navigateToDashboards, navigateToPlayground, openBuildChatWithPrompt]);

  const handleRefreshWidget = async (widgetId: string) => {
    if (!activeDashboard) return;
    setRefreshingWidgetId(widgetId);
    setWidgetErrors(prev => ({ ...prev, [widgetId]: undefined }));
    setStatusMessage('Refreshing widget...');
    // W40: tag this run so a stale batched refresh can't overwrite it.
    const tickets = issueRefreshTicket([widgetId]);
    const ticket = tickets.get(widgetId) ?? -1;

    try {
      const result = await dashboardApi.refreshWidget(activeDashboard.id, widgetId);
      if (!isLatestTicket(widgetId, ticket)) {
        setStatusMessage('Widget refresh superseded');
        return;
      }
      if (result.data) {
        setWidgetData(prev => ({ ...prev, [widgetId]: result.data }));
        setWidgetCachedAt(prev => ({ ...prev, [widgetId]: undefined }));
        setStatusMessage('Widget refreshed');
        return;
      }

      const reason = result.error || (
        result.status === 'not_implemented'
          ? 'Widget runtime refresh is unavailable for this datasource.'
          : `Widget refresh returned no runtime data: ${result.status}`
      );
      setWidgetErrors(prev => ({ ...prev, [widgetId]: reason }));
      setStatusMessage('Widget refresh unavailable');
    } catch (err) {
      if (!isLatestTicket(widgetId, ticket)) return;
      console.error('Failed to refresh widget:', err);
      setWidgetErrors(prev => ({ ...prev, [widgetId]: errorMessage(err, 'Failed to refresh widget') }));
      setStatusMessage('Widget refresh failed');
    } finally {
      setRefreshingWidgetId(null);
    }
  };

  const handleAddProvider = async (provider: CreateProviderRequest) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Saving provider...');
    try {
      const saved = await providerApi.add(provider);
      setProviders(prev => upsertProvider(prev, saved));
      setActiveProviderId(saved.id);
      await configApi.set('active_provider_id', saved.id);
      if (providers.length === 0) {
        setIsProviderSettingsOpen(false);
      }
      setStatusMessage('LLM provider configured');
    } catch (err) {
      setStatusMessage('Provider save failed');
      throw err;
    } finally {
      setIsBusy(false);
    }
  };

  const handleUpdateProvider = async (id: string, provider: UpdateProviderRequest) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Updating provider...');
    try {
      const saved = await providerApi.update(id, provider);
      setProviders(prev => upsertProvider(prev, saved));
      if (saved.is_enabled) {
        setActiveProviderId(saved.id);
        await configApi.set('active_provider_id', saved.id);
      }
      setStatusMessage('Provider updated');
    } catch (err) {
      setError(errorMessage(err, 'Failed to update provider'));
      setStatusMessage('Provider update failed');
      throw err;
    } finally {
      setIsBusy(false);
    }
  };

  const handleSetProviderEnabled = async (id: string, isEnabled: boolean) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage(isEnabled ? 'Enabling provider...' : 'Disabling provider...');
    try {
      const saved = await providerApi.setEnabled(id, isEnabled);
      setProviders(prev => upsertProvider(prev, saved));
      if (!isEnabled && activeProviderId === id) {
        const nextActiveId = providers.find(provider => provider.id !== id && provider.is_enabled)?.id ?? null;
        setActiveProviderId(nextActiveId);
        await configApi.set('active_provider_id', nextActiveId ?? '');
      } else if (isEnabled) {
        setActiveProviderId(saved.id);
        await configApi.set('active_provider_id', saved.id);
      }
      setStatusMessage(isEnabled ? 'Provider enabled' : 'Provider disabled');
    } catch (err) {
      setError(errorMessage(err, 'Failed to update provider state'));
      setStatusMessage('Provider state update failed');
    } finally {
      setIsBusy(false);
    }
  };

  const handleRemoveProvider = async (id: string) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Removing provider...');
    try {
      await providerApi.remove(id);
      const remaining = providers.filter(provider => provider.id !== id);
      const nextActiveId = remaining.find(provider => provider.is_enabled)?.id ?? null;
      setProviders(remaining);
      if (activeProviderId === id) {
        setActiveProviderId(nextActiveId);
        await configApi.set('active_provider_id', nextActiveId ?? '');
      }
      setStatusMessage(nextActiveId ? 'Provider removed' : 'LLM provider required');
    } catch (err) {
      setError(errorMessage(err, 'Failed to remove provider'));
      setStatusMessage('Provider remove failed');
    } finally {
      setIsBusy(false);
    }
  };

  const handleSetActiveProvider = async (id: string) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Selecting provider...');
    try {
      await configApi.set('active_provider_id', id);
      setActiveProviderId(id);
      setStatusMessage('Active provider updated');
    } catch (err) {
      setError(errorMessage(err, 'Failed to select provider'));
      setStatusMessage('Provider selection failed');
    } finally {
      setIsBusy(false);
    }
  };

  const handleTestProvider = async (id: string) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Testing provider...');
    try {
      const result = await providerApi.test(id);
      setStatusMessage(result.status === 'ok' ? 'Provider test passed' : 'Provider test failed');
      return result;
    } finally {
      setIsBusy(false);
    }
  };

  if (!isReady || !isProvidersReady) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-background">
        <div className="text-center space-y-4">
          <div className="relative w-14 h-14 rounded-lg bg-gradient-to-br from-primary/20 to-accent/20 flex items-center justify-center mx-auto border border-primary/30">
            <span className="neon-pulse" aria-hidden />
            <DatrinaLogo className="relative h-10 w-10 rounded-md border border-primary/30 bg-background/80" imageClassName="scale-110" />
          </div>
          <p className="text-muted-foreground text-xs mono uppercase tracking-[0.2em]">Booting Datrina…</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-screen w-screen bg-background overflow-hidden">
      <Sidebar
        dashboards={dashboards}
        activeId={activeId}
        onSelect={(id) => { navigateToDashboards(); handleSelectDashboard(id); }}
        onCreate={() => { navigateToDashboards(); handleCreate('blank'); }}
        onCreateFromTemplate={() => setIsTemplateGalleryOpen(true)}
        onDelete={handleDelete}
        theme={theme}
        onToggleTheme={() => setTheme(t => t === 'dark' ? 'light' : 'dark')}
        onOpenSettings={() => setIsProviderSettingsOpen(true)}
        onOpenMcpSettings={() => setIsMcpSettingsOpen(true)}
        onOpenMemorySettings={() => setIsMemorySettingsOpen(true)}
        onOpenCostsView={() => setIsCostsViewOpen(true)}
        onOpenPlayground={navigateToPlayground}
        isPlaygroundActive={route === 'playground'}
        onOpenAlerts={navigateToAlerts}
        isAlertsActive={route === 'alerts'}
        onOpenWorkbench={navigateToWorkbench}
        isWorkbenchActive={route === 'workbench'}
        onOpenSources={navigateToSources}
        isSourcesActive={route === 'sources'}
        onOpenOperations={() => navigateToOperations()}
        isOperationsActive={route === 'operations'}
        unacknowledgedAlertCount={unacknowledgedAlertCount}
        isCollapsed={sidebarCollapsed}
        onToggleCollapse={() => setSidebarCollapsed(!sidebarCollapsed)}
      />

      <div className="flex flex-col flex-1 min-w-0">
        <TopBar
          dashboard={activeDashboard}
          activeProvider={activeProvider}
          isChatOpen={isChatOpen}
          onToggleChat={() => setIsChatOpen(!isChatOpen)}
          onOpenBuildChat={openBuildChatFromTopBar}
          onOpenSettings={() => setIsProviderSettingsOpen(true)}
        />

        <main className={`flex-1 min-h-0 ${route === 'playground' ? 'overflow-hidden' : 'overflow-auto p-4 scrollbar-thin'}`}>
          {error && route === 'dashboards' && (
            <div className="mb-3 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              <svg className="w-4 h-4 flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden>
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01M4.93 19h14.14a2 2 0 001.73-3l-7.07-12a2 2 0 00-3.46 0l-7.07 12a2 2 0 001.73 3z" />
              </svg>
              <span className="flex-1">{error}</span>
            </div>
          )}
          {route === 'playground' ? (
            <Playground
              onUseAsWidget={({ prompt }) => {
                navigateToDashboards();
                openBuildChatWithPrompt(prompt);
              }}
              onClose={navigateToDashboards}
            />
          ) : route === 'alerts' ? (
            <AlertsView
              dashboards={dashboards}
              onJumpToWidget={(dashboardId) => {
                navigateToDashboards();
                handleSelectDashboard(dashboardId);
              }}
              onJumpToRun={(runId) => navigateToOperations({ runId })}
              onClose={navigateToDashboards}
            />
          ) : route === 'workbench' ? (
            <Workbench
              onClose={navigateToDashboards}
              onJumpToWidget={(dashboardId) => {
                navigateToDashboards();
                handleSelectDashboard(dashboardId);
              }}
              onJumpToOperations={(workflowId) => navigateToOperations({ workflowId })}
            />
          ) : route === 'sources' ? (
            <SourceCatalog
              onClose={navigateToDashboards}
              onOpenWorkbench={navigateToWorkbench}
            />
          ) : route === 'operations' ? (
            (() => {
              // Parse the hash query once per render so deep-links like
              // `#/operations?workflow_id=…&run_id=…` map to the
              // OperationsView props. Returning a tuple via IIFE keeps
              // the JSX site readable.
              const params = (() => {
                if (typeof window === 'undefined') return null;
                const hash = window.location.hash;
                const idx = hash.indexOf('?');
                if (idx < 0) return null;
                return new URLSearchParams(hash.slice(idx + 1));
              })();
              const workflowId = params?.get('workflow_id') ?? undefined;
              const runId = params?.get('run_id') ?? undefined;
              return (
                <OperationsView
                  onClose={navigateToDashboards}
                  onJumpToWidget={(dashboardId) => {
                    navigateToDashboards();
                    handleSelectDashboard(dashboardId);
                  }}
                  onJumpToDatasource={() => navigateToWorkbench()}
                  initialFilter={workflowId ? { workflow_id: workflowId } : undefined}
                  initialRunId={runId}
                />
              );
            })()
          ) : activeDashboard ? (
            <DashboardGrid
              dashboard={activeDashboard}
              widgetData={widgetData}
              widgetCachedAt={widgetCachedAt}
              widgetErrors={widgetErrors}
              widgetStream={widgetStream}
              workflowRuns={workflowRuns}
              refreshingWidgetId={refreshingWidgetId}
              onRefreshWidget={handleRefreshWidget}
              onLayoutCommit={handleLayoutCommit}
              onAddWidget={handleAddWidget}
              onUpdateWidgets={handleLayoutCommit}
              onOpenHistory={() => setIsHistoryOpen(true)}
              widgetAlertStatus={widgetAlertStatus}
              onOpenAlertsEditor={(widgetId) => setAlertEditorWidgetId(widgetId)}
              providers={providers}
              onDashboardChange={(updated) =>
                setDashboards((prev) => upsertDashboard(prev, updated))
              }
            />
          ) : (
            <TemplateGallery
              onSelect={handleTemplateSelect}
              onOpenPlayground={navigateToPlayground}
              onOpenMcpSettings={() => setIsMcpSettingsOpen(true)}
            />
          )}
        </main>

        <StatusBar dashboardCount={dashboards.length} widgetCount={activeDashboard?.layout.length ?? 0} status={statusMessage} isBusy={isBusy || refreshingWidgetId !== null} />
      </div>

      {isChatOpen && (
        <ChatPanel
          mode={chatMode}
          dashboardId={activeId ?? undefined}
          dashboardName={activeDashboard?.name}
          activeProvider={activeProvider}
          canApplyToDashboard={Boolean(activeDashboard)}
          initialPrompt={pendingBuildPrompt ?? undefined}
          onInitialPromptConsumed={() => setPendingBuildPrompt(null)}
          freshSessionKey={freshChatSessionKey}
          onClose={() => setIsChatOpen(false)}
          onModeChange={setChatMode}
          onApplyBuildProposal={handleApplyBuildProposal}
          onOpenProviderSettings={() => setIsProviderSettingsOpen(true)}
          onForkBuildChat={forkToFreshBuildChat}
        />
      )}

      {isTemplateGalleryOpen && (
        <TemplateGallery
          variant="modal"
          onSelect={handleTemplateSelect}
          onOpenPlayground={navigateToPlayground}
          onOpenMcpSettings={() => setIsMcpSettingsOpen(true)}
          onClose={() => setIsTemplateGalleryOpen(false)}
        />
      )}

      {isMcpSettingsOpen && (
        <McpSettings onClose={() => setIsMcpSettingsOpen(false)} />
      )}

      {isMemorySettingsOpen && (
        <MemorySettings onClose={() => setIsMemorySettingsOpen(false)} />
      )}

      {isCostsViewOpen && (
        <CostsView onClose={() => setIsCostsViewOpen(false)} />
      )}

      {(isProviderSettingsOpen || providers.length === 0) && (
        <ProviderSettings
          providers={providers}
          activeProviderId={activeProviderId}
          initialSetup={providers.length === 0}
          isBusy={isBusy}
          error={error}
          onClose={() => setIsProviderSettingsOpen(false)}
          onAddProvider={handleAddProvider}
          onUpdateProvider={handleUpdateProvider}
          onSetProviderEnabled={handleSetProviderEnabled}
          onRemoveProvider={handleRemoveProvider}
          onSetActiveProvider={handleSetActiveProvider}
          onTestProvider={handleTestProvider}
        />
      )}

      {isHistoryOpen && activeDashboard && (
        <HistoryDrawer
          dashboardId={activeDashboard.id}
          onClose={() => setIsHistoryOpen(false)}
          onRestored={dashboard => {
            setDashboards(prev => upsertDashboard(prev, dashboard));
            setActiveId(dashboard.id);
            setUndoToast(null);
          }}
        />
      )}

      {alertEditorWidgetId && activeDashboard && (
        <AlertEditorModal
          dashboardId={activeDashboard.id}
          widgetId={alertEditorWidgetId}
          widgetTitle={activeDashboard.layout.find(w => w.id === alertEditorWidgetId)?.title ?? ''}
          lastData={widgetData[alertEditorWidgetId]}
          onClose={() => setAlertEditorWidgetId(null)}
          onSaved={() => {
            setAlertEditorWidgetId(null);
            refreshAlertCount();
          }}
        />
      )}

      {undoToast && (
        <div className="pointer-events-auto fixed bottom-6 right-6 z-[60] flex items-center gap-3 rounded-md border border-primary/40 bg-card/95 backdrop-blur px-4 py-2 text-sm shadow-xl glow-primary">
          <span className="mono text-[10px] uppercase tracking-wider text-primary">Applied</span>
          <span className="text-foreground truncate max-w-xs">{undoToast.label}</span>
          <button
            onClick={() => handleRestoreVersion(undoToast.versionId)}
            className="rounded-md border border-border px-2 py-1 text-xs font-medium hover:bg-muted hover:border-primary/40 transition-colors"
          >
            ↩ Undo
          </button>
          <button
            onClick={() => setUndoToast(null)}
            className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
            aria-label="Dismiss"
          >
            <svg className="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      )}
    </div>
  );
}

function upsertDashboard(dashboards: Dashboard[], dashboard: Dashboard) {
  const exists = dashboards.some(item => item.id === dashboard.id);
  if (!exists) return [...dashboards, dashboard];
  return dashboards.map(item => item.id === dashboard.id ? dashboard : item);
}

function upsertProvider(providers: LLMProvider[], provider: LLMProvider) {
  const exists = providers.some(item => item.id === provider.id);
  if (!exists) return [...providers, provider];
  return providers.map(item => item.id === provider.id ? provider : item);
}

function errorMessage(err: unknown, fallback: string) {
  return err instanceof Error ? err.message : fallback;
}

function severityRank(severity: AlertSeverity): number {
  if (severity === 'critical') return 0;
  if (severity === 'warning') return 1;
  return 2;
}

export default App;
