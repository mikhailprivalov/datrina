import { useState, useRef, useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { chatApi, dashboardApi } from '../../lib/api';
import {
  appendErrorRuntimeMessage,
  appendUserRuntimeMessage,
  applyChatEvent,
  createChatRuntimeState,
  messageText,
} from '../../lib/chat/runtime';
import type { ChatRuntimeMessage } from '../../lib/chat/runtime';
import type {
  AgentPhase,
  AgentPhaseEntry,
  BuildProposal,
  BuildWidgetProposal,
  ChatEventEnvelope,
  ChatMessage,
  ChatMessagePart,
  ChatSession,
  ChatSessionSummary,
  LLMProvider,
  ValidationIssue,
  WidgetDryRunResult,
} from '../../lib/api';

interface Props {
  mode: 'build' | 'context';
  dashboardId?: string;
  dashboardName?: string;
  activeProvider?: LLMProvider;
  canApplyToDashboard: boolean;
  onClose: () => void;
  onModeChange: (mode: 'build' | 'context') => void;
  onApplyBuildProposal: (proposal: BuildProposal) => Promise<void>;
}

export function ChatPanel({ mode, dashboardId, dashboardName, activeProvider, canApplyToDashboard, onClose, onModeChange, onApplyBuildProposal }: Props) {
  const [runtime, setRuntime] = useState(createChatRuntimeState([]));
  const [input, setInput] = useState('');
  const [session, setSession] = useState<ChatSession | null>(null);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [showJumpToBottom, setShowJumpToBottom] = useState(false);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [editingDraft, setEditingDraft] = useState('');
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const modeRef = useRef(mode);
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);

  useEffect(() => {
    let cancelled = false;
    const init = async () => {
      try {
        const all = await chatApi.listSessionSummaries();
        if (cancelled) return;
        const sorted = [...all].sort((a, b) => b.updated_at - a.updated_at);
        setSessions(sorted);
        const candidate = sorted.find(s =>
          s.mode === mode && (s.dashboard_id ?? null) === (dashboardId ?? null)
        );
        if (candidate) {
          const full = await chatApi.getSession(candidate.id);
          if (cancelled) return;
          sessionIdRef.current = full.id;
          setSession(full);
          setRuntime(createChatRuntimeState(full.messages));
          return;
        }
        const created = await chatApi.createSession(mode, dashboardId);
        if (cancelled) return;
        sessionIdRef.current = created.id;
        setSession(created);
        setRuntime(createChatRuntimeState(created.messages));
        setSessions(prev => [sessionToSummary(created), ...prev]);
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to init session:', err);
      }
    };
    init();
    return () => {
      cancelled = true;
    };
  }, [mode, dashboardId]);

  const refreshSessionsList = async () => {
    try {
      const all = await chatApi.listSessionSummaries();
      setSessions([...all].sort((a, b) => b.updated_at - a.updated_at));
    } catch (err) {
      console.error('Failed to refresh sessions:', err);
    }
  };

  const handleNewChat = async () => {
    if (runtime.isLoading && session) {
      try { await chatApi.cancelResponse(session.id); } catch {}
    }
    try {
      const created = await chatApi.createSession(mode, dashboardId);
      sessionIdRef.current = created.id;
      setSession(created);
      setRuntime(createChatRuntimeState(created.messages));
      setSessions(prev => [sessionToSummary(created), ...prev]);
    } catch (err) {
      console.error('Failed to create chat:', err);
    }
  };

  const handleSwitchSession = async (id: string) => {
    if (id === session?.id) return;
    if (runtime.isLoading && session) {
      try { await chatApi.cancelResponse(session.id); } catch {}
    }
    try {
      const full = await chatApi.getSession(id);
      sessionIdRef.current = full.id;
      setSession(full);
      setRuntime(createChatRuntimeState(full.messages));
    } catch (err) {
      console.error('Failed to load session:', err);
    }
  };

  const handleDeleteSession = async (id: string) => {
    try {
      await chatApi.deleteSession(id);
      setSessions(prev => prev.filter(s => s.id !== id));
      if (session?.id === id) {
        sessionIdRef.current = null;
        setSession(null);
        setRuntime(createChatRuntimeState([]));
      }
    } catch (err) {
      console.error('Failed to delete session:', err);
    }
  };

  useEffect(() => {
    if (!stickToBottomRef.current) return;
    const container = scrollContainerRef.current;
    if (!container) return;
    container.scrollTop = container.scrollHeight;
  }, [runtime.messages]);

  const handleScroll = (event: React.UIEvent<HTMLDivElement>) => {
    const el = event.currentTarget;
    const distanceFromBottom = el.scrollHeight - (el.scrollTop + el.clientHeight);
    const atBottom = distanceFromBottom < 80;
    stickToBottomRef.current = atBottom;
    setShowJumpToBottom(!atBottom);
  };

  const scrollToBottom = () => {
    const container = scrollContainerRef.current;
    if (!container) return;
    stickToBottomRef.current = true;
    container.scrollTop = container.scrollHeight;
    setShowJumpToBottom(false);
  };

  useEffect(() => {
    const unsubscribe = listen<ChatEventEnvelope>('chat:event', event => {
      const chatEvent = event.payload;
      const isMatched = chatEvent.session_id === sessionIdRef.current;
      if (!isMatched) return;
      setRuntime(prev => applyChatEvent(prev, chatEvent, modeRef.current));
      if (chatEvent.kind === 'message_completed' || chatEvent.kind === 'message_failed' || chatEvent.kind === 'message_cancelled') {
        refreshSessionsList();
      }
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

  const resubmitFromMessage = async (messageId: string, content: string) => {
    if (!session || runtime.isLoading) return;
    const trimmed = content.trim();
    if (!trimmed) return;
    try {
      const truncated = await chatApi.truncateMessages(session.id, messageId);
      setSession(truncated);
      setRuntime(createChatRuntimeState(truncated.messages));
    } catch (err) {
      console.error('Failed to truncate messages:', err);
      return;
    }

    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: trimmed,
      parts: [{ type: 'text', text: trimmed }],
      mode,
      timestamp: Date.now(),
    };
    setRuntime(prev => appendUserRuntimeMessage(prev, userMsg));

    try {
      const assistant = await chatApi.sendMessageStream(session.id, trimmed);
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

  const handleStartEdit = (messageId: string, content: string) => {
    setEditingMessageId(messageId);
    setEditingDraft(content);
  };

  const handleCancelEdit = () => {
    setEditingMessageId(null);
    setEditingDraft('');
  };

  const handleSaveEdit = async (messageId: string) => {
    const draft = editingDraft;
    setEditingMessageId(null);
    setEditingDraft('');
    await resubmitFromMessage(messageId, draft);
  };

  const handleRegenerate = async () => {
    if (!session || runtime.isLoading) return;
    const messages = runtime.messages;
    let lastUser: ChatRuntimeMessage | null = null;
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'user') {
        lastUser = messages[i];
        break;
      }
    }
    if (!lastUser) return;
    const lastUserText = messageText(lastUser);
    await resubmitFromMessage(lastUser.id, lastUserText);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex bg-card border-l border-border shadow-lg">
      {sidebarOpen && (
        <SessionsSidebar
          sessions={sessions}
          activeId={session?.id ?? null}
          onSelect={handleSwitchSession}
          onDelete={handleDeleteSession}
          onNew={handleNewChat}
          onClose={() => setSidebarOpen(false)}
        />
      )}
    <aside className="w-96 flex flex-col">
      <div className="flex items-center justify-between h-12 px-4 border-b border-border">
        <div className="flex items-center gap-2">
          {!sidebarOpen && (
            <button
              onClick={() => setSidebarOpen(true)}
              title="Show chat history"
              className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
              </svg>
            </button>
          )}
          <div className={`w-2 h-2 rounded-full ${mode === 'build' ? 'bg-amber-500' : 'bg-blue-500'}`} />
          <div className="min-w-0">
            <span className="block text-sm font-medium">
              {mode === 'build'
                ? (dashboardName ? `Editing "${dashboardName}"` : 'Build new dashboard')
                : (dashboardName ? `Context: "${dashboardName}"` : 'Context Chat')}
            </span>
            <span className="block max-w-56 truncate text-[10px] text-muted-foreground">
              {activeProvider ? `${activeProvider.name} - ${activeProvider.default_model}` : 'No LLM provider configured'}
            </span>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <button onClick={handleNewChat} title="New chat (cancels current if streaming)" className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
            </svg>
          </button>
          {runtime.isLoading && (
            <button
              onClick={() => {
                if (session) { chatApi.cancelResponse(session.id).catch(() => {}); }
                setRuntime(prev => ({ ...prev, isLoading: false }));
              }}
              title="Reset stuck loading state"
              className="p-1.5 rounded hover:bg-muted transition-colors text-amber-600 dark:text-amber-400"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
            </button>
          )}
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

      <div
        ref={scrollContainerRef}
        onScroll={handleScroll}
        className="relative flex-1 overflow-y-auto p-4 space-y-4 scrollbar-thin"
      >
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

        {runtime.messages.map((msg, msgIndex) => {
          const isEditing = editingMessageId === msg.id;
          const isLastAssistant = msg.role === 'assistant'
            && msgIndex === runtime.messages.length - 1
            && (msg.status === 'complete' || msg.status === 'failed' || msg.status === 'cancelled');
          return (
            <div key={msg.id} className={`group flex ${msg.role === 'user' ? 'justify-end' : 'justify-start'}`}>
              <div className={`relative max-w-[85%] rounded-2xl px-3.5 py-2.5 text-sm leading-relaxed ${
                msg.role === 'user'
                  ? 'bg-primary text-primary-foreground rounded-br-md'
                  : 'bg-muted text-foreground rounded-bl-md'
              }`}>
                {!isEditing && <CopyMessageButton message={msg} />}
                {!isEditing && msg.role === 'user' && !runtime.isLoading && (
                  <button
                    type="button"
                    onClick={() => handleStartEdit(msg.id, messageText(msg))}
                    title="Edit and resubmit"
                    className="absolute -top-2 -left-2 hidden group-hover:inline-flex items-center justify-center h-6 w-6 rounded-full border border-border bg-background text-muted-foreground hover:text-foreground shadow-sm"
                  >
                    <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
                      <path d="M17.414 2.586a2 2 0 00-2.828 0L7 10.172V13h2.828l7.586-7.586a2 2 0 000-2.828z" />
                      <path fillRule="evenodd" d="M2 6a2 2 0 012-2h4a1 1 0 010 2H4v10h10v-4a1 1 0 112 0v4a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" clipRule="evenodd" />
                    </svg>
                  </button>
                )}
                {isEditing ? (
                  <div className="flex flex-col gap-2 min-w-[16rem]">
                    <textarea
                      value={editingDraft}
                      onChange={e => setEditingDraft(e.target.value)}
                      autoFocus
                      rows={Math.min(8, Math.max(2, editingDraft.split('\n').length))}
                      className="w-full resize-none rounded-md border border-border bg-background/95 px-2 py-1.5 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <div className="flex justify-end gap-2 text-[11px]">
                      <button
                        type="button"
                        onClick={handleCancelEdit}
                        className="rounded-md border border-border/60 bg-background/80 px-2 py-1 text-foreground hover:bg-background"
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        onClick={() => handleSaveEdit(msg.id)}
                        disabled={!editingDraft.trim() || runtime.isLoading}
                        className="rounded-md bg-primary px-2 py-1 text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                      >
                        Save and resend
                      </button>
                    </div>
                  </div>
                ) : (
                  <MessageParts
                    message={msg}
                    isLoading={runtime.isLoading}
                    onApplyBuildProposal={onApplyBuildProposal}
                  />
                )}
                {!isEditing && msg.synthetic && (
                  <p className="mt-2 text-[10px] text-muted-foreground">Single-step provider event</p>
                )}
                {!isEditing && (msg.provider || msg.model || msg.latency_ms) && (
                  <div className="mt-2 border-t border-border/40 pt-1 text-[10px] opacity-60">
                    {[msg.provider, msg.model, msg.latency_ms ? `${msg.latency_ms}ms` : undefined]
                      .filter(Boolean)
                      .join(' - ')}
                    {msg.tokens ? ` - ${msg.tokens.prompt}/${msg.tokens.completion} tokens` : ''}
                  </div>
                )}
                {!isEditing && isLastAssistant && !runtime.isLoading && (
                  <button
                    type="button"
                    onClick={handleRegenerate}
                    title="Regenerate response"
                    className="absolute -bottom-3 -right-2 hidden group-hover:inline-flex items-center gap-1 rounded-full border border-border bg-background px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground shadow-sm"
                  >
                    <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
                      <path fillRule="evenodd" d="M4 2a1 1 0 011 1v2.101a7.002 7.002 0 0111.601 2.566 1 1 0 11-1.885.666A5.002 5.002 0 005.999 7H9a1 1 0 010 2H4a1 1 0 01-1-1V3a1 1 0 011-1zm.008 9.057a1 1 0 011.276.61A5.002 5.002 0 0014.001 13H11a1 1 0 110-2h5a1 1 0 011 1v5a1 1 0 11-2 0v-2.101a7.002 7.002 0 01-11.601-2.566 1 1 0 01.61-1.276z" clipRule="evenodd" />
                    </svg>
                    Regenerate
                  </button>
                )}
              </div>
            </div>
          );
        })}

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
        {showJumpToBottom && (
          <button
            type="button"
            onClick={scrollToBottom}
            className="sticky bottom-2 left-full -translate-x-full inline-flex items-center gap-1 rounded-full border border-border/60 bg-background/90 px-2.5 py-1 text-[11px] text-foreground shadow-sm backdrop-blur hover:bg-background"
          >
            <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
              <path fillRule="evenodd" d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z" clipRule="evenodd" />
            </svg>
            Jump to latest
          </button>
        )}
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
    </div>
  );
}

function SessionsSidebar({
  sessions,
  activeId,
  onSelect,
  onDelete,
  onNew,
  onClose,
}: {
  sessions: ChatSessionSummary[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
  onNew: () => void;
  onClose: () => void;
}) {
  return (
    <div className="w-56 flex flex-col border-r border-border bg-card">
      <div className="flex items-center justify-between h-12 px-3 border-b border-border">
        <span className="text-xs font-medium text-foreground">Chats</span>
        <div className="flex items-center gap-1">
          <button
            onClick={onNew}
            title="New chat"
            className="p-1.5 rounded hover:bg-muted text-muted-foreground"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
            </svg>
          </button>
          <button
            onClick={onClose}
            title="Hide sidebar"
            className="p-1.5 rounded hover:bg-muted text-muted-foreground"
          >
            <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
            </svg>
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-1.5 space-y-0.5 scrollbar-thin">
        {sessions.length === 0 && (
          <p className="px-2 py-3 text-[11px] text-muted-foreground">No chats yet.</p>
        )}
        {sessions.map(s => (
          <SessionRow
            key={s.id}
            session={s}
            active={s.id === activeId}
            onSelect={() => onSelect(s.id)}
            onDelete={() => onDelete(s.id)}
          />
        ))}
      </div>
    </div>
  );
}

function SessionRow({
  session,
  active,
  onSelect,
  onDelete,
}: {
  session: ChatSessionSummary;
  active: boolean;
  onSelect: () => void;
  onDelete: () => void;
}) {
  const preview = summaryPreview(session);
  return (
    <div className={`group flex items-center gap-1 rounded-md px-2 py-1.5 text-[11px] ${active ? 'bg-muted text-foreground' : 'text-muted-foreground hover:bg-muted/60'}`}>
      <button onClick={onSelect} className="min-w-0 flex-1 text-left">
        <span className={`block truncate ${session.mode === 'build' ? 'text-amber-600' : ''}`}>{preview.title}</span>
        <span className="block truncate text-[10px] opacity-70">{preview.subtitle}</span>
      </button>
      <button
        onClick={(e) => { e.stopPropagation(); if (confirm('Delete this chat?')) onDelete(); }}
        title="Delete chat"
        className="opacity-0 group-hover:opacity-100 p-1 rounded hover:bg-destructive/10 text-muted-foreground hover:text-destructive"
      >
        <svg className="w-3 w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6M1 7h22M9 7V4a1 1 0 011-1h4a1 1 0 011 1v3" />
        </svg>
      </button>
    </div>
  );
}

function summaryPreview(session: ChatSessionSummary): { title: string; subtitle: string } {
  const title = session.preview?.trim().split('\n')[0].slice(0, 60)
    || session.title
    || (session.mode === 'build' ? 'New build chat' : 'New context chat');
  const updated = new Date(session.updated_at);
  const subtitle = `${session.mode} - ${updated.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}`;
  return { title, subtitle };
}

function sessionToSummary(session: ChatSession): ChatSessionSummary {
  const firstUser = session.messages.find(m => m.role === 'user');
  return {
    id: session.id,
    mode: session.mode,
    dashboard_id: session.dashboard_id,
    widget_id: session.widget_id,
    title: session.title,
    created_at: session.created_at,
    updated_at: session.updated_at,
    message_count: session.messages.length,
    preview: firstUser?.content?.trim() || undefined,
  };
}

function normalizeChatInput(value: string) {
  return value.replace(/[—–]/g, '--');
}

function Markdown({ source, dense = false }: { source: string; dense?: boolean }) {
  const gap = dense ? 'space-y-1' : 'space-y-2';
  return (
    <div className={`${gap} break-words [overflow-wrap:anywhere]`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          p: ({ children }) => <p className="whitespace-pre-wrap">{children}</p>,
          h1: ({ children }) => <h1 className="text-base font-semibold mt-1">{children}</h1>,
          h2: ({ children }) => <h2 className="text-sm font-semibold mt-1">{children}</h2>,
          h3: ({ children }) => <h3 className="text-sm font-medium mt-1">{children}</h3>,
          h4: ({ children }) => <h4 className="text-xs font-medium mt-1 uppercase tracking-wide">{children}</h4>,
          strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
          em: ({ children }) => <em className="italic">{children}</em>,
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noopener noreferrer" className="underline text-primary">
              {children}
            </a>
          ),
          ul: ({ children }) => <ul className="list-disc pl-5 space-y-0.5">{children}</ul>,
          ol: ({ children }) => <ol className="list-decimal pl-5 space-y-0.5">{children}</ol>,
          li: ({ children }) => <li className="leading-snug">{children}</li>,
          blockquote: ({ children }) => (
            <blockquote className="border-l-2 border-border/70 pl-2 text-muted-foreground italic">{children}</blockquote>
          ),
          hr: () => <hr className="my-2 border-border/60" />,
          code: ({ className, children, ...rest }) => {
            const inline = !className;
            if (inline) {
              return <code className="rounded bg-foreground/10 px-1 py-0.5 text-[11px] font-mono" {...rest}>{children}</code>;
            }
            return <code className={`${className ?? ''} font-mono text-[11px]`} {...rest}>{children}</code>;
          },
          pre: ({ children }) => (
            <pre className="overflow-x-auto rounded-md border border-border/60 bg-background/80 p-2 text-[11px]">{children}</pre>
          ),
          table: ({ children }) => (
            <div className="overflow-x-auto">
              <table className="min-w-full text-[11px] border-collapse">{children}</table>
            </div>
          ),
          thead: ({ children }) => <thead>{children}</thead>,
          tbody: ({ children }) => <tbody>{children}</tbody>,
          tr: ({ children }) => <tr>{children}</tr>,
          th: ({ children }) => <th className="border border-border/60 px-2 py-1 bg-background/50 text-left font-medium">{children}</th>,
          td: ({ children }) => <td className="border border-border/60 px-2 py-1 align-top">{children}</td>,
        }}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}

function messageCopyText(message: ChatRuntimeMessage): string {
  const text = messageText(message);
  const reasoning = message.parts
    .filter((part): part is Extract<ChatMessagePart, { type: 'visible_reasoning' }> => part.type === 'visible_reasoning')
    .map(part => part.text)
    .join('\n');
  const proposal = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'build_proposal' }> => part.type === 'build_proposal');
  const proposalJson = proposal ? JSON.stringify(proposal.proposal, null, 2) : '';
  return [text, reasoning && `--- reasoning ---\n${reasoning}`, proposalJson && `--- proposal ---\n${proposalJson}`]
    .filter(Boolean)
    .join('\n\n');
}

function CopyMessageButton({ message }: { message: ChatRuntimeMessage }) {
  const [copied, setCopied] = useState(false);
  const onClick = async () => {
    const payload = messageCopyText(message);
    if (!payload.trim()) return;
    try {
      await navigator.clipboard.writeText(payload);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error('Copy failed:', err);
    }
  };
  return (
    <button
      type="button"
      onClick={onClick}
      title={copied ? 'Copied' : 'Copy message'}
      className={`absolute -top-2 ${message.role === 'user' ? '-left-2' : '-right-2'} hidden group-hover:inline-flex items-center justify-center h-6 w-6 rounded-full border border-border bg-background text-muted-foreground hover:text-foreground shadow-sm`}
    >
      {copied ? (
        <svg className="h-3 w-3 text-emerald-500" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M16.704 5.296a1 1 0 010 1.408l-8 8a1 1 0 01-1.408 0l-4-4a1 1 0 011.408-1.408L8 12.592l7.296-7.296a1 1 0 011.408 0z" clipRule="evenodd" />
        </svg>
      ) : (
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path d="M8 2a2 2 0 00-2 2v8a2 2 0 002 2h6a2 2 0 002-2V6.414A2 2 0 0015.414 5L13 2.586A2 2 0 0011.586 2H8z" />
          <path d="M4 6a2 2 0 012-2v10a2 2 0 002 2h6a2 2 0 01-2 2H6a2 2 0 01-2-2V6z" />
        </svg>
      )}
    </button>
  );
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
  const timelinePart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'agent_phase' }> =>
    part.type === 'agent_phase');
  const validationPart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'proposal_validation' }> =>
    part.type === 'proposal_validation');
  const renderableParts = message.parts.filter(part =>
    part.type !== 'agent_phase'
      && part.type !== 'proposal_validation'
      && part.type !== 'provider_opaque_reasoning_state'
  );
  const isStreaming = message.role === 'assistant' && (isLoading || message.status === 'streaming');
  const hasTextContent = renderableParts.some(part =>
    (part.type === 'text' && part.text.trim().length > 0)
    || part.type === 'visible_reasoning'
    || part.type === 'tool_call'
    || part.type === 'tool_result'
    || part.type === 'build_proposal'
  );
  return (
    <>
      {timelinePart && (
        <AgentTimeline
          phases={timelinePart.phases}
          collapsedDefault={hasTextContent}
          isLive={isStreaming}
        />
      )}
      {validationPart && <ProposalValidationTile part={validationPart} />}
      {renderableParts.map((part, index) => {
        if (part.type === 'text') {
          if (!part.text) return null;
          if (isProposalLikeText(part.text)) {
            const hasProposal = renderableParts.some(p => p.type === 'build_proposal');
            if (hasProposal) return null;
            return (
              <ProposalDraftBuilding
                key={`text-${index}`}
                length={part.text.length}
              />
            );
          }
          return (
            <div key={`text-${index}`}>
              <Markdown source={part.text} />
            </div>
          );
        }
        return (
          <MessagePart
            key={`${part.type}-${index}`}
            part={part}
            onApplyBuildProposal={onApplyBuildProposal}
          />
        );
      })}
      {!hasTextContent && isStreaming ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-muted-foreground/40 border-t-muted-foreground" />
          Preparing agent run...
        </div>
      ) : null}
    </>
  );
}

