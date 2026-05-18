import { useEffect, useMemo, useState } from 'react';
import {
  type AssistantLanguagePolicy,
  type CreateProviderRequest,
  type LLMProvider,
  languageApi,
  type ProviderKind,
  type ProviderTestResult,
  type UpdateProviderRequest,
} from '../../lib/api';
import { AssistantLanguagePicker } from '../settings/AssistantLanguagePicker';

interface Props {
  providers: LLMProvider[];
  activeProviderId: string | null;
  initialSetup: boolean;
  isBusy: boolean;
  error: string | null;
  onClose: () => void;
  onAddProvider: (provider: CreateProviderRequest) => Promise<void>;
  onUpdateProvider: (id: string, provider: UpdateProviderRequest) => Promise<void>;
  onSetProviderEnabled: (id: string, isEnabled: boolean) => Promise<void>;
  onRemoveProvider: (id: string) => Promise<void>;
  onSetActiveProvider: (id: string) => Promise<void>;
  onTestProvider: (id: string) => Promise<ProviderTestResult>;
}

// W29: production templates only — `local_mock` was removed. The
// fallback dev provider is gone; the operator must configure a real
// provider (OpenRouter with key, reachable Ollama, or a custom
// OpenAI-compatible endpoint).
const PROVIDER_TEMPLATES: Record<ProviderKind, CreateProviderRequest> = {
  openrouter: {
    name: 'OpenRouter',
    kind: 'openrouter',
    base_url: 'https://openrouter.ai/api/v1',
    default_model: 'openai/gpt-4o-mini',
    models: ['openai/gpt-4o-mini'],
  },
  ollama: {
    name: 'Ollama',
    kind: 'ollama',
    base_url: 'http://localhost:11434',
    default_model: 'llama3.1',
    models: ['llama3.1'],
  },
  custom: {
    name: 'Custom OpenAI-compatible',
    kind: 'custom',
    base_url: '',
    default_model: '',
    models: [],
  },
};

const DEFAULT_KIND: ProviderKind = 'openrouter';

