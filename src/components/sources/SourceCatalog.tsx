// W37: External open-source / free-use source catalog.
//
// Shows the built-in catalog with review status, license/terms metadata,
// enable/disable toggle, optional credential entry, a Test panel, and a
// "Save as datasource" shortcut. Disabled / blocked / credential-missing
// entries fail closed at the backend; this UI just surfaces the reason.

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  externalSourceApi,
  systemApi,
  type ExternalSourceImpactPreview,
  type ExternalSourceWithState,
  type ExternalSourceTestResult,
  type ExternalSourceReviewStatus,
} from '../../lib/api';

interface Props {
  onClose: () => void;
  onOpenWorkbench?: () => void;
}

const REVIEW_LABEL: Record<ExternalSourceReviewStatus, string> = {
  allowed: 'Allowed',
  allowed_with_conditions: 'Allowed with conditions',
  needs_review: 'Needs review',
  blocked: 'Blocked',
};

const REVIEW_CLASS: Record<ExternalSourceReviewStatus, string> = {
  allowed: 'bg-emerald-500/15 text-emerald-300 border-emerald-500/40',
  allowed_with_conditions: 'bg-amber-500/15 text-amber-300 border-amber-500/40',
  needs_review: 'bg-amber-500/10 text-amber-200 border-amber-500/30',
  blocked: 'bg-destructive/15 text-destructive border-destructive/40',
};

function previewJson(value: unknown): string {
  try {
    const text = JSON.stringify(value, null, 2);
    return text.length > 6_000 ? `${text.slice(0, 6_000)}\n…(truncated)` : text;
  } catch {
    return String(value);
  }
}