function isProposalLikeText(text: string): boolean {
  const trimmed = text.trimStart();
  if (!trimmed) return false;
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) return true;
  // Sometimes the model wraps JSON in ```json ... ``` fences.
  if (trimmed.startsWith('```')) return true;
  return false;
}

function ProposalDraftBuilding({ length }: { length: number }) {
  return (
    <div className="flex items-center gap-2 rounded-md border border-dashed border-border/60 bg-background/50 px-2.5 py-1.5 text-[11px] text-muted-foreground">
      <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-primary/30 border-t-primary" />
      <span>Composing dashboard proposal...</span>
      <span className="ml-auto tabular-nums opacity-60">{length} chars</span>
    </div>
  );
}

function phaseLabel(phase: AgentPhase): string {
  switch (phase.kind) {
    case 'mcp_reconnect':
      return 'Reconnecting MCP servers';
    case 'mcp_list_tools':
      return `Listing tools - ${phase.server_id}`;
    case 'provider_request':
      return 'Calling provider';
    case 'provider_first_byte':
      return 'Receiving provider response';
    case 'tool_resume':
      return `Provider tool resume #${phase.iteration}`;
    case 'loop_detected':
      return `Tool loop short-circuited (${phase.tool_name})`;
    case 'proposal_validation':
      return 'Validating proposal';
  }
}