export function ProviderSettings({
  providers,
  activeProviderId,
  initialSetup,
  isBusy,
  error,
  onClose,
  onAddProvider,
  onUpdateProvider,
  onSetProviderEnabled,
  onRemoveProvider,
  onSetActiveProvider,
  onTestProvider,
}: Props) {
  const [draft, setDraft] = useState<CreateProviderRequest>(PROVIDER_TEMPLATES[DEFAULT_KIND]);
  const [apiKey, setApiKey] = useState('');
  const [modelsText, setModelsText] = useState(PROVIDER_TEMPLATES[DEFAULT_KIND].models?.join(', ') ?? '');
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<ProviderTestResult | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  // W47: app-level default language policy. Loaded once when the
  // settings panel opens; mutations round-trip through the Tauri
  // command so a reload picks up the same value.
  const [appLanguage, setAppLanguage] = useState<AssistantLanguagePolicy | null>(null);
  const [languageError, setLanguageError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    languageApi
      .getAppPolicy()
      .then(policy => {
        if (!cancelled) {
          setAppLanguage(policy);
          setLanguageError(null);
        }
      })
      .catch(err => {
        if (!cancelled) {
          setLanguageError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const activeProvider = useMemo(
    () => providers.find(provider => provider.id === activeProviderId) ?? null,
    [providers, activeProviderId]
  );

  const selectKind = (kind: ProviderKind) => {
    const next = PROVIDER_TEMPLATES[kind];
    setDraft(next);
    setApiKey('');
    setModelsText(next.models?.join(', ') ?? '');
    setTestResult(null);
    setLocalError(null);
  };

  const editProvider = (provider: LLMProvider) => {
    setEditingProviderId(provider.id);
    setDraft({
      name: provider.name,
      kind: provider.kind,
      base_url: provider.base_url,
      default_model: provider.default_model,
      models: provider.models,
    });
    setApiKey('');
    setModelsText(provider.models.join(', '));
    setTestResult(null);
    setLocalError(null);
  };

  const resetDraft = () => {
    setEditingProviderId(null);
    setDraft(PROVIDER_TEMPLATES[DEFAULT_KIND]);
    setApiKey('');
    setModelsText(PROVIDER_TEMPLATES[DEFAULT_KIND].models?.join(', ') ?? '');
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    setLocalError(null);
    setTestResult(null);

    const trimmedApiKey = apiKey.trim();
    const models = modelsText
      .split(',')
      .map((model: string) => model.trim())
      .filter(Boolean);

    try {
      const payload = {
        ...draft,
        name: draft.name.trim(),
        base_url: draft.base_url.trim(),
        default_model: draft.default_model.trim(),
        api_key: trimmedApiKey ? trimmedApiKey : undefined,
        models,
      };
      if (editingProviderId) {
        await onUpdateProvider(editingProviderId, payload);
        resetDraft();
      } else {
        await onAddProvider(payload);
      }
      if (!initialSetup) {
        setApiKey('');
      }
    } catch (err) {
      setLocalError(err instanceof Error ? err.message : String(err));
    }
  };

  const runTest = async (provider: LLMProvider) => {
    setLocalError(null);
    try {
      const result = await onTestProvider(provider.id);
      setTestResult(result);
    } catch (err) {
      setLocalError(err instanceof Error ? err.message : String(err));
    }
  };

  const canSubmit = !isBusy && draft.name.trim() && draft.default_model.trim() && draft.base_url.trim();

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/85 p-4 backdrop-blur-sm">
      <div className="flex max-h-[92vh] w-full max-w-5xl flex-col overflow-hidden rounded-md border border-border bg-card shadow-2xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-4 bg-muted/20">
          <div>
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// providers</p>
            <h2 className="mt-0.5 text-base font-semibold tracking-tight">{initialSetup ? 'LLM provider setup' : 'Settings'}</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              {activeProvider ? <>Active: <span className="text-foreground">{activeProvider.name}</span> · <span className="mono">{activeProvider.default_model}</span></> : 'No active LLM provider'}
            </p>
          </div>
          {!initialSetup && (
            <button onClick={onClose} className="rounded-md p-2 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground" aria-label="Close settings">
              <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          )}
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-1 overflow-y-auto md:grid-cols-[minmax(0,1fr)_360px]">
          <form onSubmit={handleSubmit} className="space-y-5 border-b border-border p-5 md:border-b-0 md:border-r">
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
              {(['openrouter', 'ollama', 'custom'] as ProviderKind[]).map(kind => (
                <button
                  key={kind}
                  type="button"
                  onClick={() => selectKind(kind)}
                  className={`rounded-lg border px-3 py-2 text-left text-sm transition-colors ${
                    draft.kind === kind
                      ? 'border-primary bg-primary/10 text-primary'
                      : 'border-border bg-muted/30 hover:bg-muted'
                  }`}
                >
                  {labelForKind(kind)}
                </button>
              ))}
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <label className="space-y-1.5 text-sm">
                <span className="text-xs font-medium text-muted-foreground">Name</span>
                <input
                  value={draft.name}
                  onChange={event => setDraft(prev => ({ ...prev, name: event.target.value }))}
                  className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/30"
                />
              </label>

              <label className="space-y-1.5 text-sm">
                <span className="text-xs font-medium text-muted-foreground">Model</span>
                <input
                  value={draft.default_model}
                  onChange={event => setDraft(prev => ({ ...prev, default_model: event.target.value }))}
                  className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/30"
                />
              </label>
            </div>

            <label className="block space-y-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">Base URL</span>
              <input
                value={draft.base_url}
                onChange={event => setDraft(prev => ({ ...prev, base_url: event.target.value }))}
                className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm outline-none disabled:opacity-60 focus:ring-2 focus:ring-primary/30"
              />
            </label>

            {(draft.kind === 'openrouter' || draft.kind === 'custom') && (
              <label className="block space-y-1.5 text-sm">
                <span className="text-xs font-medium text-muted-foreground">API key</span>
                <input
                  type="password"
                  value={apiKey}
                  onChange={event => setApiKey(event.target.value)}
                  className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/30"
                  autoComplete="off"
                />
              </label>
            )}

            <label className="block space-y-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">Known models</span>
              <input
                value={modelsText}
                onChange={event => setModelsText(event.target.value)}
                className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/30"
              />
            </label>

            {(error || localError) && (
              <div className="rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive">
                {localError || error}
              </div>
            )}

            {testResult && (
              <div className={`rounded-lg border px-3 py-2 text-sm ${
                testResult.status === 'ok'
                  ? 'border-neon-lime/30 bg-neon-lime/10 text-neon-lime'
                  : 'border-destructive/30 bg-destructive/5 text-destructive'
              }`}>
                Test {testResult.status}: {testResult.error || `${testResult.provider} responded for ${testResult.model}`}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2">
              <button
                type="submit"
                disabled={!canSubmit}
                className="rounded-lg bg-primary px-4 py-2 text-sm text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {editingProviderId ? 'Save changes' : initialSetup ? 'Save and continue' : 'Add provider'}
              </button>
              {editingProviderId && (
                <button
                  type="button"
                  onClick={resetDraft}
                  className="rounded-lg border border-border px-4 py-2 text-sm transition-colors hover:bg-muted"
                >
                  Cancel edit
                </button>
              )}
            </div>
          </form>

          <div className="space-y-4 p-5">
            <div className="space-y-2 rounded-md border border-border bg-background/60 p-3">
              <div>
                <h3 className="text-sm font-medium">Assistant language</h3>
                <p className="mt-1 text-xs text-muted-foreground">
                  Applies to chat, Build chat, and LLM-backed widgets / pipelines.
                  Dashboard and chat-session overrides take precedence.
                </p>
              </div>
              <AssistantLanguagePicker
                value={appLanguage}
                label="App default"
                hint="Schema keys, tool names, ids, and validation codes are never translated."
                onChange={async next => {
                  const policy = next ?? { mode: 'auto' };
                  try {
                    const persisted = await languageApi.setAppPolicy(policy);
                    setAppLanguage(persisted);
                    setLanguageError(null);
                  } catch (err) {
                    setLanguageError(err instanceof Error ? err.message : String(err));
                  }
                }}
              />
              {languageError && (
                <p className="text-[11px] text-destructive">{languageError}</p>
              )}
            </div>

            <div>
              <h3 className="text-sm font-medium">Providers</h3>
              <p className="mt-1 text-xs text-muted-foreground">Chat uses the selected active provider.</p>
            </div>

            {providers.length === 0 ? (
              <div className="rounded-lg border border-dashed border-border p-4 text-sm text-muted-foreground">
                No providers saved.
              </div>
            ) : (
              <div className="space-y-2">
                {providers.map(provider => {
                  const isActive = provider.id === activeProviderId;
                  const unsupported = provider.is_unsupported === true;
                  return (
                    <div
                      key={provider.id}
                      className={`rounded-lg border p-3 ${
                        unsupported
                          ? 'border-destructive/40 bg-destructive/5'
                          : 'border-border bg-background/60'
                      }`}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="truncate text-sm font-medium">{provider.name}</span>
                            {isActive && !unsupported && (
                              <span className="rounded-sm border border-neon-lime/40 bg-neon-lime/15 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider font-semibold text-neon-lime">active</span>
                            )}
                            {unsupported && (
                              <span className="rounded-sm border border-destructive/40 bg-destructive/15 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider font-semibold text-destructive">unsupported</span>
                            )}
                          </div>
                          <p className="mt-1 truncate text-xs text-muted-foreground">
                            {labelForKind(provider.kind)} - {provider.default_model}
                          </p>
                          {unsupported && (
                            <p className="mt-1 text-[11px] text-destructive">
                              Legacy provider kind (e.g. <code className="rounded bg-foreground/10 px-1">local_mock</code>) is no longer supported. Edit it to OpenRouter, Ollama, or Custom — or remove it.
                            </p>
                          )}
                        </div>
                      </div>
                      <div className="mt-3 flex flex-wrap gap-2">
                        <button
                          type="button"
                          onClick={() => onSetActiveProvider(provider.id)}
                          disabled={isActive || isBusy || unsupported || !provider.is_enabled}
                          className="rounded-md border border-border px-2.5 py-1.5 text-xs transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Set active
                        </button>
                        <button
                          type="button"
                          onClick={() => runTest(provider)}
                          disabled={isBusy || unsupported}
                          className="rounded-md border border-border px-2.5 py-1.5 text-xs transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Test
                        </button>
                        <button
                          type="button"
                          onClick={() => editProvider(provider)}
                          disabled={isBusy}
                          className="rounded-md border border-border px-2.5 py-1.5 text-xs transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Edit
                        </button>
                        <button
                          type="button"
                          onClick={() => onSetProviderEnabled(provider.id, !provider.is_enabled)}
                          disabled={isBusy || (unsupported && !provider.is_enabled)}
                          className="rounded-md border border-border px-2.5 py-1.5 text-xs transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {provider.is_enabled ? 'Disable' : 'Enable'}
                        </button>
                        <button
                          type="button"
                          onClick={() => onRemoveProvider(provider.id)}
                          disabled={isBusy}
                          className="rounded-md border border-destructive/30 px-2.5 py-1.5 text-xs text-destructive transition-colors hover:bg-destructive/10 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          Remove
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function labelForKind(kind: ProviderKind) {
  switch (kind) {
    case 'openrouter':
      return 'OpenRouter';
    case 'ollama':
      return 'Ollama';
    case 'custom':
      return 'Custom';
  }
}
