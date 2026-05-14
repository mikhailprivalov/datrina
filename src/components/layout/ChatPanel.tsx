import { useState, useRef, useEffect } from 'react';
import { chatApi } from '../../lib/api';
import type { ChatMessage, ChatSession, LLMProvider } from '../../lib/api';

interface Props {
  mode: 'build' | 'context';
  dashboardId?: string;
  activeProvider?: LLMProvider;
  onClose: () => void;
  onModeChange: (mode: 'build' | 'context') => void;
}

export function ChatPanel({ mode, dashboardId, activeProvider, onClose, onModeChange }: Props) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [session, setSession] = useState<ChatSession | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

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

  const handleSend = async () => {
    if (!input.trim() || !session || isLoading) return;
    const content = input.trim();
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
      const assistant = await chatApi.sendMessage(session.id, content);
      setMessages(prev => [...prev, assistant]);
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
              {mode === 'build' ? 'Dashboard generation and tool calling are not enabled in the MVP slice.' : 'Requires a configured provider or local_mock provider.'}
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
              {msg.tool_calls && msg.tool_calls.length > 0 && (
                <div className="mt-2 pt-2 border-t border-border/50">
                  <p className="text-xs opacity-70 flex items-center gap-1">
                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
                    </svg>
                    Used {msg.tool_calls.length} tool{msg.tool_calls.length > 1 ? 's' : ''}
                  </p>
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
            value={input}
            onChange={e => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            aria-label={mode === 'build' ? 'Ask for build guidance' : 'Ask about the data'}
            className="flex-1 resize-none rounded-xl border border-border bg-muted/50 px-3 py-2.5 text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 min-h-[40px] max-h-32"
            rows={1}
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || isLoading}
            className="p-2.5 rounded-xl bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-40 disabled:cursor-not-allowed transition-colors flex-shrink-0"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
            </svg>
          </button>
        </div>
        <p className="text-[10px] text-muted-foreground/60 mt-1.5 text-center">Shift+Enter for new line</p>
      </div>
    </aside>
  );
}