function ProposalValidationTile({
  part,
}: {
  part: Extract<ChatMessagePart, { type: 'proposal_validation' }>;
}) {
  const { status, issues, retried } = part;
  if (status === 'completed' && issues.length === 0) {
    return (
      <div className="mt-2 flex items-center gap-2 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-2 py-1.5 text-[11px] text-emerald-700 dark:text-emerald-300">
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M16.704 5.296a1 1 0 010 1.408l-8 8a1 1 0 01-1.408 0l-4-4a1 1 0 011.408-1.408L8 12.592l7.296-7.296a1 1 0 011.408 0z" clipRule="evenodd" />
        </svg>
        <span>
          Proposal passed validation
          {retried ? ' (after one retry)' : ''}
        </span>
      </div>
    );
  }
  if (status === 'started') {
    return (
      <div className="mt-2 flex items-center gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-2 py-1.5 text-[11px] text-amber-700 dark:text-amber-300">
        <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-amber-500/30 border-t-amber-500" />
        <span>Validator found {issues.length} issue(s); retrying with agent...</span>
      </div>
    );
  }
  return (
    <div className="mt-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-[11px] text-destructive">
      <div className="mb-1 flex items-center gap-2 font-medium">
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm-1-5a1 1 0 112 0 1 1 0 01-2 0zm.293-7.707a1 1 0 011.414 0L11 6h-2L8.293 5.293z" clipRule="evenodd" />
        </svg>
        <span>
          Proposal validation failed ({issues.length} issue{issues.length === 1 ? '' : 's'}
          {retried ? ', after retry' : ''})
        </span>
      </div>
      <ul className="ml-1 list-disc space-y-0.5 pl-3 text-destructive/90">
        {issues.map((issue, index) => (
          <li key={`${issue.kind}-${index}`} className="break-words">
            {formatValidationIssue(issue)}
          </li>
        ))}
      </ul>
      <p className="mt-1 text-[10px] opacity-70">
        You can still apply this proposal, but it will likely fail when refreshed. Editing your prompt
        and re-sending usually produces a clean result.
      </p>
    </div>
  );
}

