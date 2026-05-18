// W30: Datasource Workbench panel. Catalog + edit/test/save loop over
// saved DatasourceDefinitions. Uses the same WorkflowEngine path that
// real widgets refresh through; no parallel engine.

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  datasourceApi,
  mcpApi,
  operationsApi,
} from '../../lib/api';
import type {
  CreateDatasourceRequest,
  DatasourceBindingSource,
  DatasourceConsumer,
  DatasourceDefinition,
  DatasourceDefinitionKind,
  DatasourceExportBundle,
  DatasourceImpactPreview,
  DatasourceRunResult,
  MCPServer,
  MCPTool,
  PipelineStep,
  WorkflowSummary,
} from '../../lib/api';
import { validatePipeline } from '../../lib/pipeline/registry';
import { PipelineStudio } from '../pipeline/PipelineStudio';
import { ScheduleEditor } from '../schedule/ScheduleEditor';

interface Props {
  onClose: () => void;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
  /** W35: open the Operations cockpit filtered to this datasource's
   * backing workflow. Optional — older callers may not pass it. */
  onJumpToOperations?: (workflowId: string) => void;
}

interface EditorState {
  name: string;
  description: string;
  kind: DatasourceDefinitionKind;
  toolName: string;
  serverId: string;
  argumentsText: string;
  prompt: string;
  pipeline: PipelineStep[];
  refreshCron: string;
}

const KIND_LABEL: Record<DatasourceDefinitionKind, string> = {
  builtin_tool: 'Built-in tool',
  mcp_tool: 'MCP tool',
  provider_prompt: 'Provider prompt',
};

const BINDING_SOURCE_LABEL: Record<DatasourceBindingSource, string> = {
  build_chat: 'Build Chat',
  workbench: 'Workbench',
  playground: 'Playground',
  import: 'Import',
  manual: 'Manual',
};

const EMPTY_EDITOR: EditorState = {
  name: '',
  description: '',
  kind: 'mcp_tool',
  toolName: '',
  serverId: '',
  argumentsText: '{}',
  prompt: '',
  pipeline: [],
  refreshCron: '',
};

function argumentsFromDef(def: DatasourceDefinition): string {
  if (def.arguments === undefined || def.arguments === null) return '{}';
  return JSON.stringify(def.arguments, null, 2);
}

function editorFromDef(def: DatasourceDefinition): EditorState {
  return {
    name: def.name,
    description: def.description ?? '',
    kind: def.kind,
    toolName: def.tool_name ?? '',
    serverId: def.server_id ?? '',
    argumentsText: argumentsFromDef(def),
    prompt: def.prompt ?? '',
    pipeline: def.pipeline ?? [],
    refreshCron: def.refresh_cron ?? '',
  };
}

function parseArguments(text: string): { value: unknown; error?: string } {
  if (!text.trim()) return { value: undefined };
  try {
    return { value: JSON.parse(text) };
  } catch (err) {
    return { value: undefined, error: err instanceof Error ? err.message : 'Invalid arguments JSON' };
  }
}

function previewValue(value: unknown): string {
  if (value === undefined) return '';
  try {
    const text = JSON.stringify(value, null, 2);
    return text.length > 8_000 ? `${text.slice(0, 8_000)}\n…(truncated)` : text;
  } catch {
    return String(value);
  }
}

function healthBadgeClass(def: DatasourceDefinition): string {
  if (!def.health) {
    return 'bg-muted text-muted-foreground border-border';
  }
  return def.health.last_status === 'ok'
    ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40'
    : 'bg-destructive/15 text-destructive border-destructive/40';
}

function healthLabel(def: DatasourceDefinition): string {
  if (!def.health) return 'never run';
  return def.health.last_status === 'ok' ? 'ok' : 'error';
}

function formatRelative(ts?: number): string {
  if (!ts) return '—';
  const delta = Date.now() - ts;
  if (delta < 60_000) return 'just now';
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;
  return `${Math.floor(delta / 86_400_000)}d ago`;
}

