import { useState, useEffect, useCallback, useMemo } from 'react';
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
import { HistoryDrawer } from './components/dashboard/HistoryDrawer';
import { Playground } from './components/playground/Playground';
import { TemplateGallery } from './components/onboarding/TemplateGallery';
import { AlertsView } from './components/alerts/AlertsView';
import { AlertEditorModal } from './components/alerts/AlertEditorModal';
import type { WidgetAlertStatus } from './components/layout/DashboardGrid';
import type { DashboardTemplate } from './lib/templates';
import { ALERT_EVENT_CHANNEL, alertApi, configApi, dashboardApi, providerApi } from './lib/api';
import type { AlertEvent, AlertSeverity, BuildProposal, CreateProviderRequest, Dashboard, LLMProvider, UpdateProviderRequest, Widget, WidgetRuntimeData, WorkflowEventEnvelope, WorkflowRun } from './lib/api';

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
    } else {
      root.classList.remove('dark');
    }
    window.localStorage.setItem('datrina:theme', theme);
  }, [theme]);
  const [isReady, setIsReady] = useState(false);
  const [isProvidersReady, setIsProvidersReady] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [statusMessage, setStatusMessage] = useState('Ready');
  const [error, setError] = useState<string | null>(null);
  const [widgetData, setWidgetData] = useState<Record<string, WidgetRuntimeData | undefined>>({});
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
  const [route, setRoute] = useState<'dashboards' | 'playground' | 'alerts'>(() => {
    if (typeof window === 'undefined') return 'dashboards';
    if (window.location.hash === '#/playground') return 'playground';
    if (window.location.hash === '#/alerts') return 'alerts';
    return 'dashboards';
  });
  const [alertEvents, setAlertEvents] = useState<AlertEvent[]>([]);
  const [unacknowledgedAlertCount, setUnacknowledgedAlertCount] = useState(0);
  const [alertEditorWidgetId, setAlertEditorWidgetId] = useState<string | null>(null);
  const [isTemplateGalleryOpen, setIsTemplateGalleryOpen] = useState(false);
  const [pendingBuildPrompt, setPendingBuildPrompt] = useState<string | null>(null);

  const loadDashboards = useCallback(async () => {
    try {
      setError(null);
      const data = await dashboardApi.list();
      setDashboards(data);
      if (data.length > 0 && !activeId) {
        setActiveId(data[0].id);
      }
    } catch (err) {
      console.error('Failed to load dashboards:', err);
      setError(errorMessage(err, 'Failed to load dashboards'));
    } finally {
      setIsReady(true);
    }
  }, [activeId]);

  useEffect(() => {
    loadDashboards();
  }, [loadDashboards]);

  const loadProviders = useCallback(async () => {
    try {
      const data = await providerApi.list();
      const configuredActiveId = await configApi.get('active_provider_id');
      const configuredProvider = configuredActiveId
        ? data.find(provider => provider.id === configuredActiveId && provider.is_enabled)
        : undefined;
      const fallbackProvider = data.find(provider => provider.is_enabled);
      const nextActiveId = configuredProvider?.id ?? fallbackProvider?.id ?? null;

      setProviders(data);
      setActiveProviderId(nextActiveId);

      if (nextActiveId && nextActiveId !== configuredActiveId) {
        await configApi.set('active_provider_id', nextActiveId);
      }
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

  const activeDashboard = dashboards.find(d => d.id === activeId);
  const activeProvider = providers.find(provider => provider.id === activeProviderId && provider.is_enabled)
    ?? providers.find(provider => provider.is_enabled);

  const handleSelectDashboard = async (id: string) => {
    setActiveId(id);
    setError(null);
    setIsBusy(true);
    setStatusMessage('Loading dashboard...');
    try {
      const dashboard = await dashboardApi.get(id);
      setDashboards(prev => upsertDashboard(prev, dashboard));
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
      for (const widget of newWidgets) {
        setRefreshingWidgetId(widget.id);
        const result = await dashboardApi.refreshWidget(updated.id, widget.id);
        if (result.data) {
          setWidgetData(prev => ({ ...prev, [widget.id]: result.data }));
        } else if (result.error) {
          setWidgetErrors(prev => ({ ...prev, [widget.id]: result.error }));
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
      for (const widget of restored.layout) {
        try {
          const result = await dashboardApi.refreshWidget(restored.id, widget.id);
          if (result.data) {
            setWidgetData(prev => ({ ...prev, [widget.id]: result.data }));
          }
        } catch (err) {
          console.warn(`Failed to refresh widget ${widget.id} after restore:`, err);
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

  const openBuildChatWithPrompt = useCallback((prompt: string) => {
    setPendingBuildPrompt(prompt);
    setChatMode('build');
    setIsChatOpen(true);
  }, []);

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
        setChatMode('build');
        setIsChatOpen(true);
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

    try {
      const result = await dashboardApi.refreshWidget(activeDashboard.id, widgetId);
      if (result.data) {
        setWidgetData(prev => ({ ...prev, [widgetId]: result.data }));
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
          <div className="w-12 h-12 rounded-xl bg-primary/10 flex items-center justify-center mx-auto animate-pulse">
            <svg className="w-6 h-6 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
            </svg>
          </div>
          <p className="text-muted-foreground text-sm">Loading Datrina...</p>
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
          onOpenBuildChat={() => { setChatMode('build'); setIsChatOpen(true); }}
          onOpenSettings={() => setIsProviderSettingsOpen(true)}
        />

        <main className={`flex-1 min-h-0 overflow-hidden ${route === 'playground' ? '' : 'overflow-auto p-4 scrollbar-thin'}`}>
          {error && route === 'dashboards' && (
            <div className="mb-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive">
              {error}
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
              onClose={navigateToDashboards}
            />
          ) : activeDashboard ? (
            <DashboardGrid
              dashboard={activeDashboard}
              widgetData={widgetData}
              widgetErrors={widgetErrors}
              workflowRuns={workflowRuns}
              refreshingWidgetId={refreshingWidgetId}
              onRefreshWidget={handleRefreshWidget}
              onLayoutCommit={handleLayoutCommit}
              onAddWidget={handleAddWidget}
              onUpdateWidgets={handleLayoutCommit}
              onOpenHistory={() => setIsHistoryOpen(true)}
              widgetAlertStatus={widgetAlertStatus}
              onOpenAlertsEditor={(widgetId) => setAlertEditorWidgetId(widgetId)}
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
          onClose={() => setIsChatOpen(false)}
          onModeChange={setChatMode}
          onApplyBuildProposal={handleApplyBuildProposal}
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
        <div className="pointer-events-auto fixed bottom-6 right-6 z-[60] flex items-center gap-3 rounded-lg border border-border bg-card px-4 py-2 text-sm shadow-lg">
          <span className="text-foreground">Applied: {undoToast.label}</span>
          <button
            onClick={() => handleRestoreVersion(undoToast.versionId)}
            className="rounded-md border border-border px-2 py-1 text-xs font-medium hover:bg-muted"
          >
            ↩ Undo
          </button>
          <button
            onClick={() => setUndoToast(null)}
            className="rounded p-1 text-muted-foreground hover:bg-muted"
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