function formatValidationIssue(issue: ValidationIssue): string {
  switch (issue.kind) {
    case 'missing_datasource_plan':
      return `Widget #${issue.widget_index} "${issue.widget_title}" has no datasource_plan.`;
    case 'unknown_replace_widget_id':
      return `Widget #${issue.widget_index} "${issue.widget_title}" replaces id "${issue.replace_widget_id}", which is not on this dashboard.`;
    case 'unknown_source_key':
      return `Widget #${issue.widget_index} "${issue.widget_title}" references shared source_key "${issue.source_key || '(empty)'}", which is not declared.`;
    case 'hardcoded_literal_value':
      return `Widget #${issue.widget_index} "${issue.widget_title}" has a hardcoded value at ${issue.path}; data must come from the pipeline.`;
    case 'text_widget_contains_raw_json':
      return `Text widget #${issue.widget_index} "${issue.widget_title}" contains raw JSON instead of markdown.`;
    case 'missing_dry_run_evidence':
      return `Widget #${issue.widget_index} "${issue.widget_title}" (${issue.widget_kind}) was not validated with dry_run_widget before final.`;
    case 'pipeline_schema_invalid':
      return `Widget #${issue.widget_index} "${issue.widget_title}" has an invalid pipeline: ${issue.error}`;
    case 'duplicate_shared_key':
      return `shared_datasources contains duplicate key "${issue.key}".`;
  }
}