export function Workbench({ onClose, onJumpToWidget, onJumpToOperations }: Props) {
  const [definitions, setDefinitions] = useState<DatasourceDefinition[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState>(EMPTY_EDITOR);
  const [isDirty, setIsDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string>('Ready');
  const [lastRun, setLastRun] = useState<DatasourceRunResult | null>(null);
  const [consumers, setConsumers] = useState<DatasourceConsumer[]>([]);
  const [servers, setServers] = useState<MCPServer[]>([]);
  const [tools, setTools] = useState<MCPTool[]>([]);
  const [newDraft, setNewDraft] = useState(false);
  const [impactPreview, setImpactPreview] = useState<DatasourceImpactPreview | null>(null);
  const [pendingAction, setPendingAction] = useState<'delete' | 'save' | null>(null);
  const [scheduleSummary, setScheduleSummary] = useState<WorkflowSummary | null>(null);

  const loadDefinitions = useCallback(async () => {
    try {
      const list = await datasourceApi.list();
      setDefinitions(list);
      return list;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load datasources');
      return [];
    }
  }, []);

  const loadConsumersFor = useCallback(async (id: string) => {
    try {
      const list = await datasourceApi.listConsumers(id);
      setConsumers(list);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to list consumers');
      setConsumers([]);
    }
  }, []);

  useEffect(() => {
    loadDefinitions();
  }, [loadDefinitions]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [serverList, toolList] = await Promise.all([
          mcpApi.listServers(),
          mcpApi.listTools().catch(() => [] as MCPTool[]),
        ]);
        if (cancelled) return;
        setServers(serverList);
        setTools(toolList);
      } catch (err) {
        if (!cancelled) {
          // Non-fatal: workbench is still usable with manual server/tool ids.
          console.warn('Workbench failed to load MCP catalog:', err);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const selected = useMemo(
    () => definitions.find(d => d.id === selectedId) ?? null,
    [definitions, selectedId],
  );

  useEffect(() => {
    if (newDraft) {
      setEditor(EMPTY_EDITOR);
      setIsDirty(true);
      setLastRun(null);
      setConsumers([]);
      return;
    }
    if (selected) {
      setEditor(editorFromDef(selected));
      setIsDirty(false);
      setLastRun(null);
      loadConsumersFor(selected.id);
    } else {
      setScheduleSummary(null);
    }
  }, [selected, newDraft, loadConsumersFor]);

  // W50: load the backing workflow's schedule summary so the editor can
  // render pause/resume + cadence controls inline with the cron field.
  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    (async () => {
      try {
        const all = await operationsApi.listWorkflowSummaries();
        if (cancelled) return;
        setScheduleSummary(all.find(s => s.id === selected.workflow_id) ?? null);
      } catch {
        if (!cancelled) setScheduleSummary(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const beginNew = () => {
    setNewDraft(true);
    setSelectedId(null);
  };

  const cancelNew = () => {
    setNewDraft(false);
    setEditor(EMPTY_EDITOR);
    setIsDirty(false);
  };

  const updateEditor = <K extends keyof EditorState>(key: K, value: EditorState[K]) => {
    setEditor(prev => ({ ...prev, [key]: value }));
    setIsDirty(true);
  };

  const validateAndBuildPayload = (): CreateDatasourceRequest | null => {
    if (!editor.name.trim()) {
      setError('Name is required');
      return null;
    }
    const args = parseArguments(editor.argumentsText);
    if (args.error) {
      setError(`Arguments JSON: ${args.error}`);
      return null;
    }
    const pipelineError = validatePipeline(editor.pipeline);
    if (pipelineError) {
      setError(`Pipeline step ${pipelineError.index + 1}: ${pipelineError.message}`);
      return null;
    }
    if (editor.kind === 'mcp_tool' && !editor.serverId.trim()) {
      setError('MCP tool datasource requires a server_id');
      return null;
    }
    if ((editor.kind === 'mcp_tool' || editor.kind === 'builtin_tool') && !editor.toolName.trim()) {
      setError('Tool datasource requires a tool_name');
      return null;
    }
    if (editor.kind === 'provider_prompt' && !editor.prompt.trim()) {
      setError('Provider prompt datasource requires a prompt');
      return null;
    }
    setError(null);
    return {
      name: editor.name.trim(),
      description: editor.description.trim() || undefined,
      kind: editor.kind,
      tool_name: editor.toolName.trim() || undefined,
      server_id: editor.serverId.trim() || undefined,
      arguments: args.value,
      prompt: editor.prompt.trim() || undefined,
      pipeline: editor.pipeline,
      refresh_cron: editor.refreshCron.trim() || undefined,
    };
  };

  const handleSave = async () => {
    const payload = validateAndBuildPayload();
    if (!payload) return;
    setBusy(true);
    setStatus('Saving…');
    try {
      let savedId: string;
      if (newDraft || !selected) {
        const saved = await datasourceApi.create(payload);
        savedId = saved.id;
        setNewDraft(false);
      } else {
        const saved = await datasourceApi.update(selected.id, payload);
        savedId = saved.id;
      }
      const list = await loadDefinitions();
      setSelectedId(savedId);
      setIsDirty(false);
      setStatus('Saved');
      // Refresh consumers after save in case the workflow id changed.
      const refreshed = list.find(d => d.id === savedId);
      if (refreshed) {
        await loadConsumersFor(refreshed.id);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save');
      setStatus('Save failed');
    } finally {
      setBusy(false);
    }
  };

  const handleRun = async () => {
    if (!selected) {
      setError('Save the datasource before running a test');
      return;
    }
    if (isDirty) {
      setError('Unsaved changes — save first, then run.');
      return;
    }
    setBusy(true);
    setStatus('Running…');
    setError(null);
    try {
      const result = await datasourceApi.run(selected.id);
      setLastRun(result);
      if (result.status === 'error') {
        setStatus(`Run failed${result.error ? `: ${result.error}` : ''}`);
      } else {
        setStatus(`Ran in ${result.duration_ms}ms`);
      }
      await loadDefinitions();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Test-run failed');
      setStatus('Run errored');
    } finally {
      setBusy(false);
    }
  };

  const requestDelete = async () => {
    if (!selected) return;
    setBusy(true);
    setStatus('Computing impact preview…');
    try {
      const preview = await datasourceApi.previewImpact(selected.id);
      setImpactPreview(preview);
      setPendingAction('delete');
      if (preview.consumers.length === 0) {
        setStatus('No consumers — confirm to delete.');
      } else {
        setStatus(`${preview.consumers.length} consumer(s) — review before deleting.`);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Impact preview failed');
      setStatus('Preview failed');
    } finally {
      setBusy(false);
    }
  };

  const requestSave = async () => {
    if (newDraft || !selected) {
      await handleSave();
      return;
    }
    setBusy(true);
    setStatus('Computing impact preview…');
    try {
      const preview = await datasourceApi.previewImpact(selected.id);
      if (preview.consumers.length === 0) {
        setImpactPreview(null);
        setPendingAction(null);
        await handleSave();
        return;
      }
      setImpactPreview(preview);
      setPendingAction('save');
      setStatus(`Saving will affect ${preview.consumers.length} consumer(s) — confirm.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Impact preview failed');
      setStatus('Preview failed');
    } finally {
      setBusy(false);
    }
  };

  const cancelImpactPreview = () => {
    setImpactPreview(null);
    setPendingAction(null);
    setStatus('Cancelled');
  };

  const confirmDelete = async () => {
    if (!selected) return;
    setImpactPreview(null);
    setPendingAction(null);
    setBusy(true);
    setStatus('Deleting…');
    try {
      await datasourceApi.remove(selected.id);
      await loadDefinitions();
      setSelectedId(null);
      setLastRun(null);
      setConsumers([]);
      setStatus('Deleted');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete');
      setStatus('Delete failed');
    } finally {
      setBusy(false);
    }
  };

  const confirmSave = async () => {
    setImpactPreview(null);
    setPendingAction(null);
    await handleSave();
  };

  const handleDuplicate = async () => {
    if (!selected) return;
    setBusy(true);
    setStatus('Duplicating…');
    try {
      const copy = await datasourceApi.duplicate(selected.id);
      const list = await loadDefinitions();
      const fresh = list.find(d => d.id === copy.id);
      setSelectedId(fresh?.id ?? copy.id);
      setStatus('Duplicated');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to duplicate');
      setStatus('Duplicate failed');
    } finally {
      setBusy(false);
    }
  };

  const handleExport = async () => {
    setBusy(true);
    setStatus('Exporting…');
    try {
      const bundle = await datasourceApi.export(selected ? [selected.id] : []);
      downloadJson(bundle, `datrina-datasources-${Date.now()}.json`);
      setStatus(`Exported ${bundle.definitions.length} definition(s)`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Export failed');
      setStatus('Export failed');
    } finally {
      setBusy(false);
    }
  };

  const handleImportClick = async () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = 'application/json';
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      setBusy(true);
      setStatus('Importing…');
      try {
        const text = await file.text();
        const bundle = JSON.parse(text) as DatasourceExportBundle;
        const result = await datasourceApi.import(bundle, false);
        await loadDefinitions();
        const summary = `imported ${result.imported}, skipped ${result.skipped}` +
          (result.errors.length > 0 ? `, ${result.errors.length} error(s)` : '');
        setStatus(summary);
        if (result.errors.length > 0) {
          setError(result.errors.join('\n'));
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Import failed');
        setStatus('Import failed');
      } finally {
        setBusy(false);
      }
    };
    input.click();
  };

  const showProviderPromptField = editor.kind === 'provider_prompt';
  const showToolFields = editor.kind === 'mcp_tool' || editor.kind === 'builtin_tool';
  const showServerField = editor.kind === 'mcp_tool';

  return (
    <div className="absolute inset-0 z-30 flex flex-col bg-background">
      <header className="flex items-center justify-between border-b border-border px-4 py-3">
        <div className="flex items-center gap-3">
          <span className="text-[10px] mono uppercase tracking-[0.2em] text-muted-foreground">// Workbench</span>
          <h2 className="text-lg font-semibold">Datasources</h2>
          <span className="text-xs text-muted-foreground">{definitions.length} total</span>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={handleImportClick}
            disabled={busy}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors"
          >
            Import…
          </button>
          <button
            type="button"
            onClick={handleExport}
            disabled={busy || definitions.length === 0}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors disabled:opacity-50"
          >
            Export {selected ? 'selected' : 'all'}
          </button>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors"
          >
            Close
          </button>
        </div>
      </header>

      <div className="flex flex-1 min-h-0">
        {/* Catalog */}
        <aside className="w-72 flex flex-col border-r border-border bg-card/40">
          <div className="p-3 border-b border-border">
            <button
              type="button"
              onClick={beginNew}
              className="w-full rounded-md border border-primary/40 bg-primary/10 px-3 py-2 text-sm text-primary hover:bg-primary/20 transition-colors"
            >
              + New datasource
            </button>
          </div>
          <div className="flex-1 overflow-y-auto scrollbar-thin">
            {definitions.length === 0 && !newDraft && (
              <div className="p-4 text-xs text-muted-foreground">
                No saved datasources yet. Use the Workbench to capture a reusable source
                or import a JSON bundle.
              </div>
            )}
            {definitions.map(def => {
              const isActive = def.id === selectedId && !newDraft;
              return (
                <button
                  key={def.id}
                  type="button"
                  onClick={() => { setNewDraft(false); setSelectedId(def.id); }}
                  className={`w-full text-left px-3 py-2 border-l-2 transition-colors ${
                    isActive
                      ? 'border-l-primary bg-primary/5'
                      : 'border-l-transparent hover:bg-muted/30'
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate font-medium text-sm">{def.name}</span>
                    <span className={`shrink-0 rounded-full border px-1.5 text-[10px] mono uppercase tracking-wider ${healthBadgeClass(def)}`}>
                      {healthLabel(def)}
                    </span>
                  </div>
                  <div className="mt-0.5 flex items-center justify-between text-[10px] mono uppercase tracking-wider text-muted-foreground">
                    <span>{KIND_LABEL[def.kind]}</span>
                    <span>{def.health?.consumer_count ?? 0} consumer(s)</span>
                  </div>
                  {def.originated_external_source_id && (
                    <div className="mt-0.5 text-[10px] mono uppercase tracking-wider text-primary">
                      from catalog: {def.originated_external_source_id}
                    </div>
                  )}
                  <div className="mt-0.5 text-[10px] text-muted-foreground">
                    Last run {formatRelative(def.health?.last_run_at)}
                  </div>
                </button>
              );
            })}
            {newDraft && (
              <div className="w-full text-left px-3 py-2 border-l-2 border-l-accent bg-accent/5">
                <span className="text-sm font-medium">(new draft)</span>
                <div className="text-[10px] mono uppercase tracking-wider text-muted-foreground">unsaved</div>
              </div>
            )}
          </div>
        </aside>

        {/* Editor */}
        <section className="flex-1 min-w-0 overflow-y-auto scrollbar-thin">
          {!selected && !newDraft ? (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground p-6 text-center">
              Pick a datasource on the left or start a new one. Definitions back the
              same workflow engine real widgets use — saving them gives the UI a
              reusable, inspectable surface.
            </div>
          ) : (
            <div className="space-y-4 p-5 max-w-3xl">
              {error && (
                <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive whitespace-pre-line">
                  {error}
                </div>
              )}

              <div className="grid grid-cols-2 gap-3">
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">Name</span>
                  <input
                    type="text"
                    value={editor.name}
                    onChange={e => updateEditor('name', e.target.value)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                    placeholder="e.g. github_open_issues"
                  />
                </label>
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">Kind</span>
                  <select
                    value={editor.kind}
                    onChange={e => updateEditor('kind', e.target.value as DatasourceDefinitionKind)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                  >
                    <option value="mcp_tool">MCP tool</option>
                    <option value="builtin_tool">Built-in tool</option>
                    <option value="provider_prompt">Provider prompt</option>
                  </select>
                </label>
              </div>

              <label className="flex flex-col gap-1 text-xs">
                <span className="mono uppercase tracking-wider text-muted-foreground">Description</span>
                <input
                  type="text"
                  value={editor.description}
                  onChange={e => updateEditor('description', e.target.value)}
                  className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                  placeholder="Optional, shown in the catalog"
                />
              </label>

              {showServerField && (
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">MCP server</span>
                  <select
                    value={editor.serverId}
                    onChange={e => updateEditor('serverId', e.target.value)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                  >
                    <option value="">— pick a server —</option>
                    {servers.map(s => (
                      <option key={s.id} value={s.id}>
                        {s.name} ({s.transport})
                      </option>
                    ))}
                  </select>
                </label>
              )}

              {showToolFields && (
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">Tool name</span>
                  <input
                    list="datrina-workbench-tools"
                    type="text"
                    value={editor.toolName}
                    onChange={e => updateEditor('toolName', e.target.value)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono"
                    placeholder="e.g. read_file"
                  />
                  <datalist id="datrina-workbench-tools">
                    {tools
                      .filter(t => !editor.serverId || t.server_id === editor.serverId)
                      .map(t => (
                        <option key={`${t.server_id}:${t.name}`} value={t.name}>
                          {t.description}
                        </option>
                      ))}
                  </datalist>
                </label>
              )}

              {showProviderPromptField && (
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">Prompt</span>
                  <textarea
                    rows={4}
                    value={editor.prompt}
                    onChange={e => updateEditor('prompt', e.target.value)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono"
                    placeholder="Instruction sent to the active provider"
                  />
                </label>
              )}

              {showToolFields && (
                <label className="flex flex-col gap-1 text-xs">
                  <span className="mono uppercase tracking-wider text-muted-foreground">Arguments (JSON)</span>
                  <textarea
                    rows={5}
                    value={editor.argumentsText}
                    onChange={e => updateEditor('argumentsText', e.target.value)}
                    className="rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono"
                  />
                </label>
              )}

              <div className="space-y-1">
                <PipelineStudio
                  steps={editor.pipeline}
                  onChange={next => updateEditor('pipeline', next)}
                  sample={lastRun?.raw_source}
                  sampleLabel="Last test-run source"
                />
                <span className="text-[10px] text-muted-foreground">
                  Replay uses the last test-run's raw source as the deterministic
                  sample. Click <strong>Test-run</strong> to refresh it. The W23
                  Debug view still shows the full provider/MCP-aware trace.
                </span>
              </div>

              <label className="flex flex-col gap-1 text-xs">
                <span className="mono uppercase tracking-wider text-muted-foreground">Refresh cron (optional)</span>
                <input
                  type="text"
                  value={editor.refreshCron}
                  onChange={e => updateEditor('refreshCron', e.target.value)}
                  className="rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono"
                  placeholder="6-field cron, e.g. 0 */5 * * * *"
                />
                <span className="text-[10px] text-muted-foreground">
                  Saved with the definition. Use the schedule controls below to
                  pause without losing the cron or to apply cadence presets
                  through the validated path.
                </span>
              </label>

              {scheduleSummary && !newDraft && (
                <div className="rounded-md border border-border bg-card/40 p-3">
                  <p className="mono text-[10px] uppercase tracking-[0.16em] text-primary mb-2">
                    // schedule
                  </p>
                  <ScheduleEditor
                    summary={scheduleSummary}
                    onChange={next => setScheduleSummary(next)}
                  />
                </div>
              )}

              <div className="flex items-center justify-between border-t border-border pt-3">
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <span>{status}</span>
                  {isDirty && <span className="text-amber-400">• unsaved</span>}
                </div>
                <div className="flex items-center gap-2">
                  {newDraft && (
                    <button
                      type="button"
                      onClick={cancelNew}
                      disabled={busy}
                      className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors"
                    >
                      Cancel
                    </button>
                  )}
                  {!newDraft && selected && (
                    <>
                      <button
                        type="button"
                        onClick={handleDuplicate}
                        disabled={busy}
                        className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors"
                      >
                        Duplicate
                      </button>
                      <button
                        type="button"
                        onClick={requestDelete}
                        disabled={busy}
                        className="rounded-md border border-destructive/40 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 transition-colors"
                      >
                        Delete…
                      </button>
                    </>
                  )}
                  <button
                    type="button"
                    onClick={handleRun}
                    disabled={busy || newDraft || !selected}
                    className="rounded-md border border-accent/40 bg-accent/10 px-3 py-1.5 text-xs text-accent hover:bg-accent/20 transition-colors disabled:opacity-50"
                  >
                    Test-run
                  </button>
                  {onJumpToOperations && selected && !newDraft && (
                    <button
                      type="button"
                      onClick={() => onJumpToOperations(selected.workflow_id)}
                      className="rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground hover:border-primary/40 hover:text-foreground transition-colors"
                      title="Open scheduled runs in Operations"
                    >
                      Runs…
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={requestSave}
                    disabled={busy || !isDirty}
                    className="rounded-md border border-primary/40 bg-primary/15 px-3 py-1.5 text-xs text-primary hover:bg-primary/25 transition-colors disabled:opacity-50"
                  >
                    {newDraft ? 'Create' : 'Save'}
                  </button>
                </div>
              </div>

              {lastRun && (
                <div className="space-y-3 rounded-md border border-border bg-card/40 p-3">
                  <div className="flex items-center justify-between text-xs">
                    <span className="mono uppercase tracking-wider text-muted-foreground">Run result</span>
                    <span className={lastRun.status === 'ok' ? 'text-emerald-400' : 'text-destructive'}>
                      {lastRun.status} · {lastRun.duration_ms}ms · {lastRun.pipeline_steps} pipeline step(s)
                    </span>
                  </div>
                  {lastRun.error && (
                    <pre className="text-[11px] text-destructive whitespace-pre-wrap">{lastRun.error}</pre>
                  )}
                  <details>
                    <summary className="cursor-pointer text-xs text-muted-foreground">Raw source</summary>
                    <pre className="mt-1 max-h-64 overflow-auto rounded bg-background/60 p-2 text-[11px] font-mono">
                      {previewValue(lastRun.raw_source)}
                    </pre>
                  </details>
                  <details open>
                    <summary className="cursor-pointer text-xs text-muted-foreground">Final value (after pipeline)</summary>
                    <pre className="mt-1 max-h-64 overflow-auto rounded bg-background/60 p-2 text-[11px] font-mono">
                      {previewValue(lastRun.final_value)}
                    </pre>
                  </details>
                </div>
              )}

              {impactPreview && pendingAction && (
                <div className="space-y-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3">
                  <div className="flex items-center justify-between text-xs">
                    <span className="mono uppercase tracking-wider text-amber-300">
                      // Impact preview ({pendingAction})
                    </span>
                    <span className="text-amber-200">
                      {impactPreview.consumers.length} consumer(s) · {impactPreview.legacy_consumer_count} legacy
                    </span>
                  </div>
                  <p className="text-[11px] text-amber-200">
                    {pendingAction === 'delete'
                      ? impactPreview.consumers.length === 0
                        ? 'No widgets are bound — delete is safe.'
                        : 'Bound widgets will block the delete. Unbind them first or accept the failure.'
                      : 'Saving rebuilds the backing workflow. All listed consumers (including legacy) refresh against the new shape.'}
                  </p>
                  {impactPreview.consumers.length > 0 && (
                    <ul className="space-y-1 max-h-40 overflow-y-auto">
                      {impactPreview.consumers.map(c => (
                        <ConsumerRow
                          key={`${c.dashboard_id}-${c.widget_id}-impact`}
                          consumer={c}
                          onJumpToWidget={onJumpToWidget}
                        />
                      ))}
                    </ul>
                  )}
                  <div className="flex items-center justify-end gap-2 pt-1">
                    <button
                      type="button"
                      onClick={cancelImpactPreview}
                      disabled={busy}
                      className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted/40 transition-colors"
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      onClick={pendingAction === 'delete' ? confirmDelete : confirmSave}
                      disabled={busy}
                      className={`rounded-md border px-3 py-1.5 text-xs transition-colors ${
                        pendingAction === 'delete'
                          ? 'border-destructive/40 text-destructive hover:bg-destructive/10'
                          : 'border-primary/40 bg-primary/15 text-primary hover:bg-primary/25'
                      }`}
                    >
                      Confirm {pendingAction}
                    </button>
                  </div>
                </div>
              )}

              {selected && (
                <div className="space-y-2 rounded-md border border-border bg-card/40 p-3">
                  <div className="flex items-center justify-between text-xs">
                    <span className="mono uppercase tracking-wider text-muted-foreground">
                      Consumer widgets
                    </span>
                    <span className="text-muted-foreground">{consumers.length}</span>
                  </div>
                  {consumers.length === 0 ? (
                    <p className="text-xs text-muted-foreground">
                      No widgets are bound to this datasource yet. Build Chat or manual bindings will appear here.
                    </p>
                  ) : (
                    <ul className="space-y-1">
                      {consumers.map(c => (
                        <ConsumerRow
                          key={`${c.dashboard_id}-${c.widget_id}`}
                          consumer={c}
                          onJumpToWidget={onJumpToWidget}
                        />
                      ))}
                    </ul>
                  )}
                </div>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function ConsumerRow({
  consumer,
  onJumpToWidget,
}: {
  consumer: DatasourceConsumer;
  onJumpToWidget?: (dashboardId: string, widgetId: string) => void;
}) {
  const provenanceBadge = consumer.binding_source
    ? BINDING_SOURCE_LABEL[consumer.binding_source]
    : consumer.explicit_binding
      ? 'bound'
      : 'legacy';
  const provenanceTone = consumer.explicit_binding
    ? 'bg-primary/10 text-primary border-primary/30'
    : 'bg-muted text-muted-foreground border-border';
  return (
    <li className="flex items-center justify-between gap-2 rounded border border-border/60 bg-background/40 px-2 py-1.5 text-xs">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate font-medium">{consumer.widget_title}</span>
          <span
            className={`shrink-0 rounded-full border px-1.5 text-[9px] mono uppercase tracking-wider ${provenanceTone}`}
          >
            {provenanceBadge}
          </span>
        </div>
        <div className="truncate text-[10px] mono uppercase tracking-wider text-muted-foreground">
          {consumer.dashboard_name} · {consumer.widget_kind} · {consumer.output_key}
        </div>
        <div className="flex items-center gap-2 text-[10px] text-muted-foreground">
          {consumer.bound_at && <span>bound {formatRelative(consumer.bound_at)}</span>}
          {(consumer.tail_step_count ?? 0) > 0 && (
            <span className="text-accent">+{consumer.tail_step_count} tail step(s)</span>
          )}
        </div>
      </div>
      {onJumpToWidget && (
        <button
          type="button"
          onClick={() => onJumpToWidget(consumer.dashboard_id, consumer.widget_id)}
          className="shrink-0 rounded border border-border px-2 py-1 text-[11px] hover:bg-muted/40 transition-colors"
        >
          Open
        </button>
      )}
    </li>
  );
}

function downloadJson(value: unknown, filename: string) {
  const blob = new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
