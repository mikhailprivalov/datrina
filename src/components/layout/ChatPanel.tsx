import { useState, useRef, useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { chatApi } from '../../lib/api';
import type { BuildProposal, ChatEventEnvelope, ChatMessage, ChatSession, LLMProvider, ToolCallTrace, ToolResultTrace } from '../../lib/api';

interface Props {
  mode: 'build' | 'context';
  dashboardId?: string;
  activeProvider?: LLMProvider;
  canApplyToDashboard: boolean;
  onClose: () => void;
  onModeChange: (mode: 'build' | 'context') => void;
  onApplyBuildProposal: (proposal: BuildProposal) => Promise<void>;
}

export function ChatPanel({ mode, dashboardId, activeProvider, canApplyToDashboard, onClose, onModeChange, onApplyBuildProposal }: Props) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [session, setSession] = useState<ChatSession | null>(null);
  const [streamTraces, setStreamTraces] = useState<Record<string, StreamTrace>>({});
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const create = async () => {
      try {
        const s = await chatApi.createSession(mode, dashboardId);
        setSession(s);
        setMessages(s.messages);
      } catch (err) {
        console.error('Failed to create session:', err);
      }
    };
    create();
  }, [mode, dashboardId]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  useEffect(() => {
    if (!session?.id) return;
    const activeSessionId = session.id;
    const unsubscribe = listen<ChatEventEnvelope>('chat:event', event => {
      const chatEvent = event.payload;
      if (chatEvent.session_id !== activeSessionId) return;

      if (chatEvent.kind === 'message_started') {
        const draft = assistantDraft(chatEvent, mode);
        setMessages(prev => upsertMessage(prev, draft));
        setStreamTraces(prev => ({
          ...prev,
          [chatEvent.message_id]: {
            synthetic: chatEvent.synthetic,
            toolCalls: [],
            toolResults: [],
          },
        }));
        return;
      }

      if (chatEvent.kind === 'content_delta' && chatEvent.content_delta) {
        setMessages(prev => updateMessage(prev, chatEvent.message_id, msg => ({
          ...msg,
          content: `${msg.content}${chatEvent.content_delta}`,
        })));
        return;
      }

      if ((chatEvent.kind === 'reasoning_delta' && chatEvent.reasoning_delta) || chatEvent.kind === 'reasoning_snapshot') {
        const reasoning = chatEvent.reasoning ?? chatEvent.reasoning_delta ?? '';
        setMessages(prev => updateMessage(prev, chatEvent.message_id, msg => ({
          ...msg,
          metadata: {
            ...msg.metadata,
            reasoning: chatEvent.kind === 'reasoning_delta'
              ? `${msg.metadata?.reasoning ?? ''}${reasoning}`
              : reasoning,
          },
        })));
        return;
      }

      if (chatEvent.kind === 'tool_call_requested' || chatEvent.kind === 'tool_execution_started') {
        if (!chatEvent.tool_call) return;
        setStreamTraces(prev => ({
          ...prev,
          [chatEvent.message_id]: upsertToolCall(prev[chatEvent.message_id], chatEvent.tool_call!, chatEvent.synthetic),
        }));
        return;
      }

      if (chatEvent.kind === 'tool_result') {
        if (!chatEvent.tool_result) return;
        setStreamTraces(prev => ({
          ...prev,
          [chatEvent.message_id]: upsertToolResult(prev[chatEvent.message_id], chatEvent.tool_result!, chatEvent.synthetic),
        }));
        return;
      }

      if (chatEvent.kind === 'build_proposal_parsed' && chatEvent.build_proposal) {
        setMessages(prev => updateMessage(prev, chatEvent.message_id, msg => ({
          ...msg,
          metadata: {
            ...msg.metadata,
            build_proposal: chatEvent.build_proposal,
          },
        })));
        return;
      }

      if (chatEvent.kind === 'message_completed' && chatEvent.final_message) {
        setMessages(prev => upsertMessage(prev, chatEvent.final_message!));
        setIsLoading(false);
        return;
      }

      if (chatEvent.kind === 'message_failed') {
        setMessages(prev => upsertMessage(prev, {
          id: chatEvent.message_id,
          role: 'assistant',
          content: `Error: ${chatEvent.error ?? 'Chat stream failed'}`,
          mode,
          timestamp: chatEvent.emitted_at,
        }));
        setIsLoading(false);
      }
    });

    return () => {
      unsubscribe.then(dispose => dispose()).catch(err => {
        console.error('Failed to unsubscribe from chat events:', err);
      });
    };
  }, [session?.id, mode]);

  useEffect(() => {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 128)}px`;
  }, [input]);

  const handleInputChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInput(normalizeChatInput(event.target.value));
  };

  const handleSend = async () => {
    if (!input.trim() || !session || isLoading) return;
    const content = normalizeChatInput(input).trim();
    setInput('');
    setIsLoading(true);

    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content,
      mode,
      timestamp: Date.now(),
    };
    setMessages(prev => [...prev, userMsg]);

    try {
      const assistant = await chatApi.sendMessageStream(session.id, content);
      setMessages(prev => upsertMessage(prev, assistant));
    } catch (err) {
      const error: ChatMessage = {
        id: crypto.randomUUID(),
        role: 'assistant',
        content: `Error: ${err instanceof Error ? err.message : String(err)}`,
        mode,
        timestamp: Date.now(),
      };
      setMessages(prev => [...prev, error]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleCancel = async () => {
    if (!session || !isLoading) return;
    try {
      await chatApi.cancelResponse(session.id);
    } catch (err) {
      console.error('Failed to cancel chat response:', err);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <aside className="w-96 flex flex-col bg-card border-l border-border shadow-lg">
      <div className="flex items-center justify-between h-12 px-4 border-b border-border">
        <div className="flex items-center gap-2">
          <div className={`w-2 h-2 rounded-full ${mode === 'build' ? 'bg-amber-500' : 'bg-blue-500'}`} />
          <div className="min-w-0">
            <span className="block text-sm font-medium">{mode === 'build' ? 'Build Assistant' : 'Context Chat'}</span>
            <span className="block max-w-56 truncate text-[10px] text-muted-foreground">
              {activeProvider ? `${activeProvider.name} - ${activeProvider.default_model}` : 'No LLM provider configured'}
            </span>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <button onClick={() => onModeChange(mode === 'build' ? 'context' : 'build')} className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 7h12m0 0l-4-4m4 4l-4 4m0 6H4m0 0l4 4m-4-4l4-4" />
            </svg>
          </button>
          <button onClick={onClose} className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4 scrollbar-thin">
        {messages.length === 0 && (
          <div className="text-center text-muted-foreground text-sm mt-8">
            <svg className="w-10 h-10 mx-auto mb-3 opacity-40" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
            </svg>
            <p className="font-medium">
              {mode === 'build' ? 'Ask for build guidance' : 'Ask about your dashboard data'}
            </p>
            <p className="text-xs mt-1 opacity-70">
              {mode === 'build' ? 'Generated proposals are applied only after explicit confirmation.' : 'Requires a configured provider or local_mock dev/test provider.'}
            </p>
          </div>
        )}

        {mode === 'build' && (
          <div className="rounded-lg border border-border bg-background/70 p-3 text-xs">
            <p className="font-medium text-foreground">Build proposals</p>
            <p className="mt-1 text-muted-foreground">
              Ask the provider for a dashboard, widget, or workflow change. The next structured proposal will show a preview before apply{canApplyToDashboard ? '.' : ' or create a new dashboard.'}
            </p>
          </div>
        )}

        {messages.map(msg => (
          <div key={msg.id} className={`flex ${msg.role === 'user' ? 'justify-end' : 'justify-start'}`}>
            <div className={`max-w-[85%] rounded-2xl px-3.5 py-2.5 text-sm leading-relaxed ${
              msg.role === 'user'
                ? 'bg-primary text-primary-foreground rounded-br-md'
                : 'bg-muted text-foreground rounded-bl-md'
            }`}>
              {msg.content}
              {msg.metadata?.reasoning && (
                <ReasoningTrace reasoning={msg.metadata.reasoning} />
              )}
              {streamTraces[msg.id]?.synthetic && (
                <p className="mt-2 text-[10px] text-muted-foreground">Single-step provider event</p>
              )}
              {streamTraces[msg.id] && (
                <ToolTrace trace={streamTraces[msg.id]} />
              )}
              {msg.metadata?.build_proposal && (
                <ProposalPreview
                  proposal={msg.metadata.build_proposal}
                  onApply={() => onApplyBuildProposal(msg.metadata!.build_proposal!)}
                />
              )}
              {msg.tool_calls && msg.tool_calls.length > 0 && (
                <div className="mt-2 pt-2 border-t border-border/50">
                  <p className="text-xs opacity-70 flex items-center gap-1">
                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
                    </svg>
                    Used {msg.tool_calls.length} tool{msg.tool_calls.length > 1 ? 's' : ''}
                  </p>
                  {msg.tool_calls.map(call => (
                    <p key={call.id} className="mt-1 text-[10px] opacity-70">
                      {call.name}: {previewData(maskForDisplay(call.arguments))}
                    </p>
                  ))}
                  {msg.tool_results?.map(result => (
                    <p key={result.tool_call_id} className="mt-1 text-[10px] opacity-70">
                      {result.name}: {result.error ? `error - ${result.error}` : `ok - ${previewData(maskForDisplay(result.result))}`}
                    </p>
                  ))}
                </div>
              )}
              {msg.metadata && (msg.metadata.provider || msg.metadata.model || msg.metadata.latency_ms) && (
                <div className="mt-2 border-t border-border/40 pt-1 text-[10px] opacity-60">
                  {[msg.metadata.provider, msg.metadata.model, msg.metadata.latency_ms ? `${msg.metadata.latency_ms}ms` : undefined]
                    .filter(Boolean)
                    .join(' - ')}
                  {msg.metadata.tokens ? ` - ${msg.metadata.tokens.prompt}/${msg.metadata.tokens.completion} tokens` : ''}
                </div>
              )}
            </div>
          </div>
        ))}

        {isLoading && messages[messages.length - 1]?.role !== 'assistant' && (
          <div className="flex justify-start">
            <div className="bg-muted rounded-2xl rounded-bl-md px-4 py-3">
              <div className="flex gap-1.5">
                <span className="w-2 h-2 rounded-full bg-muted-foreground/50 animate-bounce" style={{ animationDelay: '0ms' }} />
                <span className="w-2 h-2 rounded-full bg-muted-foreground/50 animate-bounce" style={{ animationDelay: '150ms' }} />
                <span className="w-2 h-2 rounded-full bg-muted-foreground/50 animate-bounce" style={{ animationDelay: '300ms' }} />
              </div>
            </div>
          </div>
        )}

        <div ref={messagesEndRef} />
      </div>

      <div className="p-3 border-t border-border">
        <div className="flex items-end gap-2">
          <textarea
            ref={inputRef}
            value={input}
            onChange={handleInputChange}
            onKeyDown={handleKeyDown}
            aria-label={mode === 'build' ? 'Ask for build guidance' : 'Ask about the data'}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            spellCheck={false}
            className="flex-1 resize-none overflow-y-auto rounded-xl border border-border bg-muted/50 px-3 py-2.5 text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 min-h-[40px] max-h-32"
            rows={1}
          />
          <button
            onClick={isLoading ? handleCancel : handleSend}
            disabled={!isLoading && !input.trim()}
            className="p-2.5 rounded-xl bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-40 disabled:cursor-not-allowed transition-colors flex-shrink-0"
          >
            {isLoading ? (
              <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M7 7h10v10H7z" />
              </svg>
            ) : (
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
              </svg>
            )}
          </button>
        </div>
        <p className="text-[10px] text-muted-foreground/60 mt-1.5 text-center">Shift+Enter for new line</p>
      </div>
    </aside>
  );
}

function normalizeChatInput(value: string) {
  return value.replace(/[—–]/g, '--');
}

interface StreamTrace {
  synthetic: boolean;
  toolCalls: ToolCallTrace[];
  toolResults: ToolResultTrace[];
}

function assistantDraft(event: ChatEventEnvelope, mode: 'build' | 'context'): ChatMessage {
  return {
    id: event.message_id,
    role: 'assistant',
    content: '',
    mode,
    metadata: {
      provider: event.provider_id,
      model: event.model,
    },
    timestamp: event.emitted_at,
  };
}

function upsertMessage(messages: ChatMessage[], message: ChatMessage) {
  const index = messages.findIndex(item => item.id === message.id);
  if (index === -1) return [...messages, message];
  return messages.map(item => item.id === message.id ? message : item);
}

function updateMessage(messages: ChatMessage[], id: string, update: (message: ChatMessage) => ChatMessage) {
  return messages.map(message => message.id === id ? update(message) : message);
}

function upsertToolCall(trace: StreamTrace | undefined, toolCall: ToolCallTrace, synthetic: boolean): StreamTrace {
  const current = trace ?? { synthetic, toolCalls: [], toolResults: [] };
  const existing = current.toolCalls.findIndex(item => item.id === toolCall.id);
  return {
    ...current,
    synthetic,
    toolCalls: existing === -1
      ? [...current.toolCalls, toolCall]
      : current.toolCalls.map(item => item.id === toolCall.id ? toolCall : item),
  };
}

function upsertToolResult(trace: StreamTrace | undefined, toolResult: ToolResultTrace, synthetic: boolean): StreamTrace {
  const current = trace ?? { synthetic, toolCalls: [], toolResults: [] };
  const existing = current.toolResults.findIndex(item => item.tool_call_id === toolResult.tool_call_id);
  return {
    ...current,
    synthetic,
    toolResults: existing === -1
      ? [...current.toolResults, toolResult]
      : current.toolResults.map(item => item.tool_call_id === toolResult.tool_call_id ? toolResult : item),
  };
}

function ReasoningTrace({ reasoning }: { reasoning: string }) {
  if (!reasoning.trim()) return null;
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 p-2 text-[11px] text-muted-foreground">
      <p className="mb-1 font-medium text-foreground">Visible reasoning</p>
      <p className="whitespace-pre-wrap">{reasoning}</p>
    </div>
  );
}

function ToolTrace({ trace }: { trace: StreamTrace }) {
  if (trace.toolCalls.length === 0 && trace.toolResults.length === 0) return null;
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 p-2 text-[11px]">
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="font-medium text-foreground">Tool activity</p>
        {trace.synthetic && (
          <span className="text-[10px] text-muted-foreground">single-step provider</span>
        )}
      </div>
      <div className="space-y-1">
        {trace.toolCalls.map(call => {
          const result = trace.toolResults.find(item => item.tool_call_id === call.id);
          return (
            <div key={call.id} className="rounded border border-border/50 px-2 py-1">
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium">{call.name}</span>
                <span className={result?.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}>
                  {result?.status ?? call.status}
                </span>
              </div>
              <p className="mt-0.5 line-clamp-2 text-muted-foreground">{previewData(call.arguments_preview)}</p>
              {result && (
                <p className="mt-0.5 line-clamp-2 text-muted-foreground">
                  {result.error ? `Error: ${result.error}` : previewData(result.result_preview)}
                </p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function ProposalPreview({ proposal, onApply }: { proposal: BuildProposal; onApply: () => Promise<void> }) {
  const [isApplying, setIsApplying] = useState(false);

  const apply = async () => {
    setIsApplying(true);
    try {
      await onApply();
    } finally {
      setIsApplying(false);
    }
  };

  return (
    <div className="mt-3 rounded-lg border border-border bg-background p-3 text-xs text-foreground">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="font-medium">{proposal.title}</p>
          {proposal.dashboard_name && (
            <p className="mt-0.5 text-muted-foreground">Dashboard: {proposal.dashboard_name}</p>
          )}
        </div>
        <button
          onClick={apply}
          disabled={isApplying || proposal.widgets.length === 0}
          className="rounded-md bg-primary px-2.5 py-1.5 text-primary-foreground hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {isApplying ? 'Applying...' : 'Apply'}
        </button>
      </div>

      <div className="mt-2 space-y-1.5">
        {proposal.widgets.map((widget, index) => (
          <div key={`${widget.title}-${index}`} className="rounded-md border border-border/70 px-2 py-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="font-medium">{widget.title}</span>
              <span className="text-[10px] uppercase tracking-wide text-muted-foreground">{widget.widget_type}</span>
            </div>
            <p className="mt-1 text-[10px] text-muted-foreground">Creates a persisted datasource workflow for runtime refresh.</p>
            {widget.datasource_plan ? (
              <p className="mt-1 text-[10px] text-muted-foreground">
                {widget.datasource_plan.kind}
                {widget.datasource_plan.tool_name ? ` / ${widget.datasource_plan.tool_name}` : ''}
                {widget.datasource_plan.server_id ? ` / ${widget.datasource_plan.server_id}` : ''}
                {widget.datasource_plan.refresh_cron ? ` / ${widget.datasource_plan.refresh_cron}` : ''}
              </p>
            ) : (
              <p className="mt-1 text-[10px] text-destructive">Missing executable datasource plan</p>
            )}
            <p className="mt-1 line-clamp-2 text-[11px] text-muted-foreground">{previewData(widget.data)}</p>
          </div>
        ))}
      </div>
    </div>
  );
}

function previewData(data: unknown) {
  if (data === undefined || data === null) return 'No preview sample';
  if (typeof data === 'string') return data;
  if (typeof data === 'number') return String(data);
  try {
    return JSON.stringify(data);
  } catch {
    return 'Preview unavailable';
  }
}

function maskForDisplay(value: unknown, depth = 0): unknown {
  if (depth >= 5) return '...';
  if (Array.isArray(value)) {
    return value.slice(0, 12).map(item => maskForDisplay(item, depth + 1));
  }
  if (value && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>).slice(0, 24);
    return Object.fromEntries(entries.map(([key, item]) => [
      key,
      isSecretKey(key) ? '***' : maskForDisplay(item, depth + 1),
    ]));
  }
  if (typeof value === 'string') {
    if (looksLikeSecret(value)) return '***';
    return value.length > 240 ? `${value.slice(0, 240)}...` : value;
  }
  return value;
}

function isSecretKey(key: string) {
  const normalized = key.toLowerCase();
  return ['authorization', 'api_key', 'apikey', 'token', 'secret', 'password', 'key']
    .some(part => normalized.includes(part));
}

function looksLikeSecret(value: string) {
  const trimmed = value.trim();
  return trimmed.startsWith('Bearer ')
    || trimmed.startsWith('sk-')
    || (trimmed.length >= 32 && /^[A-Za-z0-9_.=:-]+$/.test(trimmed));
}
