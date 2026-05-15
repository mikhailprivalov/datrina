import { useState, useRef, useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { chatApi } from '../../lib/api';
import {
  appendErrorRuntimeMessage,
  appendUserRuntimeMessage,
  applyChatEvent,
  createChatRuntimeState,
  messageText,
} from '../../lib/chat/runtime';
import type { ChatRuntimeMessage } from '../../lib/chat/runtime';
import type { BuildProposal, ChatEventEnvelope, ChatMessage, ChatMessagePart, ChatSession, LLMProvider } from '../../lib/api';

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
  const [runtime, setRuntime] = useState(createChatRuntimeState([]));
  const [input, setInput] = useState('');
  const [session, setSession] = useState<ChatSession | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const modeRef = useRef(mode);

  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);

  useEffect(() => {
    let cancelled = false;
    const create = async () => {
      try {
        const s = await chatApi.createSession(mode, dashboardId);
        if (cancelled) return;
        sessionIdRef.current = s.id;
        setSession(s);
        setRuntime(createChatRuntimeState(s.messages));
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to create session:', err);
      }
    };
    create();
    return () => {
      cancelled = true;
    };
  }, [mode, dashboardId]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [runtime.messages]);

  useEffect(() => {
    const unsubscribe = listen<ChatEventEnvelope>('chat:event', event => {
      const chatEvent = event.payload;
      const isMatched = chatEvent.session_id === sessionIdRef.current;
      if (!isMatched) return;
      setRuntime(prev => applyChatEvent(prev, chatEvent, modeRef.current));
    });

    return () => {
      unsubscribe.then(dispose => dispose()).catch(err => {
        console.error('Failed to unsubscribe from chat events:', err);
      });
    };
  }, []);

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
    if (!input.trim() || !session || runtime.isLoading) return;
    const content = normalizeChatInput(input).trim();
    setInput('');
    sessionIdRef.current = session.id;

    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content,
      parts: [{ type: 'text', text: content }],
      mode,
      timestamp: Date.now(),
    };
    setRuntime(prev => appendUserRuntimeMessage(prev, userMsg));

    try {
      const assistant = await chatApi.sendMessageStream(session.id, content);
      setRuntime(prev => prev.messages.some(message => message.id === assistant.id)
        ? prev
        : {
            ...prev,
            isLoading: true,
            messages: [
              ...prev.messages,
              {
                ...createChatRuntimeState([assistant]).messages[0],
                status: 'streaming',
              },
            ],
          });
    } catch (err) {
      setRuntime(prev => appendErrorRuntimeMessage(
        prev,
        `Error: ${err instanceof Error ? err.message : String(err)}`,
        mode
      ));
    }
  };

  const handleCancel = async () => {
    if (!session || !runtime.isLoading) return;
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
        {runtime.messages.length === 0 && (
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

        {runtime.messages.map(msg => (
          <div key={msg.id} className={`flex ${msg.role === 'user' ? 'justify-end' : 'justify-start'}`}>
            <div className={`max-w-[85%] rounded-2xl px-3.5 py-2.5 text-sm leading-relaxed ${
              msg.role === 'user'
                ? 'bg-primary text-primary-foreground rounded-br-md'
                : 'bg-muted text-foreground rounded-bl-md'
            }`}>
              <MessageParts
                message={msg}
                isLoading={runtime.isLoading}
                onApplyBuildProposal={onApplyBuildProposal}
              />
              {msg.synthetic && (
                <p className="mt-2 text-[10px] text-muted-foreground">Single-step provider event</p>
              )}
              {(msg.provider || msg.model || msg.latency_ms) && (
                <div className="mt-2 border-t border-border/40 pt-1 text-[10px] opacity-60">
                  {[msg.provider, msg.model, msg.latency_ms ? `${msg.latency_ms}ms` : undefined]
                    .filter(Boolean)
                    .join(' - ')}
                  {msg.tokens ? ` - ${msg.tokens.prompt}/${msg.tokens.completion} tokens` : ''}
                </div>
              )}
            </div>
          </div>
        ))}

        {runtime.isLoading && runtime.messages[runtime.messages.length - 1]?.role !== 'assistant' && (
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
            onClick={runtime.isLoading ? handleCancel : handleSend}
            disabled={!runtime.isLoading && !input.trim()}
            className="p-2.5 rounded-xl bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-40 disabled:cursor-not-allowed transition-colors flex-shrink-0"
          >
            {runtime.isLoading ? (
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

function MessageParts({
  message,
  isLoading,
  onApplyBuildProposal,
}: {
  message: ChatRuntimeMessage;
  isLoading: boolean;
  onApplyBuildProposal: (proposal: BuildProposal) => Promise<void>;
}) {
  const text = messageText(message);
  const hasRenderableParts = message.parts.some(part => part.type !== 'text' && part.type !== 'provider_opaque_reasoning_state');
  const showWaiting = message.role === 'assistant'
    && isLoading
    && !text
    && !hasRenderableParts;
  return (
    <>
      {text || showWaiting ? (
        <div className="whitespace-pre-wrap break-words">
          {text || 'Waiting for provider response...'}
        </div>
      ) : null}
      {message.parts.map((part, index) => (
        <MessagePart
          key={`${part.type}-${index}`}
          part={part}
          onApplyBuildProposal={onApplyBuildProposal}
        />
      ))}
    </>
  );
}

function MessagePart({
  part,
  onApplyBuildProposal,
}: {
  part: ChatMessagePart;
  onApplyBuildProposal: (proposal: BuildProposal) => Promise<void>;
}) {
  switch (part.type) {
    case 'text':
    case 'provider_opaque_reasoning_state':
      return null;
    case 'visible_reasoning':
      return <ReasoningTrace reasoning={part.text} />;
    case 'tool_call':
      return <ToolCallPart part={part} />;
    case 'tool_result':
      return <ToolResultPart part={part} />;
    case 'build_proposal':
      return (
        <ProposalPreview
          proposal={part.proposal}
          onApply={() => onApplyBuildProposal(part.proposal)}
        />
      );
    case 'error':
      return <p className="mt-2 text-xs text-destructive">Error: {part.message}</p>;
    case 'cancellation':
      return <p className="mt-2 text-xs text-muted-foreground">Cancelled: {part.reason}</p>;
  }
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

function ToolCallPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_call' }> }) {
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 p-2 text-[11px]">
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="font-medium text-foreground">{part.name}</p>
        <span className={part.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}>
          {part.policy_decision} / {part.status}
        </span>
      </div>
      <p className="line-clamp-2 text-muted-foreground">{previewData(part.arguments_preview)}</p>
    </div>
  );
}

function ToolResultPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_result' }> }) {
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 p-2 text-[11px]">
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="font-medium text-foreground">{part.name} result</p>
        <span className={part.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}>{part.status}</span>
      </div>
      <p className="line-clamp-2 text-muted-foreground">
        {part.error ? `Error: ${part.error}` : previewData(part.result_preview)}
      </p>
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
