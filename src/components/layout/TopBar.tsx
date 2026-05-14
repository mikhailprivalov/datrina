import type { Dashboard, LLMProvider } from '../../lib/api';

interface Props {
  dashboard?: Dashboard;
  activeProvider?: LLMProvider;
  isChatOpen: boolean;
  onToggleChat: () => void;
  onOpenBuildChat: () => void;
  onOpenSettings: () => void;
}

export function TopBar({ dashboard, activeProvider, isChatOpen, onToggleChat, onOpenBuildChat, onOpenSettings }: Props) {
  return (
    <header className="flex items-center justify-between h-14 px-4 bg-card border-b border-border">
      <div className="flex items-center gap-3 min-w-0">
        {dashboard ? (
          <>
            <h1 className="text-base font-semibold truncate">{dashboard.name}</h1>
            {dashboard.description && (
              <span className="text-xs text-muted-foreground hidden sm:inline">{dashboard.description}</span>
            )}
          </>
        ) : (
          <h1 className="text-base font-semibold text-muted-foreground">No dashboard selected</h1>
        )}
      </div>

      <div className="flex items-center gap-2 flex-shrink-0">
        <button
          onClick={onOpenSettings}
          className={`hidden md:flex items-center gap-2 px-3 py-1.5 rounded-lg border text-xs transition-colors ${
            activeProvider
              ? 'border-border bg-muted/40 text-foreground hover:bg-muted'
              : 'border-destructive/30 bg-destructive/5 text-destructive hover:bg-destructive/10'
          }`}
          title={activeProvider ? `Active LLM provider: ${activeProvider.name}` : 'LLM provider is not configured'}
        >
          <span className={`w-2 h-2 rounded-full ${activeProvider ? 'bg-emerald-500' : 'bg-destructive'}`} />
          <span className="max-w-44 truncate">
            {activeProvider ? `${activeProvider.name} - ${activeProvider.default_model}` : 'Configure LLM'}
          </span>
        </button>

        <button
          onClick={onOpenBuildChat}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-primary/10 text-primary text-sm rounded-lg hover:bg-primary/20 transition-colors"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
          </svg>
          <span className="hidden sm:inline">Build Chat</span>
        </button>

        <button
          onClick={onToggleChat}
          className={`p-2 rounded-lg transition-colors ${isChatOpen ? 'bg-primary text-primary-foreground' : 'hover:bg-muted text-muted-foreground'}`}
        >
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
          </svg>
        </button>
      </div>
    </header>
  );
}