export function SourceCatalog({ onClose, onOpenWorkbench }: Props) {
  const [sources, setSources] = useState<ExternalSourceWithState[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string>('Ready');
  const [credentialDraft, setCredentialDraft] = useState<Record<string, string>>({});
  const [argsDraft, setArgsDraft] = useState<Record<string, Record<string, string>>>({});
  const [lastTest, setLastTest] = useState<ExternalSourceTestResult | null>(null);
  const [saveName, setSaveName] = useState<string>('');
  const [impact, setImpact] = useState<ExternalSourceImpactPreview | null>(null);

  const load = useCallback(async () => {
    try {
      const next = await externalSourceApi.list();
      setSources(next);
      if (next.length && !selectedId) {
        setSelectedId(next[0].id);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [selectedId]);

  useEffect(() => {
    void load();
  }, [load]);

  const selected = useMemo(
    () => sources.find((s) => s.id === selectedId) ?? null,
    [sources, selectedId],
  );

  const updateInList = useCallback((next: ExternalSourceWithState) => {
    setSources((prev) => prev.map((s) => (s.id === next.id ? next : s)));
  }, []);

  const handleToggle = useCallback(
    async (id: string, enabled: boolean) => {
      setBusy(true);
      setError(null);
      try {
        // Before disabling, surface saved datasources that originated
        // from this source so the user knows what they'll break.
        if (!enabled) {
          const preview = await externalSourceApi.previewImpact(id);
          if (preview.originating_datasources.length > 0) {
            const names = preview.originating_datasources
              .map((d) => d.name)
              .join(', ');
            const ok = window.confirm(
              `Disabling this source will leave ${preview.originating_datasources.length} saved datasource(s) without their credential: ${names}\n\nProceed?`,
            );
            if (!ok) {
              setBusy(false);
              return;
            }
          }
        }
        const next = await externalSourceApi.setEnabled(id, enabled);
        updateInList(next);
        if (enabled) {
          setImpact(null);
        } else {
          // Refresh impact so the right pane reflects post-disable state.
          try {
            const preview = await externalSourceApi.previewImpact(id);
            setImpact(preview);
          } catch {
            /* impact preview is best-effort */
          }
        }
        setStatus(enabled ? `Enabled ${next.display_name}` : `Disabled ${next.display_name}`);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    },
    [updateInList],
  );

  useEffect(() => {
    if (!selectedId) {
      setImpact(null);
      return;
    }
    externalSourceApi
      .previewImpact(selectedId)
      .then(setImpact)
      .catch(() => setImpact(null));
  }, [selectedId]);

  const handleSaveCredential = useCallback(
    async (id: string) => {
      const value = (credentialDraft[id] ?? '').trim();
      setBusy(true);
      setError(null);
      try {
        const next = await externalSourceApi.setCredential(id, value || null);
        updateInList(next);
        setCredentialDraft((prev) => ({ ...prev, [id]: '' }));
        setStatus(value ? 'Credential stored locally' : 'Credential cleared');
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    },
    [credentialDraft, updateInList],
  );

  const handleTest = useCallback(async () => {
    if (!selected) return;
    setBusy(true);
    setError(null);
    setLastTest(null);
    try {
      const args = argsDraft[selected.id] ?? {};
      const payload: Record<string, unknown> = {};
      for (const param of selected.params) {
        const raw = args[param.name];
        if (raw === undefined || raw === '') continue;
        const schemaType =
          param.schema && typeof param.schema === 'object'
            ? (param.schema as { type?: string }).type
            : undefined;
        if (schemaType === 'integer' || schemaType === 'number') {
          const num = Number(raw);
          if (!Number.isNaN(num)) {
            payload[param.name] = num;
            continue;
          }
        }
        payload[param.name] = raw;
      }
      const result = await externalSourceApi.test(selected.id, payload);
      setLastTest(result);
      setStatus(`Tested ${selected.display_name} in ${result.duration_ms}ms`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [selected, argsDraft]);

  const handleSave = useCallback(async () => {
    if (!selected) return;
    const name = saveName.trim();
    if (!name) {
      setError('Give the saved datasource a name first');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const args = argsDraft[selected.id] ?? {};
      const payload: Record<string, unknown> = {};
      for (const param of selected.params) {
        const raw = args[param.name];
        if (raw === undefined || raw === '') continue;
        const schemaType =
          param.schema && typeof param.schema === 'object'
            ? (param.schema as { type?: string }).type
            : undefined;
        if (schemaType === 'integer' || schemaType === 'number') {
          const num = Number(raw);
          if (!Number.isNaN(num)) {
            payload[param.name] = num;
            continue;
          }
        }
        payload[param.name] = raw;
      }
      const result = await externalSourceApi.saveAsDatasource({
        source_id: selected.id,
        name,
        arguments: payload,
      });
      setSaveName('');
      setStatus(`Saved as datasource ${result.datasource_id.slice(0, 8)}…`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [selected, saveName, argsDraft]);

  return (
    <div className="fixed inset-0 z-40 bg-background/90 backdrop-blur-sm flex items-stretch">
      <div className="flex-1 flex flex-col">
        <header className="flex items-center justify-between border-b border-border px-6 py-3">
          <div>
            <h2 className="text-lg font-semibold tracking-tight">External Source Catalog</h2>
            <p className="text-xs text-muted-foreground mono mt-0.5">
              Reviewed open-source / free-use sources Datrina chat can call as typed tools.
            </p>
          </div>
          <div className="flex items-center gap-2">
            {onOpenWorkbench && (
              <button
                onClick={onOpenWorkbench}
                className="text-xs mono px-2.5 py-1.5 border border-border rounded-md hover:bg-muted/40"
              >
                Open Workbench →
              </button>
            )}
            <button
              onClick={onClose}
              className="text-xs mono px-2.5 py-1.5 border border-border rounded-md hover:bg-muted/40"
            >
              Close
            </button>
          </div>
        </header>

        {error && (
          <div className="mx-6 mt-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </div>
        )}

        <div className="flex flex-1 overflow-hidden">
          {/* Catalog list */}
          <aside className="w-80 border-r border-border overflow-y-auto">
            <ul className="divide-y divide-border">
              {sources.map((source) => (
                <li key={source.id}>
                  <button
                    onClick={() => {
                      setSelectedId(source.id);
                      setLastTest(null);
                    }}
                    className={`w-full text-left px-4 py-3 hover:bg-muted/30 ${
                      selectedId === source.id ? 'bg-muted/40' : ''
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="font-medium text-sm">{source.display_name}</span>
                      <span
                        className={`text-[10px] mono uppercase tracking-wider px-1.5 py-0.5 rounded border ${
                          REVIEW_CLASS[source.review_status]
                        }`}
                      >
                        {REVIEW_LABEL[source.review_status]}
                      </span>
                    </div>
                    <div className="mt-1 flex items-center gap-2 text-[10px] mono text-muted-foreground">
                      <span>{source.domain.replace('_', ' ')}</span>
                      <span aria-hidden>·</span>
                      <span>{source.is_runnable ? 'runnable' : source.blocked_reason ?? '—'}</span>
                      {source.state.is_enabled && (
                        <>
                          <span aria-hidden>·</span>
                          <span className="text-emerald-300">enabled</span>
                        </>
                      )}
                    </div>
                  </button>
                </li>
              ))}
              {sources.length === 0 && (
                <li className="px-4 py-6 text-xs text-muted-foreground">No catalog entries.</li>
              )}
            </ul>
          </aside>

          {/* Detail */}
          <main className="flex-1 overflow-y-auto px-6 py-4 space-y-4">
            {!selected && (
              <div className="text-sm text-muted-foreground">Select a source on the left.</div>
            )}
            {selected && (
              <>
                <section className="space-y-2">
                  <div className="flex items-center gap-3 flex-wrap">
                    <h3 className="text-base font-semibold">{selected.display_name}</h3>
                    <span
                      className={`text-[10px] mono uppercase tracking-wider px-1.5 py-0.5 rounded border ${
                        REVIEW_CLASS[selected.review_status]
                      }`}
                    >
                      {REVIEW_LABEL[selected.review_status]}
                    </span>
                    <span className="text-[10px] mono text-muted-foreground">
                      adapter: {selected.adapter_license}
                    </span>
                    <span className="text-[10px] mono text-muted-foreground">
                      reviewed: {selected.review_date}
                    </span>
                  </div>
                  <p className="text-sm">{selected.description}</p>
                  <p className="text-xs text-muted-foreground italic">{selected.review_notes}</p>
                  {selected.attribution && (
                    <p className="text-xs text-muted-foreground">
                      <strong>Attribution:</strong> {selected.attribution}
                    </p>
                  )}
                  <div className="text-xs">
                    Terms / API policy:{' '}
                    <button
                      type="button"
                      onClick={() => void systemApi.openUrl(selected.terms_url).catch(() => {})}
                      className="underline text-primary mono"
                    >
                      {selected.terms_url}
                    </button>
                  </div>
                </section>

                {selected.adapter !== 'mcp_recommended' && (
                  <section className="rounded-md border border-border bg-muted/10 px-4 py-3 space-y-3">
                    <div className="flex items-center justify-between">
                      <div>
                        <h4 className="text-sm font-medium">Enablement</h4>
                        <p className="text-[11px] text-muted-foreground mono">
                          Enabled sources are exposed to chat as `source_{selected.id}` tool calls.
                        </p>
                      </div>
                      <label className="flex items-center gap-2 text-sm">
                        <input
                          type="checkbox"
                          checked={selected.state.is_enabled}
                          disabled={
                            busy ||
                            (!selected.review_status.endsWith('conditions') &&
                              selected.review_status !== 'allowed')
                          }
                          onChange={(e) => void handleToggle(selected.id, e.target.checked)}
                        />
                        Enable
                      </label>
                    </div>
                    {selected.blocked_reason && (
                      <div className="text-xs text-amber-300">
                        Not runnable: {selected.blocked_reason}
                      </div>
                    )}
                  </section>
                )}

                {selected.rate_limit && (
                  <section className="rounded-md border border-border bg-muted/10 px-4 py-3 space-y-2">
                    <h4 className="text-sm font-medium">Plan / rate</h4>
                    <div className="text-xs grid gap-1">
                      <div>
                        <span className="mono uppercase tracking-wider text-muted-foreground">
                          plan
                        </span>{' '}
                        {selected.rate_limit.plan_name}
                      </div>
                      <div>
                        <span className="mono uppercase tracking-wider text-muted-foreground">
                          free quota
                        </span>{' '}
                        {selected.rate_limit.free_quota}
                      </div>
                      {selected.rate_limit.paid_tier && (
                        <div>
                          <span className="mono uppercase tracking-wider text-muted-foreground">
                            paid tier
                          </span>{' '}
                          {selected.rate_limit.paid_tier}
                        </div>
                      )}
                      {typeof selected.rate_limit.queries_per_second === 'number' && (
                        <div>
                          <span className="mono uppercase tracking-wider text-muted-foreground">
                            qps
                          </span>{' '}
                          {selected.rate_limit.queries_per_second}
                        </div>
                      )}
                      <div className="flex gap-3 mt-1">
                        {selected.rate_limit.attribution_required && (
                          <span className="text-[10px] mono uppercase tracking-wider rounded border border-amber-500/40 px-1.5 text-amber-300">
                            attribution required
                          </span>
                        )}
                        {selected.rate_limit.storage_rights_required && (
                          <span className="text-[10px] mono uppercase tracking-wider rounded border border-amber-500/40 px-1.5 text-amber-300">
                            storage rights required
                          </span>
                        )}
                      </div>
                    </div>
                  </section>
                )}

                {selected.mcp_install && (
                  <section className="rounded-md border border-primary/30 bg-primary/5 px-4 py-3 space-y-2">
                    <h4 className="text-sm font-medium">MCP install command</h4>
                    <p className="text-[11px] text-muted-foreground mono">
                      Datrina does not install MCP servers automatically. Copy the
                      command below into MCP Settings.
                    </p>
                    <pre className="text-xs font-mono rounded-md bg-background border border-border px-3 py-2 overflow-auto whitespace-pre">
                      {[selected.mcp_install.command, ...selected.mcp_install.args].join(' ')}
                    </pre>
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground">
                        package
                      </span>
                      <span className="text-[11px] mono">
                        {selected.mcp_install.package_kind} · {selected.mcp_install.package_name}
                      </span>
                      <button
                        type="button"
                        onClick={() => {
                          const cmd = [
                            selected.mcp_install!.command,
                            ...selected.mcp_install!.args,
                          ].join(' ');
                          void navigator.clipboard
                            .writeText(cmd)
                            .then(() => setStatus('Command copied to clipboard'))
                            .catch(() =>
                              setError('Clipboard write failed; copy manually'),
                            );
                        }}
                        className="text-xs mono px-2.5 py-1 border border-border rounded-md hover:bg-muted/40"
                      >
                        Copy command
                      </button>
                    </div>
                    {selected.mcp_install.env_hints.length > 0 && (
                      <div className="space-y-1">
                        <div className="text-[10px] mono uppercase tracking-wider text-muted-foreground">
                          required env
                        </div>
                        <ul className="text-xs space-y-0.5">
                          {selected.mcp_install.env_hints.map((hint) => (
                            <li key={hint.name} className="mono">
                              · {hint.name}
                              {hint.required ? '*' : ''} —{' '}
                              <span className="text-muted-foreground">{hint.description}</span>
                            </li>
                          ))}
                        </ul>
                      </div>
                    )}
                  </section>
                )}

                {impact && impact.originating_datasources.length > 0 && (
                  <section className="rounded-md border border-amber-500/30 bg-amber-500/5 px-4 py-3 space-y-2">
                    <h4 className="text-sm font-medium text-amber-200">Originating datasources</h4>
                    <p className="text-[11px] text-muted-foreground mono">
                      {impact.originating_datasources.length} saved datasource(s) were created from this catalog entry. Disabling the source will leave their BYOK header unfilled at refresh time.
                    </p>
                    <ul className="text-xs space-y-1">
                      {impact.originating_datasources.map((d) => (
                        <li key={d.datasource_id} className="mono">
                          · {d.name}{' '}
                          <span className="text-muted-foreground">
                            ({d.datasource_id.slice(0, 8)}…)
                          </span>
                          {onOpenWorkbench && (
                            <button
                              type="button"
                              onClick={onOpenWorkbench}
                              className="ml-2 underline text-primary"
                            >
                              open in Workbench
                            </button>
                          )}
                        </li>
                      ))}
                    </ul>
                  </section>
                )}

                {selected.credential_policy !== 'none' && selected.adapter !== 'mcp_recommended' && (
                  <section className="rounded-md border border-border bg-muted/10 px-4 py-3 space-y-2">
                    <h4 className="text-sm font-medium">Credential</h4>
                    {selected.credential_help && (
                      <p className="text-[11px] text-muted-foreground">
                        {selected.credential_help}
                      </p>
                    )}
                    <p className="text-[11px] text-muted-foreground mono">
                      {selected.state.has_credential
                        ? 'credential: stored locally'
                        : 'credential: not set'}
                      {selected.credential_policy === 'required' && ' (required)'}
                      {selected.credential_policy === 'optional' && ' (optional)'}
                    </p>
                    <div className="flex gap-2">
                      <input
                        type="password"
                        placeholder={
                          selected.state.has_credential ? 'replace credential…' : 'paste credential'
                        }
                        value={credentialDraft[selected.id] ?? ''}
                        onChange={(e) =>
                          setCredentialDraft((prev) => ({
                            ...prev,
                            [selected.id]: e.target.value,
                          }))
                        }
                        className="flex-1 rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                      />
                      <button
                        onClick={() => void handleSaveCredential(selected.id)}
                        disabled={busy}
                        className="text-xs mono px-3 py-1.5 border border-border rounded-md hover:bg-muted/40 disabled:opacity-50"
                      >
                        Save
                      </button>
                      {selected.state.has_credential && (
                        <button
                          onClick={() => {
                            setCredentialDraft((prev) => ({ ...prev, [selected.id]: '' }));
                            void externalSourceApi
                              .setCredential(selected.id, null)
                              .then(updateInList)
                              .catch((err) =>
                                setError(err instanceof Error ? err.message : String(err)),
                              );
                          }}
                          disabled={busy}
                          className="text-xs mono px-3 py-1.5 border border-destructive/40 text-destructive rounded-md hover:bg-destructive/10 disabled:opacity-50"
                        >
                          Clear
                        </button>
                      )}
                    </div>
                  </section>
                )}

                {selected.adapter !== 'mcp_recommended' && (
                <section className="rounded-md border border-border bg-muted/10 px-4 py-3 space-y-2">
                  <h4 className="text-sm font-medium">Try it</h4>
                  <p className="text-[11px] text-muted-foreground mono">
                    {selected.adapter === 'web_fetch'
                      ? 'tool_engine.web_fetch (robots-aware, 500 KiB cap)'
                      : `${selected.http.method} ${selected.http.url}`}
                  </p>
                  <div className="grid gap-2">
                    {selected.params.map((param) => {
                      const schemaType =
                        param.schema && typeof param.schema === 'object'
                          ? (param.schema as { type?: string }).type
                          : undefined;
                      const inputType =
                        schemaType === 'integer' || schemaType === 'number' ? 'number' : 'text';
                      return (
                        <label key={param.name} className="grid gap-1 text-xs">
                          <span className="mono uppercase tracking-wider text-muted-foreground">
                            {param.name}
                            {param.required ? '*' : ''}
                          </span>
                          <input
                            type={inputType}
                            placeholder={param.description}
                            value={argsDraft[selected.id]?.[param.name] ?? ''}
                            onChange={(e) =>
                              setArgsDraft((prev) => ({
                                ...prev,
                                [selected.id]: {
                                  ...(prev[selected.id] ?? {}),
                                  [param.name]: e.target.value,
                                },
                              }))
                            }
                            className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                          />
                          <span className="text-[10px] text-muted-foreground">
                            {param.description}
                          </span>
                        </label>
                      );
                    })}
                  </div>
                  <div className="flex flex-wrap gap-2 items-center mt-2">
                    <button
                      onClick={() => void handleTest()}
                      disabled={busy || !selected.is_runnable}
                      className="text-xs mono px-3 py-1.5 border border-border rounded-md hover:bg-muted/40 disabled:opacity-50"
                    >
                      Test request
                    </button>
                    <input
                      type="text"
                      placeholder="Saved-datasource name"
                      value={saveName}
                      onChange={(e) => setSaveName(e.target.value)}
                      className="rounded-md border border-border bg-background px-2 py-1.5 text-sm flex-1 min-w-[180px]"
                    />
                    <button
                      onClick={() => void handleSave()}
                      disabled={busy || !selected.is_runnable || selected.credential_policy === 'required'}
                      title={
                        selected.credential_policy === 'required'
                          ? 'Sources with required credentials cannot be saved as datasources (key would leak into workflow JSON).'
                          : ''
                      }
                      className="text-xs mono px-3 py-1.5 border border-border rounded-md hover:bg-muted/40 disabled:opacity-50"
                    >
                      Save as datasource
                    </button>
                  </div>
                </section>
                )}

                {lastTest && (
                  <section className="rounded-md border border-border bg-muted/10 px-4 py-3 space-y-2">
                    <div className="flex items-center justify-between">
                      <h4 className="text-sm font-medium">Last test result</h4>
                      <span className="text-[10px] mono text-muted-foreground">
                        {lastTest.duration_ms}ms · {lastTest.pipeline_steps} pipeline step(s)
                      </span>
                    </div>
                    <p className="text-[11px] mono text-muted-foreground break-all">
                      → {lastTest.effective_url}
                    </p>
                    <details>
                      <summary className="text-xs cursor-pointer text-muted-foreground">
                        Final value
                      </summary>
                      <pre className="mt-2 text-[11px] font-mono whitespace-pre-wrap rounded-md bg-background border border-border px-3 py-2 max-h-96 overflow-auto">
                        {previewJson(lastTest.final_value)}
                      </pre>
                    </details>
                    <details>
                      <summary className="text-xs cursor-pointer text-muted-foreground">
                        Raw response
                      </summary>
                      <pre className="mt-2 text-[11px] font-mono whitespace-pre-wrap rounded-md bg-background border border-border px-3 py-2 max-h-96 overflow-auto">
                        {previewJson(lastTest.raw_response)}
                      </pre>
                    </details>
                  </section>
                )}
              </>
            )}
          </main>
        </div>

        <footer className="border-t border-border px-6 py-2 text-[11px] mono text-muted-foreground">
          {status}
        </footer>
      </div>
    </div>
  );
}
