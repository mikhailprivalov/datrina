import { useState, useRef, useEffect, useCallback, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { chatApi, costApi, dashboardApi, datasourceApi, debugApi, languageApi } from '../../lib/api';
import type {
  DatasourceDefinition,
  RawArtifactPayload,
  SessionCostSnapshot,
  SourceMention,
  ToolResultCompression,
} from '../../lib/api';
import { SessionBudgetModal } from '../chat/SessionBudgetModal';
import { AssistantLanguagePicker } from '../settings/AssistantLanguagePicker';
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
  Dashboard,
  LLMProvider,
  PlanArtifact,
  PlanStepKind,
  PlanStepStatus,
  ProposalMaterializationPreview,
  ValidationIssue,
  Widget,
  WidgetDryRunResult,
  WidgetMention,
} from '../../lib/api';

interface Props {
  mode: 'build' | 'context';
  dashboardId?: string;
  dashboardName?: string;
  activeProvider?: LLMProvider;
  canApplyToDashboard: boolean;
  /** Pre-filled input value, e.g. seeded from Playground "Use as widget" or
   * a Template Gallery selection. Consumed exactly once per change. */
  initialPrompt?: string;
  onInitialPromptConsumed?: () => void;
  /** W28: bumped by the host whenever a fresh Build chat must be opened
   * (top-bar Build "start new", Playground "use as widget", template launch).
   * Changing this value resets the active session to a local draft. */
  freshSessionKey?: number;
  onClose: () => void;
  onModeChange: (mode: 'build' | 'context') => void;
  onApplyBuildProposal: (proposal: BuildProposal, sessionId?: string) => Promise<void>;
  /** W28: open Provider Settings from chat empty/no-provider state. */
  onOpenProviderSettings?: () => void;
  /** W28: explicit honest path for Build retry/edit. Forks the prompt
   * into a fresh Build chat instead of in-place truncating the current
   * session (which would reuse plans, cost totals, and the same title).
   * Available only for Build messages; Context can still do inline edit. */
  onForkBuildChat?: (prompt: string) => void;
}

type SessionDraftKey = string;

function draftKey(mode: 'build' | 'context', dashboardId: string | undefined, sessionId: string | null): SessionDraftKey {
  return sessionId ? `s:${sessionId}` : `d:${mode}:${dashboardId ?? '-'}`;
}

