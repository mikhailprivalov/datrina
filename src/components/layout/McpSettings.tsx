import { useEffect, useMemo, useState } from 'react';
import { mcpApi, toolApi } from '../../lib/api';
import type { MCPServer, MCPTool } from '../../lib/api';

interface Props {
  onClose: () => void;
}

type DraftTransport = 'stdio' | 'http';

interface DraftServer {
  id: string;
  name: string;
  transport: DraftTransport;
  command: string;
  argsText: string;
  envText: string;
  url: string;
  is_enabled: boolean;
}

const EMPTY_DRAFT: DraftServer = {
  id: '',
  name: '',
  transport: 'stdio',
  command: '',
  argsText: '',
  envText: '',
  url: '',
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
    transport: server.transport,
    command: server.command ?? '',
    argsText: (server.args ?? []).join('\n'),
    envText: Object.entries(server.env ?? {}).map(([k, v]) => `${k}=${v}`).join('\n'),
    url: server.url ?? '',
    is_enabled: server.is_enabled,
  });

  const handleSave = async () => {
    if (!draft) return;
    const name = draft.name.trim();
    if (!name) {
      setError('Server name is required.');
      return;
    }
    let payload: MCPServer;
    if (draft.transport === 'stdio') {
      const command = draft.command.trim();
      if (!command) {
        setError('Command is required for stdio MCP servers.');
        return;
      }
      const args = parseLines(draft.argsText);
      const env = parseEnv(draft.envText);
      if (env === null) {
        setError('Env lines must be KEY=VALUE.');
        return;
      }
      payload = {
        id: draft.id || suggestId(),
        name,
        transport: 'stdio',
        is_enabled: draft.is_enabled,
        command,
        args: args.length ? args : undefined,
        env: Object.keys(env).length ? env : undefined,
        url: undefined,
      };
    } else {
      const url = draft.url.trim();
      if (!/^https?:\/\//i.test(url)) {
        setError('URL must start with http:// or https://.');
        return;
      }
      payload = {
        id: draft.id || suggestId(),
        name,
        transport: 'http',
        is_enabled: draft.is_enabled,
        command: undefined,
        args: undefined,
        env: undefined,
        url,
      };
    }
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 backdrop-blur-sm">
      <div className="flex max-h-[85vh] w-[min(92vw,56rem)] flex-col rounded-md border border-border bg-card shadow-2xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-3 bg-muted/20">
          <div>
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// mcp</p>
            <h2 className="mt-0.5 text-sm font-semibold text-foreground tracking-tight">MCP servers</h2>
            <p className="text-[11px] text-muted-foreground">Configure MCP servers (stdio or HTTP) reachable from chat and workflows.</p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleReconnect}
              disabled={busyId === '__all'}
              className="rounded-md border border-border bg-card px-2.5 py-1 text-xs mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 disabled:opacity-50 transition-colors"
            >
              {busyId === '__all' ? 'Reconnecting…' : 'Reconnect enabled'}
            </button>
            <button onClick={startCreate} className="rounded-md bg-primary border border-primary px-3 py-1 text-xs mono uppercase tracking-wider font-semibold text-primary-foreground hover:glow-primary transition-all">
              + Add server
            </button>
            <button onClick={onClose} className="p-1 rounded hover:bg-muted text-muted-foreground">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        </div>

        {(status || error) && (
          <div className={`border-b border-border px-5 py-2 text-xs ${error ? 'text-destructive' : 'text-neon-lime'}`}>
            {error ?? status}
          </div>
        )}

        <HttpDefaultsRow onError={setError} onStatus={setStatus} />

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
              No MCP servers yet. Click <span className="font-medium">Add server</span> to register a stdio command or HTTP endpoint.
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
    connected: 'text-neon-lime bg-neon-lime/15',
    idle: 'text-primary bg-primary/15',
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
            {server.transport === 'http'
              ? (server.url ?? '')
              : `${server.command ?? ''} ${(server.args ?? []).join(' ')}`.trim()}
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
  const isHttp = draft.transport === 'http';
  return (
    <div className="rounded-lg border border-primary/40 bg-primary/5 p-4 space-y-3">
      <div className="grid grid-cols-2 gap-3">
        <Field label="Server name">
          <input
            value={draft.name}
            onChange={e => onChange({ ...draft, name: e.target.value })}
            placeholder="Local MCP server"
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-primary/40"
          />
        </Field>
        <Field label="ID (stable)">
          <input
            value={draft.id}
            onChange={e => onChange({ ...draft, id: e.target.value })}
            placeholder="local-mcp-server"
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
          />
        </Field>
      </div>
      <Field label="Transport">
        <div className="flex gap-1">
          {(['stdio', 'http'] as const).map(option => {
            const active = draft.transport === option;
            return (
              <button
                key={option}
                type="button"
                onClick={() => onChange({ ...draft, transport: option })}
                className={`rounded-md border px-3 py-1.5 text-xs mono uppercase tracking-wider transition-colors ${
                  active
                    ? 'border-primary bg-primary/15 text-primary'
                    : 'border-border bg-background hover:bg-muted'
                }`}
              >
                {option}
              </button>
            );
          })}
        </div>
      </Field>
      {isHttp ? (
        <Field label="URL (Streamable HTTP endpoint)">
          <input
            value={draft.url}
            onChange={e => onChange({ ...draft, url: e.target.value })}
            placeholder="https://mcp.example.com/v1"
            className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
          />
        </Field>
      ) : (
        <>
          <Field label="Command (executable path)">
            <input
              value={draft.command}
              onChange={e => onChange({ ...draft, command: e.target.value })}
              placeholder="/Users/me/.local/bin/mcp-server"
              className="w-full rounded-md border border-border bg-background px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
            />
          </Field>
          <Field label="Arguments (one per line)">
            <textarea
              value={draft.argsText}
              onChange={e => onChange({ ...draft, argsText: e.target.value })}
              placeholder={'--token\nyour-token\n--endpoint\nmcp.example.net/ws'}
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
        </>
      )}
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

function HttpDefaultsRow({
  onStatus,
  onError,
}: {
  onStatus: (msg: string | null) => void;
  onError: (msg: string | null) => void;
}) {
  const [ua, setUa] = useState('');
  const [original, setOriginal] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    toolApi
      .getHttpUserAgent()
      .then(value => {
        if (cancelled) return;
        setUa(value);
        setOriginal(value);
      })
      .catch(err => onError(formatError(err)));
    return () => {
      cancelled = true;
    };
  }, [onError]);

  const dirty = ua !== original;

  const save = async () => {
    setBusy(true);
    onError(null);
    try {
      const resolved = await toolApi.setHttpUserAgent(ua);
      setUa(resolved);
      setOriginal(resolved);
      onStatus(`HTTP User-Agent updated`);
    } catch (err) {
      onError(formatError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="border-b border-border px-5 py-3 bg-muted/10">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <p className="mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
            // http User-Agent
          </p>
          <p className="mt-0.5 text-[11px] text-muted-foreground">
            Sent on every <code className="font-mono">http_request</code> from chat tools and widget pipelines. Leave blank to reset to the default.
          </p>
        </div>
        <input
          value={ua}
          onChange={event => setUa(event.target.value)}
          placeholder="Datrina/0.1.0 (+local)"
          className="w-72 rounded-md border border-border bg-background px-2 py-1.5 text-xs font-mono focus:outline-none focus:ring-1 focus:ring-primary/40"
        />
        <button
          type="button"
          onClick={save}
          disabled={!dirty || busy}
          className="rounded-md border border-border bg-card px-2.5 py-1.5 text-[11px] mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 disabled:opacity-50 transition-colors"
        >
          {busy ? 'Saving…' : 'Save'}
        </button>
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
