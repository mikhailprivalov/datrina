import { useEffect, useMemo, useState } from 'react';
import {
  mcpApi,
  playgroundApi,
  toolApi,
} from '../../lib/api';
import type {
  HttpRequestArgs,
  MCPServer,
  MCPTool,
  PlaygroundPreset,
} from '../../lib/api';
import { ArgumentsForm } from './ArgumentsForm';
import { ResultPane, safeStringify } from './ResultPane';

interface Props {
  onUseAsWidget: (params: {
    prompt: string;
    sourceLabel: string;
  }) => void;
  onClose: () => void;
}

type SelectedSource =
  | { kind: 'mcp'; serverId: string; tool: MCPTool }
  | { kind: 'http' };

const HTTP_METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'] as const;

export function Playground({ onUseAsWidget, onClose }: Props) {
  const [servers, setServers] = useState<MCPServer[]>([]);
  const [tools, setTools] = useState<MCPTool[]>([]);
  const [presets, setPresets] = useState<PlaygroundPreset[]>([]);
  const [selection, setSelection] = useState<SelectedSource | null>(null);
  const [args, setArgs] = useState<Record<string, unknown>>({});
  const [httpForm, setHttpForm] = useState<HttpRequestArgs>({
    method: 'GET',
    url: '',
  });
  const [headersDraft, setHeadersDraft] = useState<string>('');
  const [bodyDraft, setBodyDraft] = useState<string>('');
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<unknown>(undefined);
  const [durationMs, setDurationMs] = useState<number | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string>('Ready');
  const [widgetNote, setWidgetNote] = useState<string>('');
  const [presetName, setPresetName] = useState<string>('');
  const [loadingTools, setLoadingTools] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const list = await mcpApi.listServers();
        if (cancelled) return;
        setServers(list);
        const presetList = await playgroundApi.listPresets();
        if (cancelled) return;
        setPresets(presetList);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to load sources');
        }
      }
    };
    load();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (servers.length === 0) {
      setTools([]);
      return;
    }
    let cancelled = false;
    const load = async () => {
      setLoadingTools(true);
      try {
        const toolList = await mcpApi.listTools();
        if (cancelled) return;
        setTools(toolList);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : 'Failed to list MCP tools');
        }
      } finally {
        if (!cancelled) setLoadingTools(false);
      }
    };
    load();
    return () => {
      cancelled = true;
    };
  }, [servers.length]);

  const toolsByServer = useMemo(() => {
    const map = new Map<string, MCPTool[]>();
    for (const tool of tools) {
      const list = map.get(tool.server_id) ?? [];
      list.push(tool);
      map.set(tool.server_id, list);
    }
    return map;
  }, [tools]);

  const handleSelectMcpTool = (serverId: string, tool: MCPTool) => {
    setSelection({ kind: 'mcp', serverId, tool });
    setArgs({});
    setResult(undefined);
    setError(null);
    setStatusMessage(`Selected ${tool.name}`);
  };

  const handleSelectHttp = () => {
    setSelection({ kind: 'http' });
    setResult(undefined);
    setError(null);
    setStatusMessage('Selected Custom HTTP');
  };

  const handleApplyPreset = (preset: PlaygroundPreset) => {
    if (preset.tool_kind === 'mcp') {
      const tool = tools.find(
        t => t.server_id === preset.server_id && t.name === preset.tool_name
      );
      if (!tool) {
        setError(`Tool "${preset.tool_name}" is not available on the selected server.`);
        return;
      }
      setSelection({ kind: 'mcp', serverId: preset.server_id ?? '', tool });
      setArgs(
        preset.arguments && typeof preset.arguments === 'object'
          ? (preset.arguments as Record<string, unknown>)
          : {}
      );
    } else {
      const httpArgs = preset.arguments as Partial<HttpRequestArgs> | undefined;
      setSelection({ kind: 'http' });
      setHttpForm({
        method: httpArgs?.method ?? 'GET',
        url: httpArgs?.url ?? '',
      });
      setHeadersDraft(
        httpArgs?.headers ? safeStringify(httpArgs.headers, 2) : ''
      );
      setBodyDraft(httpArgs?.body ? safeStringify(httpArgs.body, 2) : '');
    }
    setPresetName(preset.display_name);
    setResult(undefined);
    setError(null);
    setStatusMessage(`Loaded preset "${preset.display_name}"`);
  };

  const handleRun = async () => {
    if (!selection) return;
    setRunning(true);
    setError(null);
    setStatusMessage('Running...');
    const startedAt = performance.now();
    try {
      if (selection.kind === 'mcp') {
        const value = await mcpApi.callTool(
          selection.serverId,
          selection.tool.name,
          stripUndefined(args)
        );
        setResult(value);
      } else {
        const parsedHeaders = parseOptionalJson(headersDraft);
        const parsedBody = parseOptionalJson(bodyDraft);
        if (headersDraft.trim() && parsedHeaders === undefined) {
          throw new Error('Headers must be valid JSON');
        }
        if (bodyDraft.trim() && parsedBody === undefined) {
          throw new Error('Body must be valid JSON');
        }
        const value = await toolApi.executeHttpRequest({
          method: httpForm.method,
          url: httpForm.url,
          headers: parsedHeaders as Record<string, string> | undefined,
          body: parsedBody,
        });
        setResult(value);
      }
      setStatusMessage('Run finished');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Run failed');
      setStatusMessage('Run failed');
    } finally {
      setRunning(false);
      setDurationMs(performance.now() - startedAt);
    }
  };

  const handleReset = () => {
    if (selection?.kind === 'mcp') {
      setArgs({});
    } else if (selection?.kind === 'http') {
      setHttpForm({ method: 'GET', url: '' });
      setHeadersDraft('');
      setBodyDraft('');
    }
    setResult(undefined);
    setError(null);
    setStatusMessage('Reset');
  };

  const handleSavePreset = async () => {
    if (!selection) return;
    const trimmed = presetName.trim();
    if (!trimmed) {
      setError('Give the preset a name first');
      return;
    }
    try {
      const saved = await (selection.kind === 'mcp'
        ? playgroundApi.savePreset({
            tool_kind: 'mcp',
            server_id: selection.serverId,
            tool_name: selection.tool.name,
            display_name: trimmed,
            arguments: stripUndefined(args),
          })
        : playgroundApi.savePreset({
            tool_kind: 'http',
            tool_name: `${httpForm.method} ${httpForm.url}`,
            display_name: trimmed,
            arguments: {
              method: httpForm.method,
              url: httpForm.url,
              headers: parseOptionalJson(headersDraft),
              body: parseOptionalJson(bodyDraft),
            },
          }));
      setPresets(prev => [saved, ...prev.filter(p => p.id !== saved.id)]);
      setStatusMessage(`Saved "${saved.display_name}"`);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save preset');
    }
  };

  const handleDeletePreset = async (id: string) => {
    try {
      await playgroundApi.deletePreset(id);
      setPresets(prev => prev.filter(p => p.id !== id));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to delete preset');
    }
  };

  const handleUseAsWidget = () => {
    if (!selection || result === undefined) return;
    const sourceLabel = selection.kind === 'mcp'
      ? `${selection.tool.name} (MCP)`
      : `${httpForm.method} ${httpForm.url}`;
    const kindSuggestion = suggestWidgetKind(result);
    const samplePreview = trimSample(safeStringify(result, 2), 4096);
    const argsBlock = selection.kind === 'mcp'
      ? `Source: MCP server "${selection.serverId}" tool "${selection.tool.name}"\nArguments:\n${safeStringify(stripUndefined(args), 2)}`
      : `Source: HTTP ${httpForm.method} ${httpForm.url}\nHeaders:\n${headersDraft || '{}'}\nBody:\n${bodyDraft || 'null'}`;
    const prompt = [
      `Build a ${kindSuggestion} widget for the data below.`,
      '',
      argsBlock,
      '',
      'Data sample (truncated to 4KB):',
      '```json',
      samplePreview,
      '```',
      '',
      widgetNote.trim()
        ? `The widget should display: ${widgetNote.trim()}`
        : 'Pick the most informative fields for the user.',
    ].join('\n');
    onUseAsWidget({ prompt, sourceLabel });
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-12 items-center justify-between border-b border-border bg-card/95 backdrop-blur-sm px-4">
        <div className="flex items-center gap-3">
          <svg className="h-5 w-5 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
          </svg>
          <span className="hidden sm:inline-flex h-5 items-center rounded-sm bg-primary/15 px-1.5 text-[10px] mono font-semibold uppercase tracking-wider text-primary">play</span>
          <h1 className="text-sm font-semibold tracking-tight">Data Playground</h1>
          <span className="text-xs mono text-muted-foreground">{statusMessage}</span>
        </div>
        <button
          onClick={onClose}
          className="rounded-md border border-border bg-card px-2.5 py-1 text-xs mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
        >
          ← Dashboards
        </button>
      </div>
      <div className="grid flex-1 min-h-0 grid-cols-[260px_minmax(0,1fr)_minmax(0,1.2fr)] divide-x divide-border">
        <SourcePane
          servers={servers}
          toolsByServer={toolsByServer}
          selection={selection}
          presets={presets}
          loadingTools={loadingTools}
          onSelectMcp={handleSelectMcpTool}
          onSelectHttp={handleSelectHttp}
          onApplyPreset={handleApplyPreset}
          onDeletePreset={handleDeletePreset}
        />
        <ArgumentsPane
          selection={selection}
          args={args}
          onArgsChange={setArgs}
          httpForm={httpForm}
          onHttpFormChange={setHttpForm}
          headersDraft={headersDraft}
          onHeadersDraftChange={setHeadersDraft}
          bodyDraft={bodyDraft}
          onBodyDraftChange={setBodyDraft}
          running={running}
          onRun={handleRun}
          onReset={handleReset}
          presetName={presetName}
          onPresetNameChange={setPresetName}
          onSavePreset={handleSavePreset}
        />
        <ResultPaneShell
          error={error}
          result={result}
          durationMs={durationMs}
          widgetNote={widgetNote}
          onWidgetNoteChange={setWidgetNote}
          onUseAsWidget={handleUseAsWidget}
          canUseAsWidget={Boolean(selection) && result !== undefined}
        />
      </div>
    </div>
  );
}