function PhaseStatusIcon({ status }: { status: AgentPhaseEntry['status'] }) {
  if (status === 'started') {
    return (
      <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-primary/30 border-t-primary" />
    );
  }
  if (status === 'completed') {
    return (
      <svg className="h-3 w-3 text-emerald-500" viewBox="0 0 20 20" fill="currentColor">
        <path fillRule="evenodd" d="M16.704 5.296a1 1 0 010 1.408l-8 8a1 1 0 01-1.408 0l-4-4a1 1 0 011.408-1.408L8 12.592l7.296-7.296a1 1 0 011.408 0z" clipRule="evenodd" />
      </svg>
    );
  }
  return (
    <svg className="h-3 w-3 text-destructive" viewBox="0 0 20 20" fill="currentColor">
      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z" clipRule="evenodd" />
    </svg>
  );
}

function useElapsedNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const handle = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(handle);
  }, [active]);
  return now;
}

function AgentTimeline({
  phases,
  collapsedDefault,
  isLive,
}: {
  phases: AgentPhaseEntry[];
  collapsedDefault: boolean;
  isLive: boolean;
}) {
  const [collapsed, setCollapsed] = useState(collapsedDefault);
  const anyRunning = phases.some(phase => phase.status === 'started');
  const now = useElapsedNow(isLive && anyRunning);
  useEffect(() => {
    if (collapsedDefault) setCollapsed(true);
  }, [collapsedDefault]);
  if (phases.length === 0) return null;
  if (collapsed) {
    return (
      <button
        type="button"
        onClick={() => setCollapsed(false)}
        className="mt-1 mb-2 inline-flex items-center gap-1.5 rounded-md border border-border/60 bg-background/60 px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground"
      >
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M6.293 9.293a1 1 0 011.414 0L10 11.586l2.293-2.293a1 1 0 111.414 1.414l-3 3a1 1 0 01-1.414 0l-3-3a1 1 0 010-1.414z" clipRule="evenodd" />
        </svg>
        Agent steps ({phases.length})
      </button>
    );
  }
  return (
    <div className="mb-2 rounded-md border border-border/60 bg-background/60 p-2">
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="text-[11px] font-medium text-foreground">Agent run</span>
        <button
          type="button"
          onClick={() => setCollapsed(true)}
          className="text-[10px] text-muted-foreground hover:text-foreground"
        >
          Hide
        </button>
      </div>
      <ol className="space-y-1">
        {phases.map(phase => {
          const elapsedMs = phase.status === 'started'
            ? Math.max(0, now - phase.started_at)
            : Math.max(0, (phase.finished_at ?? phase.started_at) - phase.started_at);
          return (
            <li key={phase.key} className="flex items-start gap-2 text-[11px]">
              <span className="mt-0.5 flex-shrink-0">
                <PhaseStatusIcon status={phase.status} />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline justify-between gap-2">
                  <span className="truncate font-medium text-foreground">{phaseLabel(phase.phase)}</span>
                  <span className="flex-shrink-0 text-[10px] text-muted-foreground tabular-nums">
                    {formatElapsed(elapsedMs)}
                  </span>
                </div>
                {phase.detail ? (
                  <p className={`truncate text-[10px] ${phase.status === 'failed' ? 'text-destructive' : 'text-muted-foreground'}`}>
                    {phase.detail}
                  </p>
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)}s`;
  const minutes = Math.floor(seconds / 60);
  const remaining = Math.floor(seconds % 60);
  return `${minutes}m${remaining.toString().padStart(2, '0')}s`;
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
    case 'agent_phase':
    case 'proposal_validation':
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
      return (
        <div className="mt-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive">
          <p className="font-medium">Error</p>
          <p className="mt-0.5 break-words">{part.message}</p>
          {part.recoverable ? (
            <p className="mt-1 text-[10px] opacity-70">Recoverable - you can retry from a new prompt.</p>
          ) : null}
        </div>
      );
    case 'cancellation':
      return <p className="mt-2 text-xs text-muted-foreground">Cancelled: {part.reason}</p>;
  }
}

function ReasoningTrace({ reasoning }: { reasoning: string }) {
  if (!reasoning.trim()) return null;
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 p-2 text-[11px] text-muted-foreground">
      <p className="mb-1 font-medium text-foreground">Reasoning</p>
      <Markdown source={reasoning} dense />
    </div>
  );
}

function ToolCallPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_call' }> }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 text-[11px]">
      <button
        type="button"
        onClick={() => setExpanded(v => !v)}
        className="flex w-full items-center justify-between gap-2 p-2 text-left"
        aria-expanded={expanded}
      >
        <span className="flex min-w-0 items-center gap-2">
          <svg className={`h-3 w-3 flex-shrink-0 transition-transform ${expanded ? 'rotate-90' : ''}`} viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M7.293 5.293a1 1 0 011.414 0l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414-1.414L10.586 10 7.293 6.707a1 1 0 010-1.414z" clipRule="evenodd" />
          </svg>
          <span className="truncate font-medium text-foreground">{part.name}</span>
        </span>
        <span className={`flex-shrink-0 ${part.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}`}>
          {part.policy_decision} / {part.status}
        </span>
      </button>
      {expanded ? (
        <div className="border-t border-border/40 p-2">
          <p className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">Arguments</p>
          <JsonView data={part.arguments_preview} />
        </div>
      ) : (
        <p className="px-2 pb-2 line-clamp-2 text-muted-foreground">{previewData(part.arguments_preview)}</p>
      )}
    </div>
  );
}

function ToolResultPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_result' }> }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="mt-2 rounded-md border border-border/60 bg-background/70 text-[11px]">
      <button
        type="button"
        onClick={() => setExpanded(v => !v)}
        className="flex w-full items-center justify-between gap-2 p-2 text-left"
        aria-expanded={expanded}
      >
        <span className="flex min-w-0 items-center gap-2">
          <svg className={`h-3 w-3 flex-shrink-0 transition-transform ${expanded ? 'rotate-90' : ''}`} viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M7.293 5.293a1 1 0 011.414 0l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414-1.414L10.586 10 7.293 6.707a1 1 0 010-1.414z" clipRule="evenodd" />
          </svg>
          <span className="truncate font-medium text-foreground">{part.name} result</span>
        </span>
        <span className={`flex-shrink-0 ${part.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}`}>
          {part.status}
        </span>
      </button>
      {expanded ? (
        <div className="border-t border-border/40 p-2">
          {part.error ? (
            <p className="text-destructive break-words">Error: {part.error}</p>
          ) : (
            <>
              <p className="mb-1 text-[10px] uppercase tracking-wide text-muted-foreground">Result</p>
              <JsonView data={part.result_preview} />
            </>
          )}
        </div>
      ) : (
        <p className="px-2 pb-2 line-clamp-2 text-muted-foreground">
          {part.error ? `Error: ${part.error}` : previewData(part.result_preview)}
        </p>
      )}
    </div>
  );
}

function JsonView({ data }: { data: unknown }) {
  if (data === undefined || data === null) {
    return <p className="text-muted-foreground italic">No data</p>;
  }
  const formatted = typeof data === 'string' ? data : (() => {
    try {
      return JSON.stringify(data, null, 2);
    } catch {
      return String(data);
    }
  })();
  return (
    <pre className="max-h-96 overflow-auto rounded bg-background/80 p-2 font-mono text-[10px] leading-snug whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
      {formatted}
    </pre>
  );
}

function ProposalPreview({ proposal, onApply }: { proposal: BuildProposal; onApply: () => Promise<void> }) {
  const [isApplying, setIsApplying] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [dryRuns, setDryRuns] = useState<Record<number, WidgetDryRunResult | { status: 'running' } | undefined>>({});

  const runWidget = async (index: number, widget: BuildWidgetProposal) => {
    setDryRuns(prev => ({ ...prev, [index]: { status: 'running' } }));
    try {
      const result = await dashboardApi.dryRunWidget(widget, proposal.shared_datasources);
      setDryRuns(prev => ({ ...prev, [index]: result }));
    } catch (err) {
      setDryRuns(prev => ({
        ...prev,
        [index]: {
          status: 'error',
          error: err instanceof Error ? err.message : String(err),
          duration_ms: 0,
          pipeline_steps: 0,
          has_llm_step: false,
          workflow_node_ids: [],
        },
      }));
    }
  };

  const runAll = async () => {
    await Promise.all(proposal.widgets.map((w, i) => runWidget(i, w)));
  };

  if (dismissed) {
    return (
      <div className="mt-3 rounded-lg border border-border bg-background/60 p-2 text-[11px] text-muted-foreground">
        Proposal "{proposal.title}" rejected.
      </div>
    );
  }

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
          {proposal.summary && (
            <p className="mt-1 text-[11px] text-muted-foreground">{proposal.summary}</p>
          )}
        </div>
        <div className="flex flex-shrink-0 items-center gap-1.5">
          <button
            onClick={runAll}
            className="rounded-md border border-border bg-background px-2 py-1.5 text-[11px] hover:bg-muted"
          >
            Test all
          </button>
          <button
            onClick={() => setDismissed(true)}
            className="rounded-md border border-border bg-background px-2 py-1.5 text-[11px] hover:bg-muted"
          >
            Reject
          </button>
          <button
            onClick={apply}
            disabled={isApplying || proposal.widgets.length === 0}
            className="rounded-md bg-primary px-2.5 py-1.5 text-primary-foreground hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {isApplying ? 'Applying...' : 'Apply'}
          </button>
        </div>
      </div>

      <div className="mt-2 space-y-1.5">
        {proposal.widgets.map((widget, index) => (
          <WidgetProposalRow
            key={`${widget.title}-${index}`}
            widget={widget}
            dryRun={dryRuns[index]}
            onTest={() => runWidget(index, widget)}
          />
        ))}
        {proposal.remove_widget_ids && proposal.remove_widget_ids.length > 0 && (
          <div className="rounded-md border border-destructive/40 bg-destructive/5 px-2 py-1.5 text-[11px] text-destructive">
            Removes {proposal.remove_widget_ids.length} existing widget(s) on Apply
          </div>
        )}
      </div>
    </div>
  );
}

function WidgetProposalRow({
  widget,
  dryRun,
  onTest,
}: {
  widget: BuildWidgetProposal;
  dryRun?: WidgetDryRunResult | { status: 'running' };
  onTest: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const pipelineSteps = widget.datasource_plan?.pipeline?.length ?? 0;
  const testLabel = !dryRun
    ? 'Test'
    : dryRun.status === 'running'
      ? 'Testing...'
      : dryRun.status === 'ok'
        ? 'Re-test'
        : 'Retry';
  const testTone = !dryRun
    ? 'border-border bg-background'
    : dryRun.status === 'running'
      ? 'border-border bg-muted opacity-70'
      : dryRun.status === 'ok'
        ? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
        : 'border-destructive/40 bg-destructive/10 text-destructive';
  return (
    <div className="rounded-md border border-border/70 px-2 py-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium truncate">{widget.title}</span>
        <div className="flex items-center gap-1">
          <span className="text-[10px] uppercase tracking-wide text-muted-foreground">{widget.widget_type}</span>
          <button
            onClick={onTest}
            disabled={dryRun?.status === 'running'}
            className={`rounded-md border px-2 py-0.5 text-[10px] hover:opacity-90 disabled:cursor-wait ${testTone}`}
          >
            {testLabel}
          </button>
        </div>
      </div>
      {widget.datasource_plan ? (
        <p className="mt-1 text-[10px] text-muted-foreground">
          {widget.datasource_plan.kind}
          {widget.datasource_plan.tool_name ? ` / ${widget.datasource_plan.tool_name}` : ''}
          {widget.datasource_plan.server_id ? ` / ${widget.datasource_plan.server_id}` : ''}
          {widget.datasource_plan.refresh_cron ? ` / ${widget.datasource_plan.refresh_cron}` : ''}
          {pipelineSteps > 0 ? ` / ${pipelineSteps} pipeline step(s)` : ''}
          {widget.replace_widget_id ? ' / REPLACES existing' : ''}
        </p>
      ) : (
        <p className="mt-1 text-[10px] text-destructive">Missing executable datasource plan</p>
      )}
      {dryRun && dryRun.status === 'ok' && (
        <div className="mt-1 rounded border border-emerald-500/30 bg-emerald-500/5 p-1.5">
          <button
            onClick={() => setExpanded(v => !v)}
            className="flex w-full items-center justify-between text-[10px] text-emerald-700 dark:text-emerald-300"
          >
            <span>OK · {dryRun.duration_ms}ms · {dryRun.pipeline_steps} step(s){dryRun.has_llm_step ? ' · LLM' : ''}</span>
            <span>{expanded ? 'hide' : 'show output'}</span>
          </button>
          {expanded && (
            <pre className="mt-1 max-h-40 overflow-auto rounded bg-background/60 p-1 font-mono text-[10px]">
              {JSON.stringify(dryRun.widget_runtime, null, 2)}
            </pre>
          )}
        </div>
      )}
      {dryRun && dryRun.status === 'error' && (
        <div className="mt-1 rounded border border-destructive/30 bg-destructive/5 p-1.5">
          <p className="text-[10px] text-destructive break-words">{dryRun.error ?? 'Unknown error'}</p>
        </div>
      )}
      {!dryRun && (
        <p className="mt-1 line-clamp-2 text-[11px] text-muted-foreground">{previewData(widget.data)}</p>
      )}
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