export function ChatPanel({ mode, dashboardId, dashboardName, activeProvider, canApplyToDashboard, initialPrompt, onInitialPromptConsumed, freshSessionKey, onClose, onModeChange, onApplyBuildProposal, onOpenProviderSettings, onForkBuildChat }: Props) {
  const [runtime, setRuntime] = useState(createChatRuntimeState([]));
  const [input, setInput] = useState('');
  const [session, setSession] = useState<ChatSession | null>(null);
  const [sessions, setSessions] = useState<ChatSessionSummary[]>([]);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [showJumpToBottom, setShowJumpToBottom] = useState(false);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [editingDraft, setEditingDraft] = useState('');
  // W22: live cost snapshot for the footer (running session totals + today).
  const [costSnapshot, setCostSnapshot] = useState<SessionCostSnapshot | null>(null);
  const [isBudgetModalOpen, setIsBudgetModalOpen] = useState(false);
  // W47: per-session assistant language override modal.
  const [isLanguageModalOpen, setIsLanguageModalOpen] = useState(false);
  const [languageModalError, setLanguageModalError] = useState<string | null>(null);
  // W28: explicit cancellation transition distinct from `isLoading`.
  const [isCancelling, setIsCancelling] = useState(false);
  // W28: visible inline errors for session init / load / delete / send
  // failures, instead of swallowing them into the console.
  const [panelError, setPanelError] = useState<{ scope: 'init' | 'load' | 'delete' | 'send'; message: string } | null>(null);
  const [isInitialising, setIsInitialising] = useState(true);
  // W38: widget picker state for Build mode @-mentions.
  const [dashboardWidgets, setDashboardWidgets] = useState<Widget[]>([]);
  const [mentions, setMentions] = useState<WidgetMention[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerQuery, setPickerQuery] = useState('');
  // W48: source picker state for Build mode `&source` mentions. The
  // `&` trigger is dedicated to sources so it can coexist with `@` for
  // widgets without ambiguity in a single composer.
  const [savedDatasources, setSavedDatasources] = useState<DatasourceDefinition[]>([]);
  const [sourceMentions, setSourceMentions] = useState<SourceMention[]>([]);
  const [sourcePickerOpen, setSourcePickerOpen] = useState(false);
  const [sourcePickerQuery, setSourcePickerQuery] = useState('');
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const modeRef = useRef(mode);
  const stickToBottomRef = useRef(true);
  // W28: draft preservation across switch / new chat / mode toggle /
  // dashboard switch. Keyed by sessionId, or by (mode, dashboardId) for
  // not-yet-persisted drafts.
  const draftsRef = useRef<Map<SessionDraftKey, string>>(new Map());
  const draftKeyRef = useRef<SessionDraftKey>(draftKey(mode, dashboardId, null));
  // W28: latest freshSessionKey we have already consumed. A change forces
  // the panel back to a local draft instead of reloading the last session.
  const lastFreshKeyRef = useRef<number | undefined>(freshSessionKey);

  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);

  // W28: lazy session lifecycle.
  // - On mount / mode change / dashboard change: try to load the latest
  //   *non-empty* matching session. If none exists, fall back to a local
  //   draft (session === null) instead of creating an empty backend row.
  // - If the host bumped `freshSessionKey`, always start a fresh draft
  //   regardless of whatever session is on disk.
  // - Backend session is created lazily on the first send (see `handleSend`).
  useEffect(() => {
    let cancelled = false;
    const init = async () => {
      setIsInitialising(true);
      const wantsFresh = freshSessionKey !== undefined
        && freshSessionKey !== lastFreshKeyRef.current;
      try {
        const all = await chatApi.listSessionSummaries();
        if (cancelled) return;
        const sorted = [...all].sort((a, b) => b.updated_at - a.updated_at);
        setSessions(sorted);
        setPanelError(null);

        if (wantsFresh) {
          lastFreshKeyRef.current = freshSessionKey;
          openDraftSessionInternal();
          return;
        }

        const candidate = sorted.find(s =>
          s.mode === mode
            && (s.dashboard_id ?? null) === (dashboardId ?? null)
            && s.message_count > 0
        );
        if (candidate) {
          const full = await chatApi.getSession(candidate.id);
          if (cancelled) return;
          adoptSession(full);
          return;
        }
        openDraftSessionInternal();
      } catch (err) {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        console.error('Failed to init session:', err);
        setPanelError({ scope: 'init', message });
        openDraftSessionInternal();
      } finally {
        if (!cancelled) setIsInitialising(false);
      }
    };
    init();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, dashboardId, freshSessionKey]);

  // Save the current textarea into the draft map keyed by the active
  // session/draft, then switch the key to the requested target. The
  // caller is responsible for setting `session` and `runtime` after.
  const swapDraftKey = useCallback((nextKey: SessionDraftKey) => {
    const prevKey = draftKeyRef.current;
    if (prevKey !== nextKey) {
      if (input) draftsRef.current.set(prevKey, input);
      else draftsRef.current.delete(prevKey);
      draftKeyRef.current = nextKey;
      setInput(draftsRef.current.get(nextKey) ?? '');
    }
  }, [input]);

  const adoptSession = useCallback((full: ChatSession) => {
    sessionIdRef.current = full.id;
    setSession(full);
    setRuntime(createChatRuntimeState(full.messages));
    setEditingMessageId(null);
    setEditingDraft('');
    swapDraftKey(draftKey(full.mode, full.dashboard_id, full.id));
  }, [swapDraftKey]);

  const openDraftSessionInternal = useCallback(() => {
    sessionIdRef.current = null;
    setSession(null);
    setRuntime(createChatRuntimeState([]));
    setEditingMessageId(null);
    setEditingDraft('');
    swapDraftKey(draftKey(mode, dashboardId, null));
  }, [mode, dashboardId, swapDraftKey]);

  const refreshSessionsList = useCallback(async () => {
    try {
      const all = await chatApi.listSessionSummaries();
      setSessions([...all].sort((a, b) => b.updated_at - a.updated_at));
    } catch (err) {
      console.error('Failed to refresh sessions:', err);
      // non-fatal: keep showing the cached list
    }
  }, []);

  // W28: explicit cancellation with a visible "cancelling…" transition.
  // Resolves once the backend acknowledges, but the local UI already
  // unlocks isLoading optimistically so the Stop button feels responsive.
  const cancelActiveStream = useCallback(async (): Promise<boolean> => {
    if (!session || (!runtime.isLoading && !isCancelling)) return true;
    setIsCancelling(true);
    setRuntime(prev => ({ ...prev, isLoading: false }));
    try {
      await chatApi.cancelResponse(session.id);
      return true;
    } catch (err) {
      console.error('Failed to cancel chat response:', err);
      setPanelError({
        scope: 'send',
        message: err instanceof Error ? err.message : 'Failed to cancel the in-flight response.',
      });
      return false;
    } finally {
      setIsCancelling(false);
    }
  }, [isCancelling, runtime.isLoading, session]);

  const confirmDuringStream = useCallback((action: string): boolean => {
    if (!runtime.isLoading) return true;
    return window.confirm(`A response is streaming. ${action} will cancel it. Continue?`);
  }, [runtime.isLoading]);

  const handleNewChat = async () => {
    if (!confirmDuringStream('Starting a new chat')) return;
    if (runtime.isLoading) await cancelActiveStream();
    setPanelError(null);
    openDraftSessionInternal();
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const handleSwitchSession = async (id: string) => {
    if (id === session?.id) return;
    if (!confirmDuringStream('Switching session')) return;
    if (runtime.isLoading) await cancelActiveStream();
    try {
      const full = await chatApi.getSession(id);
      adoptSession(full);
      setPanelError(null);
    } catch (err) {
      console.error('Failed to load session:', err);
      setPanelError({
        scope: 'load',
        message: err instanceof Error ? err.message : 'Failed to load that chat session.',
      });
    }
  };

  const handleDeleteSession = async (id: string) => {
    const isActive = session?.id === id;
    if (isActive && !confirmDuringStream('Deleting this chat')) return;
    if (isActive && runtime.isLoading) await cancelActiveStream();
    try {
      await chatApi.deleteSession(id);
      setSessions(prev => prev.filter(s => s.id !== id));
      if (isActive) openDraftSessionInternal();
    } catch (err) {
      console.error('Failed to delete session:', err);
      setPanelError({
        scope: 'delete',
        message: err instanceof Error ? err.message : 'Failed to delete that chat session.',
      });
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
        refreshCostSnapshot();
      }
    });

    return () => {
      unsubscribe.then(dispose => dispose()).catch(err => {
        console.error('Failed to unsubscribe from chat events:', err);
      });
    };
  }, []);

  // W22: pull a fresh cost snapshot whenever the active session id
  // changes, and on a slow interval so "today" stays current across
  // long-idle panels.
  const refreshCostSnapshot = useCallback(async () => {
    const sid = sessionIdRef.current;
    if (!sid) {
      setCostSnapshot(null);
      return;
    }
    try {
      const snapshot = await costApi.getSessionSnapshot(sid);
      if (sessionIdRef.current === sid) {
        setCostSnapshot(snapshot);
      }
    } catch (err) {
      console.error('Failed to load cost snapshot:', err);
    }
  }, []);

  useEffect(() => {
    refreshCostSnapshot();
    const handle = window.setInterval(refreshCostSnapshot, 60_000);
    return () => window.clearInterval(handle);
  }, [refreshCostSnapshot, session?.id]);

  // W38: keep the active dashboard's widget list cached so the @-picker
  // resolves instantly. Refreshes when the dashboard id changes or a
  // session adoption surfaces a different dashboard.
  useEffect(() => {
    let cancelled = false;
    if (mode !== 'build' || !dashboardId) {
      setDashboardWidgets([]);
      setMentions([]);
      setPickerOpen(false);
      return () => {
        cancelled = true;
      };
    }
    (async () => {
      try {
        const dashboard: Dashboard = await dashboardApi.get(dashboardId);
        if (cancelled) return;
        setDashboardWidgets(dashboard.layout ?? []);
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to load dashboard widgets for mention picker:', err);
        setDashboardWidgets([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, dashboardId, session?.id]);

  // W38: drop stale mentions if the user switches dashboards or sessions.
  useEffect(() => {
    setMentions([]);
    setPickerOpen(false);
    setPickerQuery('');
  }, [session?.id, dashboardId, mode]);

  // W48: keep the saved datasource catalog cached for the `&` source
  // picker. We refresh whenever Build mode is entered or when the user
  // returns to a session — datasource health/labels can change between
  // turns and we want fresh metadata in the chips.
  useEffect(() => {
    let cancelled = false;
    if (mode !== 'build') {
      setSavedDatasources([]);
      setSourceMentions([]);
      setSourcePickerOpen(false);
      return () => {
        cancelled = true;
      };
    }
    (async () => {
      try {
        const defs = await datasourceApi.list();
        if (cancelled) return;
        setSavedDatasources(defs);
      } catch (err) {
        if (cancelled) return;
        console.error('Failed to load datasource catalog for source picker:', err);
        setSavedDatasources([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mode, dashboardId, session?.id]);

  // W48: drop stale source mentions on context switches.
  useEffect(() => {
    setSourceMentions([]);
    setSourcePickerOpen(false);
    setSourcePickerQuery('');
  }, [session?.id, dashboardId, mode]);

  useEffect(() => {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 128)}px`;
  }, [input]);

  useEffect(() => {
    if (!initialPrompt) return;
    setInput(initialPrompt);
    onInitialPromptConsumed?.();
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [initialPrompt, onInitialPromptConsumed]);

  const handleInputChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    const next = normalizeChatInput(event.target.value);
    setInput(next);
    // W38: open the widget picker when the cursor sits inside an
    // `@<query>` token (no spaces). W48: same UX but for `&<query>`
    // routed to the source picker.
    if (mode === 'build') {
      const cursor = event.target.selectionStart ?? next.length;
      const upto = next.slice(0, cursor);
      const sourceMatch = /(^|\s)&([\w-]*)$/.exec(upto);
      if (sourceMatch) {
        setSourcePickerOpen(true);
        setSourcePickerQuery(sourceMatch[2]);
      }
      if (dashboardId) {
        const atMatch = /(^|\s)@([\w-]*)$/.exec(upto);
        if (atMatch) {
          setPickerOpen(true);
          setPickerQuery(atMatch[2]);
        } else if (pickerOpen && !pickerQuery) {
          // user moved away from the `@` — keep the picker open only if
          // they explicitly opened it via the button.
        }
      }
    }
  };

  const canSendNow = !runtime.isLoading && !isCancelling && Boolean(activeProvider) && !isInitialising;
  const sendBlockedReason: string | null = isInitialising
    ? 'Loading chat session…'
    : !activeProvider
      ? 'No LLM provider is active. Open Provider Settings to configure OpenRouter, Ollama, or a custom OpenAI-compatible endpoint.'
      : null;

  const handleSend = async () => {
    if (!input.trim() || runtime.isLoading || isCancelling) return;
    if (!activeProvider) {
      setPanelError({
        scope: 'send',
        message: 'No LLM provider is active. Configure one before sending.',
      });
      return;
    }
    const content = normalizeChatInput(input).trim();

    // W28: lazy backend session creation. Only persist a session row
    // once the user commits a real message; drawer open / mode toggle
    // / dashboard switch never produces empty rows.
    let activeSession = session;
    if (!activeSession) {
      try {
        activeSession = await chatApi.createSession(mode, dashboardId);
        sessionIdRef.current = activeSession.id;
        setSession(activeSession);
        setRuntime(createChatRuntimeState(activeSession.messages));
        setSessions(prev => [sessionToSummary(activeSession!), ...prev]);
        swapDraftKey(draftKey(mode, dashboardId, activeSession.id));
      } catch (err) {
        console.error('Failed to create chat session for send:', err);
        setPanelError({
          scope: 'send',
          message: err instanceof Error ? err.message : 'Could not start a new chat session.',
        });
        return;
      }
    }
    setInput('');
    draftsRef.current.delete(draftKeyRef.current);
    sessionIdRef.current = activeSession.id;
    setPanelError(null);

    // W38/W48: snapshot mention chips at send-time and clear the
    // composer chip rail so the next turn starts fresh.
    const turnMentions = mode === 'build' ? mentions : [];
    const turnSourceMentions = mode === 'build' ? sourceMentions : [];
    setMentions([]);
    setPickerOpen(false);
    setPickerQuery('');
    setSourceMentions([]);
    setSourcePickerOpen(false);
    setSourcePickerQuery('');

    const userParts: ChatMessagePart[] = [{ type: 'text', text: content }];
    if (turnMentions.length > 0) {
      userParts.push({ type: 'widget_mentions', mentions: turnMentions });
    }
    if (turnSourceMentions.length > 0) {
      userParts.push({ type: 'source_mentions', mentions: turnSourceMentions });
    }
    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content,
      parts: userParts,
      mode,
      timestamp: Date.now(),
    };
    setRuntime(prev => appendUserRuntimeMessage(prev, userMsg));

    try {
      const assistant = await chatApi.sendMessageStream(
        activeSession.id,
        content,
        turnMentions,
        turnSourceMentions,
      );
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
      const message = err instanceof Error ? err.message : String(err);
      setRuntime(prev => appendErrorRuntimeMessage(prev, `Error: ${message}`, mode));
      setPanelError({ scope: 'send', message });
    }
  };

  const handleCancel = async () => {
    if (!runtime.isLoading) return;
    await cancelActiveStream();
  };

  const resubmitFromMessage = async (messageId: string, content: string) => {
    if (!session || runtime.isLoading) return;
    const trimmed = content.trim();
    if (!trimmed) return;
    // W38: preserve the original mention scope when regenerating /
    // editing a Build user message so the agent keeps the same target
    // set across retries. W48: same for source mentions.
    const originalMentions: WidgetMention[] = (() => {
      const original = runtime.messages.find(m => m.id === messageId);
      if (!original) return [];
      const part = original.parts.find(p => p.type === 'widget_mentions');
      return part && part.type === 'widget_mentions' ? part.mentions : [];
    })();
    const originalSourceMentions: SourceMention[] = (() => {
      const original = runtime.messages.find(m => m.id === messageId);
      if (!original) return [];
      const part = original.parts.find(p => p.type === 'source_mentions');
      return part && part.type === 'source_mentions' ? part.mentions : [];
    })();
    try {
      const truncated = await chatApi.truncateMessages(session.id, messageId);
      setSession(truncated);
      setRuntime(createChatRuntimeState(truncated.messages));
    } catch (err) {
      console.error('Failed to truncate messages:', err);
      setPanelError({
        scope: 'send',
        message: err instanceof Error ? err.message : 'Failed to truncate session before resend.',
      });
      return;
    }

    const resubmitParts: ChatMessagePart[] = [{ type: 'text', text: trimmed }];
    if (originalMentions.length > 0) {
      resubmitParts.push({ type: 'widget_mentions', mentions: originalMentions });
    }
    if (originalSourceMentions.length > 0) {
      resubmitParts.push({ type: 'source_mentions', mentions: originalSourceMentions });
    }
    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: trimmed,
      parts: resubmitParts,
      mode,
      timestamp: Date.now(),
    };
    setRuntime(prev => appendUserRuntimeMessage(prev, userMsg));

    try {
      const assistant = await chatApi.sendMessageStream(
        session.id,
        trimmed,
        originalMentions,
        originalSourceMentions,
      );
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
      const message = err instanceof Error ? err.message : String(err);
      setRuntime(prev => appendErrorRuntimeMessage(prev, `Error: ${message}`, mode));
      setPanelError({ scope: 'send', message });
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
    if (e.key === 'Escape' && (pickerOpen || sourcePickerOpen)) {
      e.preventDefault();
      setPickerOpen(false);
      setPickerQuery('');
      setSourcePickerOpen(false);
      setSourcePickerQuery('');
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex bg-card/95 backdrop-blur-sm border-l border-border shadow-2xl">
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
      <div className="flex items-center justify-between h-12 px-3 border-b border-border bg-muted/20">
        <div className="flex items-center gap-2 min-w-0">
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
          <span
            className={`inline-flex h-5 items-center rounded-sm px-1.5 text-[10px] mono font-semibold uppercase tracking-wider border ${mode === 'build' ? 'bg-neon-amber/15 text-neon-amber border-neon-amber/40' : 'bg-primary/15 text-primary border-primary/40'}`}
            title={mode === 'build' ? 'Build mode' : 'Context mode'}
          >
            {mode === 'build' ? 'build' : 'ctx'}
          </span>
          <div className="min-w-0">
            <span className="block text-sm font-semibold tracking-tight truncate">
              {mode === 'build'
                ? (dashboardName ? `Editing "${dashboardName}"` : 'Build new dashboard')
                : (dashboardName ? `Context: "${dashboardName}"` : 'Context Chat')}
            </span>
            <span className="block max-w-56 truncate text-[10px] mono uppercase tracking-wider text-muted-foreground">
              {activeProvider ? `${activeProvider.name} · ${activeProvider.default_model}` : 'no provider'}
            </span>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <button
            onClick={handleNewChat}
            title={runtime.isLoading ? 'New chat — will cancel the streaming response' : 'New chat'}
            aria-label="Start new chat"
            className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
            </svg>
          </button>
          {isCancelling && (
            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-sm border border-amber-500/40 bg-amber-500/10 text-[10px] mono uppercase tracking-wider text-amber-500" aria-live="polite">
              <span className="inline-block h-2 w-2 animate-spin rounded-full border border-amber-500/40 border-t-amber-500" />
              cancelling
            </span>
          )}
          <button
            onClick={() => {
              if (!confirmDuringStream('Switching mode')) return;
              onModeChange(mode === 'build' ? 'context' : 'build');
            }}
            title={mode === 'build' ? 'Switch to Context chat' : 'Switch to Build chat'}
            aria-label={mode === 'build' ? 'Switch to Context chat' : 'Switch to Build chat'}
            className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 7h12m0 0l-4-4m4 4l-4 4m0 6H4m0 0l4 4m-4-4l4-4" />
            </svg>
          </button>
          {session && (
            <button
              onClick={() => setIsBudgetModalOpen(true)}
              title={costSnapshot?.max_cost_usd != null
                ? `Session budget: $${costSnapshot.max_cost_usd.toFixed(2)}`
                : 'Set session cost budget'}
              className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
            </button>
          )}
          {session && (
            <button
              onClick={() => {
                setLanguageModalError(null);
                setIsLanguageModalOpen(true);
              }}
              title={
                session.language_override == null
                  ? 'Assistant language: inheriting dashboard / app default'
                  : session.language_override.mode === 'auto'
                  ? 'Assistant language: auto (session)'
                  : `Assistant language: ${session.language_override.tag} (session)`
              }
              aria-label="Set session assistant language"
              className="p-1.5 rounded hover:bg-muted transition-colors text-muted-foreground"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M3 5h12M9 3v2m1.048 9.5A18.022 18.022 0 016.412 9m6.088 9h7M11 21l5-10 5 10M12.751 5C11.783 10.77 8.07 15.61 3 18.129" />
              </svg>
            </button>
          )}
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
        {/* W28: surface init / load / delete / send failures inline so
            the panel never looks idle while errors pile up in the
            console. Includes a retry hook when applicable. */}
        {panelError && (
          <ChatErrorBanner
            error={panelError}
            onDismiss={() => setPanelError(null)}
            onRetry={panelError.scope === 'init' || panelError.scope === 'load'
              ? () => { setPanelError(null); refreshSessionsList(); }
              : undefined}
          />
        )}

        {/* W28 / W29: explicit, visible no-provider remediation. Send is
            disabled below; this banner names the fix. `local_mock` is no
            longer a product option — operators must configure a real
            provider (OpenRouter, Ollama, or a custom OpenAI-compatible
            endpoint) before sending. */}
        {!activeProvider && !isInitialising && (
          <div className="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs">
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-destructive">// no provider</p>
            <p className="mt-1.5 text-muted-foreground">
              No LLM provider is active. Configure OpenRouter, Ollama, or a custom OpenAI-compatible endpoint before sending — chat fails closed without one.
            </p>
            {onOpenProviderSettings && (
              <button
                type="button"
                onClick={onOpenProviderSettings}
                className="mt-2 rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-[10px] mono uppercase tracking-wider text-destructive hover:bg-destructive/15"
              >
                Open Provider Settings
              </button>
            )}
          </div>
        )}

        {/* W28: when the panel reloaded the latest *non-empty* session
            for this dashboard, give the user a one-click escape into a
            fresh draft. Avoids the "I clicked Build and silently
            landed in an old conversation" surprise. */}
        {session && runtime.messages.length > 0 && (
          <div className="flex items-center justify-between gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-1.5 text-[11px]">
            <span className="truncate text-muted-foreground mono">
              <span className="uppercase tracking-wider text-[10px] mr-1.5 opacity-70">// continuing</span>
              {session.title?.trim() || (session.mode === 'build' ? 'last build chat' : 'last context chat')}
            </span>
            <button
              type="button"
              onClick={handleNewChat}
              className="rounded-md border border-border bg-background px-2 py-0.5 text-[10px] mono uppercase tracking-wider text-muted-foreground hover:text-primary hover:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
            >
              Start fresh
            </button>
          </div>
        )}

        {runtime.messages.length === 0 && !isInitialising && (
          <div className="text-center text-muted-foreground text-sm mt-8">
            <div className="relative inline-flex w-12 h-12 rounded-md bg-primary/10 border border-primary/30 items-center justify-center mb-3">
              <svg className="w-6 h-6 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
              </svg>
            </div>
            <p className="font-semibold text-foreground">
              {mode === 'build' ? 'Ask for build guidance' : 'Ask about your dashboard data'}
            </p>
            <p className="text-xs mt-1 opacity-70">
              {mode === 'build'
                ? (session ? 'Generated proposals are applied only after explicit confirmation.' : 'Fresh Build draft — a session will be created when you send your first message.')
                : 'Requires a configured OpenRouter, Ollama, or custom OpenAI-compatible provider.'}
            </p>
          </div>
        )}

        {mode === 'build' && runtime.messages.length === 0 && !isInitialising && (
          <div className="rounded-md border border-neon-amber/30 bg-neon-amber/5 p-3 text-xs">
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-neon-amber">// build proposals</p>
            <p className="mt-1.5 text-muted-foreground">
              Ask the provider for a dashboard, widget, or workflow change. The next structured proposal will show a preview before apply{canApplyToDashboard ? '.' : ' or create a new dashboard.'}
            </p>
          </div>
        )}

        {runtime.messages.map((msg, msgIndex) => {
          const isEditing = editingMessageId === msg.id;
          const isLastAssistant = msg.role === 'assistant'
            && msgIndex === runtime.messages.length - 1
            && (msg.status === 'complete' || msg.status === 'failed' || msg.status === 'cancelled');
          // W18: hide internal reflection trigger user messages — they are
          // not authored by the human and the follow-up assistant turn
          // already carries a reflection badge.
          if (msg.role === 'user' && messageText(msg).startsWith('[reflection]')) {
            return (
              <div key={msg.id} className="flex justify-center">
                <span className="mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                  // auto self-check triggered
                </span>
              </div>
            );
          }
          const isBuildUser = msg.role === 'user' && mode === 'build';
          // W28: Build retry/edit honesty.
          // In Build mode in-place edit/regenerate would reuse derived
          // plan state, cost totals, and the same title — the backend
          // `truncate_chat_messages` only truncates *messages*. Until
          // that contract is widened, route Build retries through
          // "Fork to fresh Build chat" (handled by host App).
          const showInlineEdit = !isEditing && msg.role === 'user' && !isBuildUser && !runtime.isLoading;
          const showForkBuild = !isEditing && isBuildUser && Boolean(onForkBuildChat);
          const showRegenerate = !isEditing && isLastAssistant && !runtime.isLoading && mode === 'context';
          return (
            <div key={msg.id} className={`group flex flex-col ${msg.role === 'user' ? 'items-end' : 'items-start'}`}>
              <div className={`relative max-w-[85%] rounded-md px-3.5 py-2.5 text-sm leading-relaxed border ${
                msg.role === 'user'
                  ? 'bg-primary/15 text-foreground border-primary/40 rounded-br-sm'
                  : 'bg-muted/40 text-foreground border-border rounded-bl-sm'
              }`}>
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
                    sessionId={session?.id}
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
              </div>
              {/* W28: stable action row beneath the bubble. Visible on
                  hover, focus-within, or keyboard focus — no more
                  overlapping absolute hover targets on the corners. */}
              {!isEditing && (
                <MessageActionRow
                  message={msg}
                  side={msg.role === 'user' ? 'end' : 'start'}
                  onCopy={() => navigator.clipboard.writeText(messageCopyText(msg)).catch(err => console.error('Copy failed:', err))}
                  onEdit={showInlineEdit ? () => handleStartEdit(msg.id, messageText(msg)) : undefined}
                  onForkBuild={showForkBuild ? () => onForkBuildChat!(messageText(msg)) : undefined}
                  onRegenerate={showRegenerate ? handleRegenerate : undefined}
                />
              )}
            </div>
          );
        })}

        {runtime.isLoading && runtime.messages[runtime.messages.length - 1]?.role !== 'assistant' && (
          <div className="flex justify-start">
            <div className="bg-muted/40 border border-border rounded-md rounded-bl-sm px-4 py-3">
              <div className="flex gap-1.5">
                <span className="w-1.5 h-1.5 rounded-full bg-primary animate-bounce" style={{ animationDelay: '0ms' }} />
                <span className="w-1.5 h-1.5 rounded-full bg-primary animate-bounce" style={{ animationDelay: '150ms' }} />
                <span className="w-1.5 h-1.5 rounded-full bg-primary animate-bounce" style={{ animationDelay: '300ms' }} />
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

      {costSnapshot && (
        <div className="px-3 pt-2 pb-1 border-t border-border/60 text-[10px] mono text-muted-foreground/80 flex items-center justify-between gap-2 bg-muted/20">
          <span className="truncate uppercase tracking-wider" title={formatCostFooterTitle(costSnapshot)}>
            {formatCostFooter(costSnapshot)}
          </span>
          {costSnapshot.max_cost_usd != null && costSnapshot.max_cost_usd > 0 && (
            <span
              className={`uppercase tracking-wider ${
                costSnapshot.cost_usd >= costSnapshot.max_cost_usd
                  ? 'text-destructive'
                  : 'text-muted-foreground/70'
              }`}
            >
              cap ${costSnapshot.max_cost_usd.toFixed(2)}
            </span>
          )}
        </div>
      )}
      <div className="p-3 border-t border-border bg-muted/20">
        {mode === 'build' && (mentions.length > 0 || pickerOpen) && (
          <MentionComposerBar
            mentions={mentions}
            widgets={dashboardWidgets}
            dashboardId={dashboardId}
            pickerOpen={pickerOpen}
            pickerQuery={pickerQuery}
            onPickerQueryChange={setPickerQuery}
            onAdd={mention => {
              setMentions(prev =>
                prev.some(m => m.widget_id === mention.widget_id)
                  ? prev
                  : [...prev, mention]
              );
            }}
            onRemove={widgetId => setMentions(prev => prev.filter(m => m.widget_id !== widgetId))}
            onClose={() => { setPickerOpen(false); setPickerQuery(''); }}
          />
        )}
        {mode === 'build' && (sourceMentions.length > 0 || sourcePickerOpen) && (
          <SourceMentionComposerBar
            mentions={sourceMentions}
            datasources={savedDatasources}
            widgets={dashboardWidgets}
            dashboardId={dashboardId}
            pickerOpen={sourcePickerOpen}
            pickerQuery={sourcePickerQuery}
            onPickerQueryChange={setSourcePickerQuery}
            onAdd={mention => {
              setSourceMentions(prev => {
                const key = sourceMentionKey(mention);
                return prev.some(m => sourceMentionKey(m) === key)
                  ? prev
                  : [...prev, mention];
              });
            }}
            onRemove={key =>
              setSourceMentions(prev => prev.filter(m => sourceMentionKey(m) !== key))
            }
            onClose={() => {
              setSourcePickerOpen(false);
              setSourcePickerQuery('');
            }}
          />
        )}
        <div className="flex items-end gap-2">
          {mode === 'build' && (
            <button
              type="button"
              onClick={() => {
                if (!dashboardId) return;
                setPickerOpen(open => !open);
                setPickerQuery('');
              }}
              disabled={!dashboardId || isCancelling || isInitialising}
              title={!dashboardId
                ? 'Mentions require an active dashboard'
                : 'Mention a widget on this dashboard (@)'}
              aria-label="Mention a widget"
              className="p-2.5 rounded-md border border-border bg-card text-muted-foreground hover:text-foreground hover:border-primary/40 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              <span className="mono text-xs">@</span>
            </button>
          )}
          {mode === 'build' && (
            <button
              type="button"
              onClick={() => {
                setSourcePickerOpen(open => !open);
                setSourcePickerQuery('');
              }}
              disabled={isCancelling || isInitialising}
              title="Mention a saved datasource, workflow, or widget-backed source (&)"
              aria-label="Mention a data source"
              className="p-2.5 rounded-md border border-border bg-card text-muted-foreground hover:text-foreground hover:border-primary/40 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              <span className="mono text-xs">&amp;</span>
            </button>
          )}
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
            disabled={isCancelling || isInitialising}
            placeholder={sendBlockedReason ?? (mode === 'build' ? 'Describe what to build…' : 'Ask about your data…')}
            className="flex-1 resize-none overflow-y-auto rounded-md border border-border bg-card px-3 py-2.5 text-sm focus:outline-none focus:border-primary/60 disabled:opacity-60 disabled:cursor-not-allowed min-h-[40px] max-h-32"
            rows={1}
          />
          <button
            onClick={runtime.isLoading ? handleCancel : handleSend}
            disabled={runtime.isLoading ? isCancelling : (!input.trim() || !canSendNow)}
            title={runtime.isLoading
              ? (isCancelling ? 'Cancelling…' : 'Cancel current response')
              : (sendBlockedReason ?? 'Send')}
            aria-label={runtime.isLoading ? 'Cancel response' : 'Send message'}
            className={`p-2.5 rounded-md disabled:opacity-40 disabled:cursor-not-allowed transition-all flex-shrink-0 border focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40 ${
              runtime.isLoading
                ? 'bg-destructive/15 text-destructive border-destructive/40 hover:bg-destructive/25'
                : 'bg-primary text-primary-foreground border-primary hover:glow-primary'
            }`}
          >
            {runtime.isLoading ? (
              isCancelling ? (
                <span className="inline-block h-4 w-4 animate-spin rounded-full border-2 border-destructive/30 border-t-destructive" aria-hidden />
              ) : (
                <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24" aria-hidden>
                  <path d="M7 7h10v10H7z" />
                </svg>
              )
            ) : (
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden>
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
              </svg>
            )}
          </button>
        </div>
        <p className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60 mt-1.5 text-center">
          {sendBlockedReason ? sendBlockedReason : 'Shift+Enter for newline'}
        </p>
      </div>
    </aside>
    {isBudgetModalOpen && session && (
      <SessionBudgetModal
        sessionId={session.id}
        currentMaxCostUsd={costSnapshot?.max_cost_usd ?? null}
        currentSpentUsd={costSnapshot?.cost_usd ?? session.total_cost_usd ?? 0}
        onClose={() => setIsBudgetModalOpen(false)}
        onSaved={(updated: ChatSession) => {
          setIsBudgetModalOpen(false);
          setSession(updated);
          refreshCostSnapshot();
        }}
      />
    )}
    {isLanguageModalOpen && session && (
      <div
        className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4 backdrop-blur-sm"
        onClick={() => setIsLanguageModalOpen(false)}
      >
        <div
          className="w-full max-w-md space-y-3 rounded-md border border-border bg-card p-4 shadow-2xl"
          onClick={event => event.stopPropagation()}
        >
          <div>
            <p className="mono text-[10px] uppercase tracking-[0.18em] text-primary">// language</p>
            <h2 className="mt-0.5 text-base font-semibold tracking-tight">Session assistant language</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Overrides the dashboard / app default for this chat. Choose
              "Inherit" to fall back to the wider scope.
            </p>
          </div>
          <AssistantLanguagePicker
            value={session.language_override ?? null}
            allowInherit
            label="Language"
            onChange={async next => {
              try {
                const updated = await languageApi.setSessionPolicy(session.id, next);
                setSession(updated);
                setLanguageModalError(null);
                setIsLanguageModalOpen(false);
              } catch (err) {
                setLanguageModalError(err instanceof Error ? err.message : String(err));
              }
            }}
          />
          {languageModalError && (
            <p className="text-[11px] text-destructive">{languageModalError}</p>
          )}
          <div className="flex justify-end">
            <button
              type="button"
              onClick={() => setIsLanguageModalOpen(false)}
              className="rounded-md border border-border px-2.5 py-1.5 text-xs hover:bg-muted"
            >
              Close
            </button>
          </div>
        </div>
      </div>
    )}
    </div>
  );
}

function formatCostFooter(snapshot: SessionCostSnapshot): string {
  const tokens = `${formatTokens(snapshot.input_tokens)} in / ${formatTokens(snapshot.output_tokens)} out`
    + (snapshot.reasoning_tokens > 0 ? ` / ${formatTokens(snapshot.reasoning_tokens)} think` : '');
  // W49: when pricing was unknown for at least one turn, surface that
  // honestly instead of pretending the session was free / cheap.
  const unknownTurns = snapshot.cost_unknown_turns ?? 0;
  let cost: string;
  if (unknownTurns > 0 && snapshot.cost_usd <= 0) {
    cost = ' · unknown cost';
  } else if (unknownTurns > 0) {
    cost = ` · ≥$${snapshot.cost_usd.toFixed(4)}*`;
  } else {
    cost = ` · $${snapshot.cost_usd.toFixed(4)}`;
  }
  const today = ` · today $${snapshot.today_cost_usd.toFixed(2)}`;
  const model = snapshot.model ? `${snapshot.model} · ` : '';
  return `${model}${tokens}${cost}${today}`;
}

function formatCostFooterTitle(snapshot: SessionCostSnapshot): string {
  const unknownTurns = snapshot.cost_unknown_turns ?? 0;
  const sessionCostLine = unknownTurns > 0 && snapshot.cost_usd <= 0
    ? 'Session cost: unknown (no pricing entry matched this model)'
    : unknownTurns > 0
      ? `Session cost: ≥$${snapshot.cost_usd.toFixed(4)} (lower bound — ${unknownTurns} unpriced turn${unknownTurns === 1 ? '' : 's'})`
      : `Session cost: $${snapshot.cost_usd.toFixed(4)}`;
  const sourceLine = snapshot.latest_cost_source
    ? `Cost source: ${costSourceLabel(snapshot.latest_cost_source)}`
    : null;
  return [
    snapshot.model ? `Model: ${snapshot.model}` : null,
    `Prompt: ${snapshot.input_tokens} tokens`,
    `Completion: ${snapshot.output_tokens} tokens`,
    snapshot.reasoning_tokens > 0 ? `Reasoning: ${snapshot.reasoning_tokens} tokens` : null,
    sessionCostLine,
    sourceLine,
    `Today total: $${snapshot.today_cost_usd.toFixed(4)}`,
    snapshot.max_cost_usd != null ? `Cap: $${snapshot.max_cost_usd.toFixed(2)}` : null,
  ]
    .filter(Boolean)
    .join('\n');
}

function costSourceLabel(source: NonNullable<SessionCostSnapshot['latest_cost_source']>): string {
  switch (source) {
    case 'provider_total':
      return 'provider total (upstream billing)';
    case 'pricing_table':
      return 'local pricing table';
    case 'unknown_pricing':
      return 'unknown — pricing entry missing';
  }
}

function formatTokens(count: number): string {
  if (count < 1000) return `${count}`;
  if (count < 10_000) return `${(count / 1000).toFixed(1)}k`;
  return `${Math.round(count / 1000)}k`;
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
    <div className="w-56 flex flex-col border-r border-border bg-card/95">
      <div className="flex items-center justify-between h-12 px-3 border-b border-border bg-muted/20">
        <span className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// chats</span>
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
    <div className={`group flex items-center gap-1 rounded-md px-2 py-1.5 text-[11px] border ${active ? 'bg-primary/10 border-primary/30 text-foreground' : 'border-transparent text-muted-foreground hover:bg-muted/40 hover:border-border'}`}>
      <button onClick={onSelect} className="min-w-0 flex-1 text-left">
        <span className={`block truncate ${session.mode === 'build' ? 'text-neon-amber' : ''}`}>{preview.title}</span>
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

// W28: stable per-message action row replacing the overlapping
// hover-only absolute buttons. Always rendered, low-opacity by default,
// promoted on hover/focus. Always reachable by keyboard with visible
// focus rings, and each action carries an aria-label.
function MessageActionRow({
  message,
  side,
  onCopy,
  onEdit,
  onForkBuild,
  onRegenerate,
}: {
  message: ChatRuntimeMessage;
  side: 'start' | 'end';
  onCopy: () => void;
  onEdit?: () => void;
  onForkBuild?: () => void;
  onRegenerate?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const hasCopyableContent = messageCopyText(message).trim().length > 0;
  const justify = side === 'end' ? 'justify-end' : 'justify-start';
  const handleCopy = async () => {
    if (!hasCopyableContent) return;
    onCopy();
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };
  const cls = "inline-flex items-center gap-1 rounded-md border border-border bg-background px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider text-muted-foreground hover:text-foreground hover:border-primary/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40 focus-visible:opacity-100";
  return (
    <div className={`mt-1 flex items-center gap-1 ${justify} opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity`}>
      {hasCopyableContent && (
        <button type="button" onClick={handleCopy} aria-label={copied ? 'Copied' : 'Copy message'} title={copied ? 'Copied' : 'Copy message'} className={cls}>
          {copied ? 'copied' : 'copy'}
        </button>
      )}
      {onEdit && (
        <button type="button" onClick={onEdit} aria-label="Edit and resend" title="Edit and resend" className={cls}>
          edit
        </button>
      )}
      {onForkBuild && (
        <button type="button" onClick={onForkBuild} aria-label="Fork prompt into a fresh Build chat" title="Build retry uses a fresh chat (avoids reusing plan/cost state)" className={cls}>
          fork build
        </button>
      )}
      {onRegenerate && (
        <button type="button" onClick={onRegenerate} aria-label="Regenerate response" title="Regenerate response" className={cls}>
          regenerate
        </button>
      )}
    </div>
  );
}

// W28: inline error banner for init / load / delete / send failures.
// Replaces the previous console.error-only path so the user can see
// what broke and retry where it makes sense.
function ChatErrorBanner({
  error,
  onDismiss,
  onRetry,
}: {
  error: { scope: 'init' | 'load' | 'delete' | 'send'; message: string };
  onDismiss: () => void;
  onRetry?: () => void;
}) {
  const label =
    error.scope === 'init' ? 'Could not load chat history' :
    error.scope === 'load' ? 'Could not open that chat' :
    error.scope === 'delete' ? 'Could not delete chat' :
    'Send failed';
  return (
    <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-2 text-[11px]" role="alert">
      <svg className="mt-0.5 h-3 w-3 flex-shrink-0 text-destructive" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01M4.93 19h14.14a2 2 0 001.73-3l-7.07-12a2 2 0 00-3.46 0l-7.07 12a2 2 0 001.73 3z" />
      </svg>
      <div className="min-w-0 flex-1">
        <p className="mono uppercase tracking-wider text-[10px] text-destructive">// {label}</p>
        <p className="mt-0.5 text-foreground break-words">{error.message}</p>
      </div>
      <div className="flex flex-shrink-0 items-center gap-1">
        {onRetry && (
          <button type="button" onClick={onRetry} className="rounded-md border border-destructive/40 bg-background px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider text-destructive hover:bg-destructive/10">
            Retry
          </button>
        )}
        <button type="button" onClick={onDismiss} aria-label="Dismiss error" className="rounded-md p-1 text-muted-foreground hover:text-foreground hover:bg-muted">
          <svg className="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>
    </div>
  );
}

function MessageParts({
  message,
  isLoading,
  sessionId,
  onApplyBuildProposal,
}: {
  message: ChatRuntimeMessage;
  isLoading: boolean;
  sessionId?: string;
  onApplyBuildProposal: (proposal: BuildProposal, sessionId?: string) => Promise<void>;
}) {
  const timelinePart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'agent_phase' }> =>
    part.type === 'agent_phase');
  const validationPart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'proposal_validation' }> =>
    part.type === 'proposal_validation');
  const planPart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'plan' }> =>
    part.type === 'plan');
  const reflectionPart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'reflection_meta' }> =>
    part.type === 'reflection_meta');
  const mentionsPart = message.parts.find((part): part is Extract<ChatMessagePart, { type: 'widget_mentions' }> =>
    part.type === 'widget_mentions');
  const sourceMentionsPart = message.parts.find(
    (part): part is Extract<ChatMessagePart, { type: 'source_mentions' }> =>
      part.type === 'source_mentions',
  );
  const renderableParts = message.parts.filter(part =>
    part.type !== 'agent_phase'
      && part.type !== 'proposal_validation'
      && part.type !== 'plan'
      && part.type !== 'reflection_meta'
      && part.type !== 'widget_mentions'
      && part.type !== 'source_mentions'
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
  const hasFixupProposal = renderableParts.some(p => p.type === 'build_proposal');
  const body = (
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
            if (hasFixupProposal) return null;
            if (isStreaming) {
              return (
                <ProposalDraftBuilding
                  key={`text-${index}`}
                  length={part.text.length}
                />
              );
            }
            return (
              <ProposalDraftSuppressed
                key={`text-${index}`}
                text={part.text}
                hasValidation={!!validationPart}
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
            sessionId={sessionId}
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
  // W18: when the reflection turn produced no fix-up proposal, collapse
  // the whole body under the badge — it's just an "ok, looks good" note
  // and clutters the chat after Apply.
  if (reflectionPart && !hasFixupProposal && !isStreaming) {
    return (
      <>
        {planPart && (
          <PlanArtifactTile plan={planPart.plan} status={planPart.status} isLive={isStreaming} />
        )}
        <ReflectionCollapsible widgetIds={reflectionPart.widget_ids}>{body}</ReflectionCollapsible>
      </>
    );
  }
  return (
    <>
      {planPart && (
        <PlanArtifactTile
          plan={planPart.plan}
          status={planPart.status}
          isLive={isStreaming}
        />
      )}
      {reflectionPart && <ReflectionBadge widgetIds={reflectionPart.widget_ids} />}
      {mentionsPart && <WidgetMentionChips mentions={mentionsPart.mentions} />}
      {sourceMentionsPart && <SourceMentionChips mentions={sourceMentionsPart.mentions} />}
      {body}
    </>
  );
}

function MentionComposerBar({
  mentions,
  widgets,
  dashboardId,
  pickerOpen,
  pickerQuery,
  onPickerQueryChange,
  onAdd,
  onRemove,
  onClose,
}: {
  mentions: WidgetMention[];
  widgets: Widget[];
  dashboardId?: string;
  pickerOpen: boolean;
  pickerQuery: string;
  onPickerQueryChange: (value: string) => void;
  onAdd: (mention: WidgetMention) => void;
  onRemove: (widgetId: string) => void;
  onClose: () => void;
}) {
  const selectedIds = new Set(mentions.map(m => m.widget_id));
  const q = pickerQuery.trim().toLowerCase();
  const filtered = widgets
    .filter(w => !selectedIds.has(w.id))
    .filter(w => {
      if (!q) return true;
      return w.title.toLowerCase().includes(q) || w.type.toLowerCase().includes(q);
    })
    .slice(0, 12);
  const labelFor = (widget: Widget) => {
    const baseLabel = widget.title.trim() || widget.id.slice(0, 8);
    const dupes = widgets.filter(w => w.title.trim() === widget.title.trim() && w.id !== widget.id);
    return dupes.length > 0
      ? `${baseLabel} (${widget.id.slice(0, 6)})`
      : baseLabel;
  };
  return (
    <div className="mb-2 rounded-md border border-border bg-card/80 p-2">
      {mentions.length > 0 && (
        <div className="mb-1 flex flex-wrap items-center gap-1">
          {mentions.map(mention => (
            <span
              key={mention.widget_id}
              className="inline-flex items-center gap-1 rounded-md border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[10px] mono text-primary"
              title={`widget_id: ${mention.widget_id}`}
            >
              @{mention.label || mention.widget_id.slice(0, 8)}
              {mention.widget_kind ? <span className="opacity-60">· {mention.widget_kind}</span> : null}
              <button
                type="button"
                onClick={() => onRemove(mention.widget_id)}
                aria-label={`Remove mention ${mention.label}`}
                className="ml-0.5 rounded hover:bg-primary/20 px-0.5"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      {pickerOpen && (
        <div>
          <div className="flex items-center gap-2">
            <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">// mention widget</span>
            <input
              type="text"
              value={pickerQuery}
              onChange={e => onPickerQueryChange(e.target.value)}
              placeholder="Filter by title or kind…"
              autoFocus
              className="flex-1 min-w-0 rounded border border-border bg-background px-2 py-1 text-[11px] focus:outline-none focus:border-primary/60"
            />
            <button
              type="button"
              onClick={onClose}
              aria-label="Close mention picker"
              className="rounded border border-border bg-card px-1.5 py-0.5 text-[10px] mono uppercase hover:bg-muted"
            >
              Esc
            </button>
          </div>
          {!dashboardId ? (
            <p className="mt-1 text-[10px] text-muted-foreground">
              Open or create a dashboard to mention its widgets.
            </p>
          ) : widgets.length === 0 ? (
            <p className="mt-1 text-[10px] text-muted-foreground">
              No widgets on this dashboard yet.
            </p>
          ) : filtered.length === 0 ? (
            <p className="mt-1 text-[10px] text-muted-foreground">
              No widgets match {pickerQuery ? `"${pickerQuery}"` : 'the filter'}.
            </p>
          ) : (
            <ul className="mt-1 max-h-40 overflow-y-auto rounded border border-border/60 bg-background/60 text-[11px]">
              {filtered.map(widget => (
                <li key={widget.id}>
                  <button
                    type="button"
                    onClick={() => {
                      onAdd({
                        widget_id: widget.id,
                        dashboard_id: dashboardId,
                        label: labelFor(widget),
                        widget_kind: widget.type,
                      });
                    }}
                    className="flex w-full items-center justify-between gap-2 px-2 py-1 text-left hover:bg-muted/60"
                  >
                    <span className="truncate font-medium">{widget.title.trim() || '(untitled)'}</span>
                    <span className="flex items-center gap-2 text-[10px] mono text-muted-foreground">
                      <span>{widget.type}</span>
                      <span className="opacity-60">{widget.id.slice(0, 6)}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function WidgetMentionChips({ mentions }: { mentions: Extract<ChatMessagePart, { type: 'widget_mentions' }>['mentions'] }) {
  if (mentions.length === 0) return null;
  return (
    <div className="mb-1 flex flex-wrap items-center gap-1">
      <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">// targets</span>
      {mentions.map(mention => (
        <span
          key={mention.widget_id}
          title={`widget_id: ${mention.widget_id}`}
          className="inline-flex items-center gap-1 rounded-md border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[10px] mono text-primary"
        >
          @{mention.label || mention.widget_id.slice(0, 8)}
          {mention.widget_kind ? (
            <span className="opacity-60">· {mention.widget_kind}</span>
          ) : null}
        </span>
      ))}
    </div>
  );
}

/** W48: stable identifier for a SourceMention (used for dedupe + chip
 *  removal). Prefers the saved-datasource id so two mentions of the
 *  same source via different surfaces (catalog vs. widget) collapse. */
function sourceMentionKey(mention: SourceMention): string {
  const id =
    mention.datasource_definition_id ??
    mention.workflow_id ??
    mention.widget_id ??
    mention.label;
  return `${mention.kind}:${id}`;
}

function slugifyAlias(label: string, taken: Set<string>): string {
  const base = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 32) || 'source';
  let candidate = base;
  let n = 2;
  while (taken.has(candidate)) {
    candidate = `${base}_${n}`;
    n += 1;
  }
  return candidate;
}

function SourceMentionComposerBar({
  mentions,
  datasources,
  widgets,
  dashboardId,
  pickerOpen,
  pickerQuery,
  onPickerQueryChange,
  onAdd,
  onRemove,
  onClose,
}: {
  mentions: SourceMention[];
  datasources: DatasourceDefinition[];
  widgets: Widget[];
  dashboardId?: string;
  pickerOpen: boolean;
  pickerQuery: string;
  onPickerQueryChange: (value: string) => void;
  onAdd: (mention: SourceMention) => void;
  onRemove: (key: string) => void;
  onClose: () => void;
}) {
  const selectedKeys = new Set(mentions.map(sourceMentionKey));
  const aliasesTaken = new Set(
    mentions.map(m => m.input_alias).filter((s): s is string => Boolean(s)),
  );
  const q = pickerQuery.trim().toLowerCase();
  type Candidate = {
    label: string;
    sub: string;
    kind: SourceMention['kind'];
    build: () => SourceMention;
  };
  const candidates: Candidate[] = [];
  for (const def of datasources) {
    candidates.push({
      label: def.name,
      sub: `${def.kind}${def.tool_name ? ` · ${def.tool_name}` : ''}`,
      kind: 'datasource',
      build: () => ({
        kind: 'datasource',
        label: def.name,
        datasource_definition_id: def.id,
        workflow_id: def.workflow_id,
        input_alias: slugifyAlias(def.name, new Set(aliasesTaken)),
      }),
    });
  }
  if (dashboardId) {
    for (const widget of widgets) {
      const title = widget.title.trim() || `widget ${widget.id.slice(0, 6)}`;
      candidates.push({
        label: title,
        sub: `widget · ${widget.type}`,
        kind: 'widget',
        build: () => ({
          kind: 'widget',
          label: title,
          widget_id: widget.id,
          dashboard_id: dashboardId,
          input_alias: slugifyAlias(title, new Set(aliasesTaken)),
        }),
      });
    }
  }
  const filtered = candidates
    .filter(c => {
      const built = c.build();
      return !selectedKeys.has(sourceMentionKey(built));
    })
    .filter(c => {
      if (!q) return true;
      return (
        c.label.toLowerCase().includes(q) ||
        c.sub.toLowerCase().includes(q) ||
        c.kind.includes(q)
      );
    })
    .slice(0, 16);
  return (
    <div className="mb-2 rounded-md border border-border bg-card/80 p-2">
      {mentions.length > 0 && (
        <div className="mb-1 flex flex-wrap items-center gap-1">
          {mentions.map(mention => {
            const key = sourceMentionKey(mention);
            return (
              <span
                key={key}
                className="inline-flex items-center gap-1 rounded-md border border-neon-cyan/40 bg-neon-cyan/10 px-1.5 py-0.5 text-[10px] mono text-neon-cyan"
                title={
                  mention.datasource_definition_id
                    ? `datasource_definition_id: ${mention.datasource_definition_id}`
                    : mention.workflow_id
                      ? `workflow_id: ${mention.workflow_id}`
                      : mention.widget_id
                        ? `widget_id: ${mention.widget_id}`
                        : mention.label
                }
              >
                &amp;{mention.label || key.split(':')[1].slice(0, 8)}
                {mention.input_alias ? (
                  <span className="opacity-60">·{mention.input_alias}</span>
                ) : null}
                <button
                  type="button"
                  onClick={() => onRemove(key)}
                  aria-label={`Remove source mention ${mention.label}`}
                  className="ml-0.5 rounded hover:bg-neon-cyan/20 px-0.5"
                >
                  ×
                </button>
              </span>
            );
          })}
        </div>
      )}
      {pickerOpen && (
        <div>
          <div className="flex items-center gap-2">
            <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">// mention source</span>
            <input
              type="text"
              value={pickerQuery}
              onChange={e => onPickerQueryChange(e.target.value)}
              placeholder="Filter saved datasources or dashboard widgets…"
              autoFocus
              className="flex-1 min-w-0 rounded border border-border bg-background px-2 py-1 text-[11px] focus:outline-none focus:border-primary/60"
            />
            <button
              type="button"
              onClick={onClose}
              aria-label="Close source picker"
              className="rounded border border-border bg-card px-1.5 py-0.5 text-[10px] mono uppercase hover:bg-muted"
            >
              Esc
            </button>
          </div>
          {candidates.length === 0 ? (
            <p className="mt-1 text-[10px] text-muted-foreground">
              No saved datasources yet. Save one from the Workbench, the Playground, or a Build proposal first.
            </p>
          ) : filtered.length === 0 ? (
            <p className="mt-1 text-[10px] text-muted-foreground">
              No sources match {pickerQuery ? `"${pickerQuery}"` : 'the filter'}.
            </p>
          ) : (
            <ul className="mt-1 max-h-48 overflow-y-auto rounded border border-border/60 bg-background/60 text-[11px]">
              {filtered.map((candidate, idx) => (
                <li key={`${candidate.kind}-${candidate.label}-${idx}`}>
                  <button
                    type="button"
                    onClick={() => onAdd(candidate.build())}
                    className="flex w-full items-center justify-between gap-2 px-2 py-1 text-left hover:bg-muted/60"
                  >
                    <span className="truncate font-medium">{candidate.label}</span>
                    <span className="flex items-center gap-2 text-[10px] mono text-muted-foreground">
                      <span>{candidate.sub}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function SourceMentionChips({
  mentions,
}: {
  mentions: Extract<ChatMessagePart, { type: 'source_mentions' }>['mentions'];
}) {
  if (mentions.length === 0) return null;
  return (
    <div className="mb-1 flex flex-wrap items-center gap-1">
      <span className="mono text-[10px] uppercase tracking-wider text-muted-foreground">// sources</span>
      {mentions.map(mention => {
        const key = sourceMentionKey(mention);
        const id =
          mention.datasource_definition_id ??
          mention.workflow_id ??
          mention.widget_id ??
          mention.label;
        return (
          <span
            key={key}
            title={`${mention.kind}: ${id}`}
            className="inline-flex items-center gap-1 rounded-md border border-neon-cyan/40 bg-neon-cyan/10 px-1.5 py-0.5 text-[10px] mono text-neon-cyan"
          >
            &amp;{mention.label || id.slice(0, 8)}
            {mention.input_alias ? <span className="opacity-60">·{mention.input_alias}</span> : null}
          </span>
        );
      })}
    </div>
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
    <div className="flex items-center gap-2 rounded-md border border-dashed border-primary/30 bg-primary/5 px-2.5 py-1.5 text-[11px] text-muted-foreground">
      <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-primary/30 border-t-primary" />
      <span className="mono uppercase tracking-wider text-[10px] text-primary">composing proposal</span>
      <span className="ml-auto tabular mono opacity-60">{length} chars</span>
    </div>
  );
}

function ProposalDraftSuppressed({
  text,
  hasValidation,
}: {
  text: string;
  hasValidation: boolean;
}) {
  return (
    <details className="rounded-md border border-dashed border-muted-foreground/30 bg-muted/20 px-2.5 py-1.5 text-[11px] text-muted-foreground">
      <summary className="flex cursor-pointer items-center gap-2">
        <span className="mono uppercase tracking-wider text-[10px]">
          proposal not applied
        </span>
        <span className="ml-auto tabular mono opacity-60">{text.length} chars</span>
      </summary>
      <p className="mt-1 text-[10px] opacity-70">
        {hasValidation
          ? 'See validation issues above. The raw model output is preserved below for debugging.'
          : 'The model produced JSON-like output that was not parsed into a proposal. Raw output below.'}
      </p>
      <pre className="mt-1 max-h-48 overflow-auto rounded border border-border bg-background/60 p-2 mono text-[10px] leading-snug whitespace-pre-wrap break-all">
        {text}
      </pre>
    </details>
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
    case 'plan_enforcement':
      return 'Plan enforcement';
  }
}

function PlanArtifactTile({
  plan,
  status,
  isLive,
}: {
  plan: PlanArtifact;
  status: Record<string, PlanStepStatus>;
  isLive: boolean;
}) {
  const [expanded, setExpanded] = useState(true);
  const total = plan.steps.length;
  const done = plan.steps.filter(step => status[step.id] === 'done').length;
  const running = plan.steps.find(step => status[step.id] === 'running');
  const failed = plan.steps.some(step => status[step.id] === 'failed');
  const summaryLabel = failed
    ? 'Plan failed'
    : running
      ? `Running ${running.id}`
      : isLive
        ? 'Plan ready'
        : 'Plan complete';
  return (
    <div className={`mt-1 mb-2 rounded-md border ${failed ? 'border-destructive/40 bg-destructive/5' : 'border-neon-amber/30 bg-neon-amber/5'} p-2 text-[11px]`}>
      <button
        type="button"
        onClick={() => setExpanded(v => !v)}
        className="flex w-full items-center justify-between gap-2 text-left"
      >
        <span className="flex items-center gap-1.5 font-semibold text-foreground">
          <svg className={`h-3 w-3 transition-transform ${expanded ? 'rotate-90' : ''}`} viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M7.293 5.293a1 1 0 011.414 0l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414-1.414L10.586 10 7.293 6.707a1 1 0 010-1.414z" clipRule="evenodd" />
          </svg>
          <span className="mono uppercase tracking-wider text-[10px] text-neon-amber">// plan</span>
          <span className="tabular text-foreground/80">{done}/{total}</span>
        </span>
        <span className="text-muted-foreground mono text-[10px] uppercase tracking-wider">{summaryLabel}</span>
      </button>
      {expanded && (
        <>
          {plan.summary && (
            <p className="mt-1 text-muted-foreground">{plan.summary}</p>
          )}
          <ol className="mt-1 space-y-0.5">
            {plan.steps.map(step => {
              const s = status[step.id] ?? 'pending';
              return (
                <li key={step.id} className="flex items-start gap-2 leading-snug">
                  <span className="mt-0.5 flex-shrink-0">
                    <PlanStepIcon status={s} />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline justify-between gap-2">
                      <span className={`truncate ${s === 'failed' ? 'text-destructive' : 'text-foreground'}`}>
                        <span className="font-medium">{step.id}</span>
                        <span className="ml-1 text-muted-foreground">·</span>
                        <span className="ml-1">{step.title}</span>
                      </span>
                      <span className="flex-shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                        {planKindLabel(step.kind)}
                      </span>
                    </div>
                    {step.rationale && (
                      <p className="text-[10px] text-muted-foreground">{step.rationale}</p>
                    )}
                  </div>
                </li>
              );
            })}
          </ol>
        </>
      )}
    </div>
  );
}

function PlanStepIcon({ status }: { status: PlanStepStatus }) {
  if (status === 'running') {
    return <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-neon-amber/30 border-t-neon-amber" />;
  }
  if (status === 'done') {
    return (
      <svg className="h-3 w-3 text-neon-lime" viewBox="0 0 20 20" fill="currentColor">
        <path fillRule="evenodd" d="M16.704 5.296a1 1 0 010 1.408l-8 8a1 1 0 01-1.408 0l-4-4a1 1 0 011.408-1.408L8 12.592l7.296-7.296a1 1 0 011.408 0z" clipRule="evenodd" />
      </svg>
    );
  }
  if (status === 'failed') {
    return (
      <svg className="h-3 w-3 text-destructive" viewBox="0 0 20 20" fill="currentColor">
        <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z" clipRule="evenodd" />
      </svg>
    );
  }
  return <span className="inline-block h-3 w-3 rounded-full border border-muted-foreground/40" />;
}

function planKindLabel(kind: PlanStepKind): string {
  switch (kind) {
    case 'explore': return 'explore';
    case 'fetch': return 'fetch';
    case 'design': return 'design';
    case 'test': return 'test';
    case 'propose': return 'propose';
    case 'other': return 'other';
  }
}

function ReflectionBadge({ widgetIds }: { widgetIds: string[] }) {
  return (
    <div className="mt-1 mb-2 flex items-center gap-2 rounded-md border border-primary/30 bg-primary/5 px-2 py-1 text-[11px] text-primary">
      <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
        <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm-1-5a1 1 0 112 0 1 1 0 01-2 0zM9 6a1 1 0 112 0v4a1 1 0 11-2 0V6z" clipRule="evenodd" />
      </svg>
      <span>
        Reflection suggestion — agent reviewed {widgetIds.length} widget{widgetIds.length === 1 ? '' : 's'} after first refresh.
      </span>
    </div>
  );
}

function ReflectionCollapsible({ widgetIds, children }: { widgetIds: string[]; children: ReactNode }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="mt-1 mb-1 rounded-md border border-primary/20 bg-primary/5 text-[11px] text-primary">
      <button
        type="button"
        onClick={() => setExpanded(v => !v)}
        className="flex w-full items-center justify-between gap-2 px-2 py-1 mono uppercase tracking-wider hover:bg-primary/10 transition-colors"
      >
        <span className="flex items-center gap-2">
          <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
            <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm-1-5a1 1 0 112 0 1 1 0 01-2 0zM9 6a1 1 0 112 0v4a1 1 0 11-2 0V6z" clipRule="evenodd" />
          </svg>
          <span className="text-[10px]">// self-check passed · {widgetIds.length} widget{widgetIds.length === 1 ? '' : 's'}</span>
        </span>
        <span className="text-[10px] opacity-70">{expanded ? 'hide' : 'show'}</span>
      </button>
      {expanded && (
        <div className="border-t border-primary/20 px-2 py-1.5 text-foreground">{children}</div>
      )}
    </div>
  );
}

function ProposalValidationTile({
  part,
}: {
  part: Extract<ChatMessagePart, { type: 'proposal_validation' }>;
}) {
  const { status, issues, retried } = part;
  if (status === 'completed' && issues.length === 0) {
    return (
      <div className="mt-2 flex items-center gap-2 rounded-md border border-neon-lime/30 bg-neon-lime/5 px-2 py-1.5 text-[11px] text-neon-lime">
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M16.704 5.296a1 1 0 010 1.408l-8 8a1 1 0 01-1.408 0l-4-4a1 1 0 011.408-1.408L8 12.592l7.296-7.296a1 1 0 011.408 0z" clipRule="evenodd" />
        </svg>
        <span className="mono uppercase tracking-wider text-[10px]">validation passed{retried ? ' (after retry)' : ''}</span>
      </div>
    );
  }
  if (status === 'started') {
    return (
      <div className="mt-2 flex items-center gap-2 rounded-md border border-neon-amber/30 bg-neon-amber/5 px-2 py-1.5 text-[11px] text-neon-amber">
        <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-neon-amber/30 border-t-neon-amber" />
        <span className="mono uppercase tracking-wider text-[10px]">retrying · {issues.length} issue{issues.length === 1 ? '' : 's'}</span>
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
        Apply is blocked for this proposal — edit your prompt and re-send. Backend rejects validation-failed proposals too, so the dashboard never sees them.
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
    case 'unknown_parameter_reference':
      return `Widget #${issue.widget_index} "${issue.widget_title}" references parameter "$${issue.param_name}" that is not declared.`;
    case 'parameter_cycle':
      return `Dashboard parameters form a depends_on cycle: ${issue.cycle.join(' → ')}.`;
    case 'off_target_widget_replace':
      return `Widget #${issue.widget_index} "${issue.widget_title}" replaces id "${issue.replace_widget_id}", which was not mentioned this turn.`;
    case 'off_target_widget_remove':
      return `Proposal removes widget id "${issue.remove_widget_id}", which was not mentioned this turn.`;
    case 'unsafe_http_datasource':
      return `Unsafe http_request ${issue.source_kind} datasource for widget #${issue.widget_index} "${issue.widget_title}": ${issue.reason}.`;
    case 'hardcoded_gallery_items':
      return `Gallery widget #${issue.widget_index} "${issue.widget_title}" embeds ${issue.item_count} hardcoded image items; items must come from the datasource pipeline.`;
    case 'proposed_explicit_coordinates':
      return `Widget #${issue.widget_index} "${issue.widget_title}" sets explicit x/y. Drop them — auto-pack owns placement on the 12-col grid.`;
    case 'conflicting_layout_fields':
      return `Widget #${issue.widget_index} "${issue.widget_title}" sets both size_preset and w/h. Pick one — prefer size_preset.`;
    case 'unused_source_mention': {
      const labels = issue.missing
        .map((m) => {
          const id = m.datasource_definition_id ?? m.workflow_id;
          return id ? `${m.label} (${id})` : m.label;
        })
        .join(', ');
      return `Proposal did not consume mentioned source(s): ${labels}. Use kind="compose" to combine inputs or bind the widget to one of them.`;
    }
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
      <svg className="h-3 w-3 text-neon-lime" viewBox="0 0 20 20" fill="currentColor">
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
        className="mt-1 mb-2 inline-flex items-center gap-1.5 rounded-md border border-border bg-muted/30 px-2 py-1 text-[10px] mono uppercase tracking-wider text-muted-foreground hover:text-primary hover:border-primary/40 transition-colors"
      >
        <svg className="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
          <path fillRule="evenodd" d="M6.293 9.293a1 1 0 011.414 0L10 11.586l2.293-2.293a1 1 0 111.414 1.414l-3 3a1 1 0 01-1.414 0l-3-3a1 1 0 010-1.414z" clipRule="evenodd" />
        </svg>
        agent steps · {phases.length}
      </button>
    );
  }
  return (
    <div className="mb-2 rounded-md border border-border bg-muted/30 p-2">
      <div className="mb-1 flex items-center justify-between gap-2">
        <span className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// agent run</span>
        <button
          type="button"
          onClick={() => setCollapsed(true)}
          className="text-[10px] mono uppercase tracking-wider text-muted-foreground hover:text-primary transition-colors"
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
  sessionId,
  onApplyBuildProposal,
}: {
  part: ChatMessagePart;
  sessionId?: string;
  onApplyBuildProposal: (proposal: BuildProposal, sessionId?: string) => Promise<void>;
}) {
  switch (part.type) {
    case 'text':
    case 'provider_opaque_reasoning_state':
    case 'agent_phase':
    case 'proposal_validation':
    case 'plan':
    case 'reflection_meta':
    case 'widget_mentions':
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
          onApply={() => onApplyBuildProposal(part.proposal, sessionId)}
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
    <div className="mt-2 rounded-md border border-neon-violet/30 bg-neon-violet/5 p-2 text-[11px] text-muted-foreground">
      <p className="mb-1 mono uppercase tracking-wider text-[10px] text-neon-violet">// reasoning</p>
      <Markdown source={reasoning} dense />
    </div>
  );
}

function ToolCallPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_call' }> }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="mt-2 rounded-md border border-border bg-muted/30 text-[11px]">
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
          <span className="truncate font-semibold text-primary mono">{part.name}</span>
        </span>
        <span className={`flex-shrink-0 text-[10px] mono uppercase tracking-wider ${part.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}`}>
          {part.policy_decision} · {part.status}
        </span>
      </button>
      {expanded ? (
        <div className="border-t border-border p-2">
          <p className="mb-1 text-[10px] mono uppercase tracking-wider text-muted-foreground">// arguments</p>
          <JsonView data={part.arguments_preview} />
        </div>
      ) : (
        <p className="px-2 pb-2 line-clamp-2 text-muted-foreground mono">{previewData(part.arguments_preview)}</p>
      )}
    </div>
  );
}

function ToolResultPart({ part }: { part: Extract<ChatMessagePart, { type: 'tool_result' }> }) {
  const [expanded, setExpanded] = useState(false);
  const compression = part.compression;
  return (
    <div className="mt-2 rounded-md border border-border bg-muted/30 text-[11px]">
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
          <span className="truncate font-semibold mono text-foreground">{part.name} <span className="text-muted-foreground">→</span></span>
        </span>
        <span className="flex flex-shrink-0 items-center gap-2">
          {compression ? <CompressionBadge compression={compression} /> : null}
          <span className={`text-[10px] mono uppercase tracking-wider ${part.status === 'error' ? 'text-destructive' : part.status === 'success' ? 'text-neon-lime' : 'text-muted-foreground'}`}>
            {part.status}
          </span>
        </span>
      </button>
      {expanded ? (
        <div className="border-t border-border p-2">
          {part.error ? (
            <p className="text-destructive break-words">Error: {part.error}</p>
          ) : (
            <>
              <p className="mb-1 text-[10px] mono uppercase tracking-wider text-muted-foreground">// compact result (sent to provider)</p>
              <JsonView data={part.result_preview} />
              {compression ? <CompressionDetail compression={compression} /> : null}
            </>
          )}
        </div>
      ) : (
        <p className="px-2 pb-2 line-clamp-2 text-muted-foreground mono">
          {part.error ? `Error: ${part.error}` : previewData(part.result_preview)}
        </p>
      )}
    </div>
  );
}

/**
 * W51: compact compression chip surfaced on the collapsed tool-result
 * header so users see at a glance how much the provider request was
 * shrunk for this call.
 */
function CompressionBadge({ compression }: { compression: ToolResultCompression }) {
  const ratio = compression.raw_bytes > 0
    ? Math.round((1 - compression.compact_bytes / compression.raw_bytes) * 100)
    : 0;
  return (
    <span
      className="rounded-sm border border-border bg-card/40 px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider text-muted-foreground"
      title={`${compression.profile} · raw ${formatBytes(compression.raw_bytes)} → ${formatBytes(compression.compact_bytes)} sent · saved ~${compression.estimated_tokens_saved} tokens`}
    >
      {ratio >= 0 ? `-${ratio}%` : `+${Math.abs(ratio)}%`}
    </span>
  );
}

/**
 * W51: expanded compression detail. Shows raw/compact bytes,
 * truncation paths, and a "view raw locally" button that streams the
 * redacted raw payload back through `debugApi.getRawArtifact`.
 */
function CompressionDetail({ compression }: { compression: ToolResultCompression }) {
  const [raw, setRaw] = useState<RawArtifactPayload | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loadRaw = async () => {
    if (!compression.raw_artifact_id) return;
    setLoading(true);
    setError(null);
    try {
      const payload = await debugApi.getRawArtifact(compression.raw_artifact_id);
      setRaw(payload);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };
  return (
    <div className="mt-2 rounded-sm border border-border/60 bg-card/40 p-2 text-[10px] mono text-muted-foreground">
      <div className="flex flex-wrap gap-x-3 gap-y-1">
        <span>profile: {compression.profile}</span>
        <span>raw: {formatBytes(compression.raw_bytes)}</span>
        <span>compact: {formatBytes(compression.compact_bytes)}</span>
        <span>saved ~{compression.estimated_tokens_saved} tokens</span>
      </div>
      {compression.truncation_paths.length > 0 ? (
        <div className="mt-1">
          <span className="uppercase tracking-wider">truncated paths:</span>{' '}
          {compression.truncation_paths.join(', ')}
        </div>
      ) : null}
      {compression.raw_artifact_id ? (
        <div className="mt-2 flex items-center gap-2">
          <button
            type="button"
            className="rounded-sm border border-border bg-muted/40 px-2 py-1 text-[10px] uppercase tracking-wider text-foreground hover:bg-muted/60"
            onClick={loadRaw}
            disabled={loading}
          >
            {loading ? 'loading…' : raw ? 'reload raw' : 'view raw locally'}
          </button>
          <span className="text-[10px]">artifact: {compression.raw_artifact_id.slice(0, 8)}…</span>
        </div>
      ) : null}
      {error ? <p className="mt-2 text-destructive">{error}</p> : null}
      {raw ? (
        <div className="mt-2">
          <p className="mb-1 uppercase tracking-wider">// raw payload (redacted)</p>
          <pre className="max-h-72 overflow-auto rounded bg-card/70 border border-border/60 p-2 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
            {raw.payload_json}
          </pre>
        </div>
      ) : null}
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
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
    <pre className="max-h-96 overflow-auto rounded bg-card/70 border border-border/60 p-2 mono text-[10px] leading-snug whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
      {formatted}
    </pre>
  );
}

function ProposalPreview({ proposal, onApply }: { proposal: BuildProposal; onApply: () => Promise<void> }) {
  const [isApplying, setIsApplying] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [dryRuns, setDryRuns] = useState<Record<number, WidgetDryRunResult | { status: 'running' } | undefined>>({});
  const [materialization, setMaterialization] = useState<ProposalMaterializationPreview | null>(null);
  const [materializationError, setMaterializationError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setMaterialization(null);
    setMaterializationError(null);
    dashboardApi
      .previewProposalMaterialization(proposal)
      .then(result => {
        if (!cancelled) setMaterialization(result);
      })
      .catch(err => {
        if (!cancelled)
          setMaterializationError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [proposal]);

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
      <div className="mt-3 rounded-md border border-border bg-muted/30 p-2 text-[11px] text-muted-foreground">
        <span className="mono uppercase tracking-wider text-[10px]">// rejected</span> · {proposal.title}
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
    <div className="mt-3 rounded-md border border-primary/40 bg-primary/5 p-3 text-xs text-foreground">
      <div className="flex items-center justify-between gap-2">
        <p className="mono uppercase tracking-[0.18em] text-[10px] text-primary">// proposal</p>
        <div className="flex flex-wrap items-center justify-end gap-1.5">
          <button
            onClick={runAll}
            className="rounded-md border border-border bg-card px-2 py-1 text-[10px] mono uppercase tracking-wider hover:bg-muted hover:border-primary/40 transition-colors"
          >
            Test all
          </button>
          <button
            onClick={() => setDismissed(true)}
            className="rounded-md border border-border bg-card px-2 py-1 text-[10px] mono uppercase tracking-wider hover:bg-muted hover:border-destructive/40 hover:text-destructive transition-colors"
          >
            Reject
          </button>
          <button
            onClick={apply}
            disabled={isApplying || proposal.widgets.length === 0}
            className="rounded-md bg-primary text-primary-foreground border border-primary px-2.5 py-1 text-[10px] mono uppercase tracking-wider font-semibold hover:glow-primary disabled:cursor-not-allowed disabled:opacity-50 transition-all"
          >
            {isApplying ? 'Applying…' : 'Apply'}
          </button>
        </div>
      </div>
      <div className="mt-2">
        <p className="font-semibold tracking-tight break-words">{proposal.title}</p>
        {proposal.dashboard_name && (
          <p className="mt-0.5 text-[11px] text-muted-foreground break-words">Dashboard: <span className="text-foreground">{proposal.dashboard_name}</span></p>
        )}
        {proposal.summary && (
          <p className="mt-1 text-[11px] text-muted-foreground break-words">{proposal.summary}</p>
        )}
      </div>

      {materialization && (
        <MaterializationSummary preview={materialization} />
      )}
      {materializationError && (
        <p className="mt-2 text-[10px] mono text-destructive">
          datasource preview failed: {materializationError}
        </p>
      )}

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
          <div className="rounded-md border border-destructive/40 bg-destructive/5 px-2 py-1.5 text-[11px] mono text-destructive">
            <span className="uppercase tracking-wider text-[10px]">// removes</span> {proposal.remove_widget_ids.length} existing widget{proposal.remove_widget_ids.length === 1 ? '' : 's'} on apply
          </div>
        )}
      </div>
    </div>
  );
}

function MaterializationSummary({ preview }: { preview: ProposalMaterializationPreview }) {
  const total =
    preview.creates.length +
    preview.reuses.length +
    preview.rejects.length +
    preview.passthrough.length;
  if (total === 0) return null;
  return (
    <div className="mt-2 rounded-md border border-border bg-card/60 px-2 py-1.5 text-[11px]">
      <p className="mono uppercase tracking-wider text-[10px] text-muted-foreground">
        // datasources on apply
      </p>
      <ul className="mt-1 space-y-0.5">
        {preview.creates.map((entry, i) => (
          <li key={`c-${i}`} className="flex items-center gap-2">
            <span className="rounded-sm border border-neon-lime/50 bg-neon-lime/10 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-neon-lime">
              create
            </span>
            <span className="truncate">
              <span className="font-medium">{entry.widget_title}</span>{' '}
              <span className="text-muted-foreground">· {entry.label}</span>
            </span>
          </li>
        ))}
        {preview.reuses.map((entry, i) => (
          <li key={`r-${i}`} className="flex items-center gap-2">
            <span className="rounded-sm border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-primary">
              reuse
            </span>
            <span className="truncate">
              <span className="font-medium">{entry.widget_title}</span>{' '}
              <span className="text-muted-foreground">· {entry.label}</span>
            </span>
          </li>
        ))}
        {preview.passthrough.map((entry, i) => (
          <li key={`p-${i}`} className="flex items-center gap-2">
            <span className="rounded-sm border border-border bg-muted/40 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-muted-foreground">
              inline
            </span>
            <span className="truncate text-muted-foreground">
              <span className="font-medium text-foreground">{entry.widget_title}</span>{' '}
              · {entry.label}
            </span>
          </li>
        ))}
        {preview.rejects.map((reject, i) => (
          <li key={`x-${i}`} className="flex items-center gap-2">
            <span className="rounded-sm border border-destructive/50 bg-destructive/10 px-1.5 py-0.5 text-[9px] mono uppercase tracking-wider text-destructive">
              reject
            </span>
            <span className="truncate text-destructive">
              <span className="font-medium">{reject.widget_title}</span>{' '}
              · {reject.reason}
            </span>
          </li>
        ))}
      </ul>
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
    ? 'border-border bg-card'
    : dryRun.status === 'running'
      ? 'border-border bg-muted opacity-70'
      : dryRun.status === 'ok'
        ? 'border-neon-lime/40 bg-neon-lime/10 text-neon-lime'
        : 'border-destructive/40 bg-destructive/10 text-destructive';
  return (
    <div className="rounded-md border border-border bg-card/60 px-2 py-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium truncate">{widget.title}</span>
        <div className="flex items-center gap-1.5">
          <span className="text-[9px] mono font-semibold uppercase tracking-wider rounded-sm border border-border bg-muted/60 px-1.5 py-0.5 text-muted-foreground">{widget.widget_type}</span>
          <button
            onClick={onTest}
            disabled={dryRun?.status === 'running'}
            className={`rounded-md border px-2 py-0.5 text-[10px] mono uppercase tracking-wider hover:opacity-90 disabled:cursor-wait ${testTone}`}
          >
            {testLabel}
          </button>
        </div>
      </div>
      {widget.datasource_plan ? (
        <p className="mt-1 text-[10px] mono text-muted-foreground">
          {widget.datasource_plan.kind}
          {widget.datasource_plan.tool_name ? ` · ${widget.datasource_plan.tool_name}` : ''}
          {widget.datasource_plan.server_id ? ` · ${widget.datasource_plan.server_id}` : ''}
          {widget.datasource_plan.refresh_cron ? ` · ${widget.datasource_plan.refresh_cron}` : ''}
          {pipelineSteps > 0 ? ` · ${pipelineSteps} step${pipelineSteps === 1 ? '' : 's'}` : ''}
          {widget.replace_widget_id ? <span className="text-neon-amber"> · REPLACES</span> : ''}
        </p>
      ) : (
        <p className="mt-1 text-[10px] mono text-destructive">// missing executable datasource plan</p>
      )}
      {dryRun && dryRun.status === 'ok' && (
        <div className="mt-1 rounded border border-neon-lime/30 bg-neon-lime/5 p-1.5">
          <button
            onClick={() => setExpanded(v => !v)}
            className="flex w-full items-center justify-between text-[10px] mono uppercase tracking-wider text-neon-lime"
          >
            <span>ok · {dryRun.duration_ms}ms · {dryRun.pipeline_steps} step{dryRun.pipeline_steps === 1 ? '' : 's'}{dryRun.has_llm_step ? ' · llm' : ''}</span>
            <span>{expanded ? 'hide' : 'show output'}</span>
          </button>
          {expanded && (
            <pre className="mt-1 max-h-40 overflow-auto rounded bg-card/70 border border-border/60 p-1 mono text-[10px]">
              {JSON.stringify(dryRun.widget_runtime, null, 2)}
            </pre>
          )}
        </div>
      )}
      {dryRun && dryRun.status === 'error' && (
        <div className="mt-1 rounded border border-destructive/30 bg-destructive/5 p-1.5">
          <p className="text-[10px] mono text-destructive break-words">{dryRun.error ?? 'Unknown error'}</p>
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