function SourcePane({
  servers,
  toolsByServer,
  selection,
  presets,
  loadingTools,
  onSelectMcp,
  onSelectHttp,
  onApplyPreset,
  onDeletePreset,
}: {
  servers: MCPServer[];
  toolsByServer: Map<string, MCPTool[]>;
  selection: SelectedSource | null;
  presets: PlaygroundPreset[];
  loadingTools: boolean;
  onSelectMcp: (serverId: string, tool: MCPTool) => void;
  onSelectHttp: () => void;
  onApplyPreset: (preset: PlaygroundPreset) => void;
  onDeletePreset: (id: string) => void;
}) {
  return (
    <aside className="flex h-full min-h-0 flex-col overflow-y-auto bg-card/40 px-3 py-3 text-sm scrollbar-thin">
      <h2 className="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
        Sources
      </h2>
      <div className="space-y-3 text-xs">
        <button
          onClick={onSelectHttp}
          className={`w-full rounded-md border px-2 py-1.5 text-left transition-colors ${
            selection?.kind === 'http'
              ? 'border-primary/40 bg-primary/10 text-primary'
              : 'border-border hover:bg-muted'
          }`}
        >
          Custom HTTP request
        </button>
        <div className="space-y-1">
          <p className="text-[11px] uppercase tracking-wider text-muted-foreground">
            MCP servers
          </p>
          {servers.length === 0 && (
            <p className="rounded-md border border-dashed border-border px-2 py-2 text-[11px] text-muted-foreground">
              No MCP servers configured. Add one from the sidebar.
            </p>
          )}
          {servers.map(server => {
            const toolList = toolsByServer.get(server.id) ?? [];
            return (
              <div key={server.id} className="rounded-md border border-border">
                <div className="flex items-center justify-between px-2 py-1 text-[11px] text-muted-foreground">
                  <span className="truncate">{server.name}</span>
                  <span className={server.is_enabled ? 'text-neon-lime' : 'text-muted-foreground'}>
                    {server.is_enabled ? 'on' : 'off'}
                  </span>
                </div>
                {loadingTools && toolList.length === 0 && (
                  <p className="px-2 pb-1 text-[11px] text-muted-foreground">Loading…</p>
                )}
                {toolList.length === 0 && !loadingTools && server.is_enabled && (
                  <p className="px-2 pb-1 text-[11px] text-muted-foreground">No tools reported.</p>
                )}
                {toolList.map(tool => (
                  <button
                    key={`${tool.server_id}:${tool.name}`}
                    onClick={() => onSelectMcp(server.id, tool)}
                    className={`flex w-full items-center justify-between gap-2 border-t border-border/50 px-2 py-1 text-left text-[11px] transition-colors ${
                      selection?.kind === 'mcp' && selection.tool.name === tool.name && selection.serverId === server.id
                        ? 'bg-primary/10 text-primary'
                        : 'hover:bg-muted'
                    }`}
                    title={tool.description}
                  >
                    <span className="truncate font-mono">{tool.name}</span>
                  </button>
                ))}
              </div>
            );
          })}
        </div>

        {presets.length > 0 && (
          <div className="space-y-1">
            <p className="text-[11px] uppercase tracking-wider text-muted-foreground">
              Saved presets
            </p>
            {presets.map(preset => (
              <div
                key={preset.id}
                className="flex items-center justify-between gap-2 rounded-md border border-border px-2 py-1"
              >
                <button
                  onClick={() => onApplyPreset(preset)}
                  className="flex-1 truncate text-left text-[11px] hover:text-primary"
                  title={`${preset.tool_kind}: ${preset.tool_name}`}
                >
                  {preset.display_name}
                </button>
                <button
                  onClick={() => onDeletePreset(preset.id)}
                  title="Delete preset"
                  className="text-muted-foreground hover:text-destructive"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}

function ArgumentsPane({
  selection,
  args,
  onArgsChange,
  httpForm,
  onHttpFormChange,
  headersDraft,
  onHeadersDraftChange,
  bodyDraft,
  onBodyDraftChange,
  running,
  onRun,
  onReset,
  presetName,
  onPresetNameChange,
  onSavePreset,
}: {
  selection: SelectedSource | null;
  args: Record<string, unknown>;
  onArgsChange: (next: Record<string, unknown>) => void;
  httpForm: HttpRequestArgs;
  onHttpFormChange: (next: HttpRequestArgs) => void;
  headersDraft: string;
  onHeadersDraftChange: (next: string) => void;
  bodyDraft: string;
  onBodyDraftChange: (next: string) => void;
  running: boolean;
  onRun: () => void;
  onReset: () => void;
  presetName: string;
  onPresetNameChange: (next: string) => void;
  onSavePreset: () => void;
}) {
  if (!selection) {
    return (
      <section className="flex h-full items-center justify-center p-6 text-sm text-muted-foreground">
        Pick an MCP tool or Custom HTTP from the left.
      </section>
    );
  }
  return (
    <section className="flex h-full min-h-0 flex-col overflow-y-auto p-4 scrollbar-thin">
      <header className="mb-3 space-y-1">
        <h2 className="text-sm font-medium">
          {selection.kind === 'mcp' ? selection.tool.name : 'Custom HTTP request'}
        </h2>
        {selection.kind === 'mcp' && selection.tool.description && (
          <p className="text-xs text-muted-foreground">{selection.tool.description}</p>
        )}
      </header>
      <div className="flex-1 min-h-0">
        {selection.kind === 'mcp' ? (
          <ArgumentsForm
            schema={selection.tool.input_schema}
            value={args}
            onChange={onArgsChange}
          />
        ) : (
          <HttpForm
            value={httpForm}
            onChange={onHttpFormChange}
            headersDraft={headersDraft}
            onHeadersDraftChange={onHeadersDraftChange}
            bodyDraft={bodyDraft}
            onBodyDraftChange={onBodyDraftChange}
          />
        )}
      </div>

      <footer className="mt-4 space-y-3 border-t border-border pt-3">
        <div className="flex gap-2">
          <button
            disabled={running || (selection.kind === 'http' && !httpForm.url.trim())}
            onClick={onRun}
            className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {running ? 'Running…' : 'Run'}
          </button>
          <button
            onClick={onReset}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted"
          >
            Reset
          </button>
        </div>
        <div className="flex gap-2">
          <input
            placeholder="Preset name"
            value={presetName}
            onChange={e => onPresetNameChange(e.target.value)}
            className="flex-1 rounded-md border border-border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary/50"
          />
          <button
            onClick={onSavePreset}
            disabled={!presetName.trim()}
            className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Save preset
          </button>
        </div>
      </footer>
    </section>
  );
}

function HttpForm({
  value,
  onChange,
  headersDraft,
  onHeadersDraftChange,
  bodyDraft,
  onBodyDraftChange,
}: {
  value: HttpRequestArgs;
  onChange: (next: HttpRequestArgs) => void;
  headersDraft: string;
  onHeadersDraftChange: (next: string) => void;
  bodyDraft: string;
  onBodyDraftChange: (next: string) => void;
}) {
  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <select
          value={value.method}
          onChange={e => onChange({ ...value, method: e.target.value })}
          className="rounded-md border border-border bg-background px-2 py-1.5 text-xs"
        >
          {HTTP_METHODS.map(m => (
            <option key={m} value={m}>{m}</option>
          ))}
        </select>
        <input
          placeholder="https://api.example.com/path"
          value={value.url}
          onChange={e => onChange({ ...value, url: e.target.value })}
          className="flex-1 rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
        />
      </div>
      <div className="space-y-1">
        <label className="text-xs text-muted-foreground">Headers (JSON object, optional)</label>
        <textarea
          rows={3}
          value={headersDraft}
          onChange={e => onHeadersDraftChange(e.target.value)}
          spellCheck={false}
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
        />
      </div>
      <div className="space-y-1">
        <label className="text-xs text-muted-foreground">Body (JSON, optional)</label>
        <textarea
          rows={4}
          value={bodyDraft}
          onChange={e => onBodyDraftChange(e.target.value)}
          spellCheck={false}
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/50"
        />
      </div>
    </div>
  );
}

function ResultPaneShell({
  error,
  result,
  durationMs,
  widgetNote,
  onWidgetNoteChange,
  onUseAsWidget,
  canUseAsWidget,
}: {
  error: string | null;
  result: unknown;
  durationMs?: number;
  widgetNote: string;
  onWidgetNoteChange: (next: string) => void;
  onUseAsWidget: () => void;
  canUseAsWidget: boolean;
}) {
  return (
    <section className="flex h-full min-h-0 flex-col p-4">
      {error && (
        <div className="mb-2 rounded-md border border-destructive/40 bg-destructive/5 px-2 py-1.5 text-xs text-destructive">
          {error}
        </div>
      )}
      <div className="flex-1 min-h-0">
        <ResultPane result={result} durationMs={durationMs} />
      </div>
      <footer className="mt-3 space-y-2 border-t border-border pt-3">
        <label className="block text-xs text-muted-foreground">
          What should the widget show?
        </label>
        <textarea
          rows={2}
          value={widgetNote}
          onChange={e => onWidgetNoteChange(e.target.value)}
          placeholder="e.g. show stars over time as a line chart"
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-primary/50"
        />
        <button
          onClick={onUseAsWidget}
          disabled={!canUseAsWidget}
          className="w-full rounded-md bg-primary px-3 py-2 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Use as widget →
        </button>
      </footer>
    </section>
  );
}

function stripUndefined<T extends Record<string, unknown>>(input: T): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    if (value !== undefined) out[key] = value;
  }
  return out;
}

function parseOptionalJson(draft: string): unknown {
  if (!draft.trim()) return undefined;
  try {
    return JSON.parse(draft);
  } catch {
    return undefined;
  }
}

function trimSample(text: string, maxBytes: number): string {
  if (text.length <= maxBytes) return text;
  return `${text.slice(0, maxBytes)}\n…[truncated]`;
}

function suggestWidgetKind(result: unknown): string {
  if (result === null || result === undefined) return 'text';
  if (Array.isArray(result)) {
    if (result.length > 0 && isObject(result[0])) {
      const sampleKeys = Object.keys(result[0] as Record<string, unknown>);
      const hasNumeric = sampleKeys.some(k =>
        typeof (result[0] as Record<string, unknown>)[k] === 'number'
      );
      return hasNumeric ? 'chart or table' : 'table';
    }
    return 'table';
  }
  if (isObject(result)) {
    const values = Object.values(result as Record<string, unknown>);
    const numericValues = values.filter(v => typeof v === 'number');
    if (numericValues.length === 1 && values.length <= 2) return 'stat';
    if (numericValues.length > 0) return 'stat or gauge';
    return 'text';
  }
  return 'text';
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
