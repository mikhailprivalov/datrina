import { useState, useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Sidebar } from './components/layout/Sidebar';
import { DashboardGrid } from './components/layout/DashboardGrid';
import { ChatPanel } from './components/layout/ChatPanel';
import { TopBar } from './components/layout/TopBar';
import { StatusBar } from './components/layout/StatusBar';
import { ProviderSettings } from './components/layout/ProviderSettings';
import { configApi, dashboardApi, providerApi } from './lib/api';
import type { BuildProposal, CreateProviderRequest, Dashboard, LLMProvider, UpdateProviderRequest, Widget, WidgetRuntimeData, WorkflowEventEnvelope, WorkflowRun } from './lib/api';

function App() {
  const [dashboards, setDashboards] = useState<Dashboard[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [isChatOpen, setIsChatOpen] = useState(false);
  const [chatMode, setChatMode] = useState<'build' | 'context'>('context');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
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

  const handleApplyBuildProposal = async (proposal: BuildProposal) => {
    setIsBusy(true);
    setError(null);
    setStatusMessage('Applying build proposal...');
    try {
      const updated = await dashboardApi.applyBuildProposal({
        proposal,
        dashboard_id: activeDashboard?.id,
        confirmed: true,
      });
      setDashboards(prev => upsertDashboard(prev, updated));
      setActiveId(updated.id);
      setStatusMessage('Build proposal applied');
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
        onSelect={handleSelectDashboard}
        onCreate={handleCreate}
        onDelete={handleDelete}
        onOpenSettings={() => setIsProviderSettingsOpen(true)}
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

        <main className="flex-1 overflow-auto p-4 scrollbar-thin">
          {error && (
            <div className="mb-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive">
              {error}
            </div>
          )}
          {activeDashboard ? (
            <DashboardGrid
              dashboard={activeDashboard}
              widgetData={widgetData}
              widgetErrors={widgetErrors}
              workflowRuns={workflowRuns}
              refreshingWidgetId={refreshingWidgetId}
              onRefreshWidget={handleRefreshWidget}
              onLayoutCommit={handleLayoutCommit}
              onAddWidget={handleAddWidget}
            />
          ) : (
            <EmptyState onCreate={() => handleCreate('local_mvp')} onBuild={() => { setChatMode('build'); setIsChatOpen(true); }} />
          )}
        </main>

        <StatusBar dashboardCount={dashboards.length} widgetCount={activeDashboard?.layout.length ?? 0} status={statusMessage} isBusy={isBusy || refreshingWidgetId !== null} />
      </div>

      {isChatOpen && (
        <ChatPanel
          mode={chatMode}
          dashboardId={activeId ?? undefined}
          activeProvider={activeProvider}
          canApplyToDashboard={Boolean(activeDashboard)}
          onClose={() => setIsChatOpen(false)}
          onModeChange={setChatMode}
          onApplyBuildProposal={handleApplyBuildProposal}
        />
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

function EmptyState({ onCreate, onBuild }: { onCreate: () => void; onBuild: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center h-full text-center gap-6">
      <div className="w-20 h-20 rounded-2xl bg-primary/10 flex items-center justify-center">
        <svg className="w-10 h-10 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z" />
        </svg>
      </div>
      <div className="space-y-2">
        <h2 className="text-2xl font-semibold text-foreground">Welcome to Datrina</h2>
        <p className="text-muted-foreground max-w-md text-sm">
          Create a local dashboard wired to a deterministic workflow, or open chat in its current provider-backed mode.
        </p>
      </div>
      <div className="flex gap-3">
        <button onClick={onCreate} className="px-4 py-2 bg-secondary text-secondary-foreground rounded-lg hover:bg-secondary/80 transition-colors text-sm">
          Create Local MVP Dashboard
        </button>
        <button onClick={onBuild} className="px-4 py-2 bg-primary text-primary-foreground rounded-lg hover:bg-primary/90 transition-colors flex items-center gap-2 text-sm">
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
          </svg>
          Build with AI
        </button>
      </div>
    </div>
  );
}

export default App;
