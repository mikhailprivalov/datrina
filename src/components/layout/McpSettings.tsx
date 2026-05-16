import { useEffect, useMemo, useState } from 'react';
import { mcpApi } from '../../lib/api';
import type { MCPServer, MCPTool } from '../../lib/api';

interface Props {
  onClose: () => void;
}

interface DraftServer {
  id: string;
  name: string;
  command: string;
  argsText: string;
  envText: string;
  is_enabled: boolean;
}

const EMPTY_DRAFT: DraftServer = {
  id: '',
  name: '',
  command: '',
  argsText: '',
  envText: '',
  is_enabled: true,
};

export function McpSettings({ onClose }: Props) {
  const [servers, setServers] = useState<MCPServer[]>([]);
  const [tools, setTools] = useState<MCPTool[]>([]);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftServer | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [list, toolList] = await Promise.all([mcpApi.listServers(), mcpApi.listTools()]);
      setServers(list);
      setTools(toolList);
    } catch (err) {
      setError(formatError(err));
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const toolsByServer = useMemo(() => {
    const map = new Map<string, MCPTool[]>();
    for (const t of tools) {
      const arr = map.get(t.server_id) ?? [];
      arr.push(t);
      map.set(t.server_id, arr);
    }
    return map;
  }, [tools]);

  const startCreate = () => setDraft({ ...EMPTY_DRAFT, id: suggestId() });
  const startEdit = (server: MCPServer) => setDraft({
    id: server.id,
    name: server.name,
    command: server.command ?? '',
    argsText: (server.args ?? []).join('\n'),
    envText: Object.entries(server.env ?? {}).map(([k, v]) => `${k}=${v}`).join('\n'),
    is_enabled: server.is_enabled,
  });

  const handleSave = async () => {
    if (!draft) return;
    const name = draft.name.trim();
    const command = draft.command.trim();
    if (!name || !command) {
      setError('Name and command are required.');
      return;
    }
    const args = parseLines(draft.argsText);
    const env = parseEnv(draft.envText);
    if (env === null) {
      setError('Env lines must be KEY=VALUE.');
      return;
    }
    const payload: MCPServer = {
      id: draft.id || suggestId(),
      name,
      transport: 'stdio',
      is_enabled: draft.is_enabled,
      command,
      args: args.length ? args : undefined,
      env: Object.keys(env).length ? env : undefined,
      url: undefined,
    };
    setBusyId(payload.id);
    setError(null);
    try {
      await mcpApi.addServer(payload);
      setStatus(`Saved ${payload.name}`);
      setDraft(null);
      await refresh();
      if (payload.is_enabled) {
        try {
          await mcpApi.enableServer(payload.id);
          setStatus(`Connected ${payload.name}`);
          await refresh();
        } catch (err) {
          setError(`Saved, but failed to connect: ${formatError(err)}`);
        }
      }
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleEnable = async (server: MCPServer) => {
    setBusyId(server.id);
    setError(null);
    try {
      await mcpApi.enableServer(server.id);
      setStatus(`Connected ${server.name}`);
      await refresh();
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleDisable = async (server: MCPServer) => {
    setBusyId(server.id);
    setError(null);
    try {
      await mcpApi.disableServer(server.id);
      setStatus(`Disabled ${server.name}`);
      await refresh();
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleRemove = async (server: MCPServer) => {
    if (!window.confirm(`Remove MCP server "${server.name}"? This disconnects and forgets the config.`)) return;
    setBusyId(server.id);
    setError(null);
    try {
      await mcpApi.removeServer(server.id);
      setStatus(`Removed ${server.name}`);
      await refresh();
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleReconnect = async () => {
    setBusyId('__all');
    setError(null);
    try {
      await mcpApi.reconnectEnabledServers();
      setStatus('Reconnected enabled servers');
      await refresh();
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 backdrop-blur-sm">
      <div className="flex max-h-[85vh] w-[min(92vw,56rem)] flex-col rounded-xl border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-3">
          <div>
            <h2 className="text-sm font-semibold text-foreground">MCP servers</h2>
            <p className="text-[11px] text-muted-foreground">Configure stdio MCP servers reachable from chat and workflows.</p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleReconnect}
              disabled={busyId === '__all'}
              className="rounded-md border border-border px-2.5 py-1 text-xs hover:bg-muted disabled:opacity-50"
            >
              {busyId === '__all' ? 'Reconnecting...' : 'Reconnect enabled'}
            </button>
            <button onClick={startCreate} className="rounded-md bg-primary px-3 py-1 text-xs text-primary-foreground hover:bg-primary/90">
              Add server
            </button>
            <button onClick={onClose} className="p-1 rounded hover:bg-muted text-muted-foreground">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        </div>

        {(status || error) && (
          <div className={`border-b border-border px-5 py-2 text-xs ${error ? 'text-destructive' : 'text-emerald-600 dark:text-emerald-400'}`}>
            {error ?? status}
          </div>
        )}

        <div className="flex-1 overflow-auto p-5 space-y-3">
          {draft && (
            <DraftCard
              draft={draft}
              onChange={setDraft}
              onCancel={() => setDraft(null)}
              onSave={handleSave}
              busy={busyId === draft.id}
            />
          )}
          {servers.length === 0 && !draft && (
            <div className="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground">
              No MCP servers yet. Click <span className="font-medium">Add server</span> to register a stdio command.
            </div>
          )}
          {servers.map(server => (
            <ServerCard
              key={server.id}
              server={server}
              tools={toolsByServer.get(server.id) ?? []}
              busy={busyId === server.id}
              onEnable={() => handleEnable(server)}
              onDisable={() => handleDisable(server)}
              onRemove={() => handleRemove(server)}
              onEdit={() => startEdit(server)}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function ServerCard({
  server,
  tools,
  busy,
  onEnable,
  onDisable,
  onRemove,
  onEdit,
}: {
  server: MCPServer;
  tools: MCPTool[];
  busy: boolean;
  onEnable: () => void;
  onDisable: () => void;
  onRemove: () => void;
  onEdit: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const connected = tools.length > 0;
  const status = !server.is_enabled ? 'disabled' : connected ? 'connected' : 'idle';
  const statusTone = {
    connected: 'text-emerald-600 dark:text-emerald-400 bg-emerald-500/15',
    idle: 'text-blue-700 dark:text-blue-400 bg-blue-500/15',
    disabled: 'text-muted-foreground bg-muted',
  }[status];
  return (
    <div className="rounded-lg border border-border bg-background/50">
      <div className="flex items-start gap-3 p-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium truncate">{server.name}</span>
            <span className={`rounded-full px-1.5 py-0.5 text-[10px] uppercase tracking-wide ${statusTone}`}>{status}</span>
            <span className="text-[10px] text-muted-foreground">{server.transport}</span>
          </div>
          <p className="mt-0.5 truncate text-[11px] text-muted-foreground font-mono">
            {server.command} {(server.args ?? []).join(' ')}
          </p>
          <p className="mt-0.5 text-[10px] text-muted-foreground">id: {server.id} · {tools.length} tool(s)</p>
        </div>
        <div className="flex items-center gap-1">
          {server.is_enabled ? (
            <button onClick={onDisable} disabled={busy} className="rounded-md border border-border px-2 py-1 text-[11px] hover:bg-muted disabled:opacity-50">
              {busy ? '...' : 'Disable'}
            </button>
          ) : (
            <button onClick={onEnable} disabled={busy} className="rounded-md bg-primary px-2 py-1 text-[11px] text-primary-foreground hover:bg-primary/90 disabled:opacity-50">
              {busy ? '...' : 'Enable & connect'}
            </button>
          )}
          <button onClick={onEdit} disabled={busy} className="rounded-md border border-border px-2 py-1 text-[11px] hover:bg-muted disabled:opacity-50">
            Edit
          </button>
          <button onClick={onRemove} disabled={busy} className="rounded-md border border-destructive/30 text-destructive px-2 py-1 text-[11px] hover:bg-destructive/10 disabled:opacity-50">
            Remove
          </button>
        </div>
      </div>
      {tools.length > 0 && (
        <div className="border-t border-border/50 px-3 py-2">
          <button
            onClick={() => setExpanded(v => !v)}
            className="text-[11px] text-muted-foreground hover:text-foreground"
          >
            {expanded ? 'Hide tools' : `Show ${tools.length} tool(s)`}
          </button>
          {expanded && (
            <ul className="mt-2 space-y-1.5">
              {tools.map(tool => (
                <li key={`${tool.server_id}:${tool.name}`} className="rounded-md bg-card border border-border/50 p-2 text-[11px]">
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-medium font-mono">{tool.name}</span>
                  </div>
                  <p className="mt-0.5 text-muted-foreground">{tool.description || 'No description.'}</p>
                  {Object.keys(tool.input_schema ?? {}).length > 0 && (
                    <details className="mt-1">
                      <summary className="text-[10px] text-muted-foreground cursor-pointer">Input schema</summary>
                      <pre className="mt-1 max-h-40 overflow-auto rounded bg-background/70 p-2 font-mono text-[10px]">
                        {JSON.stringify(tool.input_schema, null, 2)}
                      </pre>
                    </details>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function DraftCard({
  draft,
  onChange,
  onCancel,
  onSave,
  busy,
}: {
  draft: DraftServer;
  onChange: (draft: DraftServer) => void;
  onCancel: () => void;
  onSave: () => void;
  busy: boolean;
}) {
  return (
    <div className="rounded-lg border border-primary/40 bg-primary/5 p-4 space-y-3">
      <div className="grid grid-cols-2 gap-3">
        <Field label="Server name">
          <input
            value={draft.name}
            onChange={e => onChange({ ...draft, name: e.target.value })}
            placeholder="Yandex MCP store proxy"
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-primary/40"
          />
        </Field>
        <Field label="ID (stable)">
          <input
            value={draft.id}
            onChange={e => onChange({ ...draft, id: e.target.value })}
            placeholder="prompt-yandex-mcp-store-proxy"
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
          />
        </Field>
      </div>
      <Field label="Command (executable path)">
        <input
          value={draft.command}
          onChange={e => onChange({ ...draft, command: e.target.value })}
          placeholder="/Users/me/.wizard/yandex-mcp-store-proxy"
          className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
        />
      </Field>
      <Field label="Arguments (one per line)">
        <textarea
          value={draft.argsText}
          onChange={e => onChange({ ...draft, argsText: e.target.value })}
          placeholder={'-O\ny1__token\n--endpoint\nmcp.example.net/ws'}
          rows={4}
          className="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
        />
      </Field>
      <Field label="Environment (KEY=VALUE per line)">
        <textarea
          value={draft.envText}
          onChange={e => onChange({ ...draft, envText: e.target.value })}
          placeholder="LOG_LEVEL=debug"
          rows={2}
          className="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
        />
      </Field>
      <div className="flex items-center justify-between">
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <input
            type="checkbox"
            checked={draft.is_enabled}
            onChange={e => onChange({ ...draft, is_enabled: e.target.checked })}
          />
          Enable and connect immediately
        </label>
        <div className="flex gap-2">
          <button onClick={onCancel} disabled={busy} className="rounded-md border border-border px-3 py-1.5 text-xs hover:bg-muted disabled:opacity-50">
            Cancel
          </button>
          <button onClick={onSave} disabled={busy} className="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90 disabled:opacity-50">
            {busy ? 'Saving...' : 'Save server'}
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="block text-[11px] uppercase tracking-wide text-muted-foreground mb-1">{label}</span>
      {children}
    </label>
  );
}

function parseLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map(s => s.trim())
    .filter(s => s.length > 0);
}

function parseEnv(text: string): Record<string, string> | null {
  const lines = parseLines(text);
  const env: Record<string, string> = {};
  for (const line of lines) {
    const eq = line.indexOf('=');
    if (eq < 1) return null;
    const key = line.slice(0, eq).trim();
    const value = line.slice(eq + 1);
    if (!key) return null;
    env[key] = value;
  }
  return env;
}

function suggestId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID();
  return `mcp-${Date.now()}`;
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
