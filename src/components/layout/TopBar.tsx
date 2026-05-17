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
    <header className="flex items-center justify-between h-14 px-4 bg-card/95 backdrop-blur-sm border-b border-border">
      <div className="flex items-center gap-3 min-w-0">
        {dashboard ? (
          <div className="flex items-center gap-3 min-w-0">
            <span className="hidden sm:inline-flex h-5 items-center rounded-sm bg-primary/15 px-1.5 text-[10px] mono font-semibold uppercase tracking-wider text-primary">
              dash
            </span>
            <h1 className="text-base font-semibold truncate tracking-tight">{dashboard.name}</h1>
            {dashboard.description && (
              <span className="text-xs text-muted-foreground hidden md:inline truncate max-w-md">
                {dashboard.description}
              </span>
            )}
          </div>
        ) : (
          <div className="flex items-center gap-2">
            <span className="inline-flex h-5 items-center rounded-sm bg-muted px-1.5 text-[10px] mono font-semibold uppercase tracking-wider text-muted-foreground">
              idle
            </span>
            <h1 className="text-base font-semibold text-muted-foreground">No dashboard selected</h1>
          </div>
        )}
      </div>

      <div className="flex items-center gap-2 flex-shrink-0">
        <button
          onClick={onOpenSettings}
          className={`hidden md:flex items-center gap-2 px-3 py-1.5 rounded-md border text-xs transition-colors ${
            activeProvider
              ? 'border-border bg-muted/40 text-foreground hover:bg-muted'
              : 'border-destructive/40 bg-destructive/10 text-destructive hover:bg-destructive/15'
          }`}
          title={activeProvider ? `Active LLM provider: ${activeProvider.name}` : 'LLM provider is not configured'}
        >
          <span className={`relative w-2 h-2 rounded-full ${activeProvider ? 'bg-neon-lime glow-primary' : 'bg-destructive glow-destructive'}`} />
          <span className="max-w-44 truncate mono uppercase tracking-wider">
            {activeProvider ? `${activeProvider.name} · ${activeProvider.default_model}` : 'No provider'}
          </span>
        </button>

        <button
          onClick={onOpenBuildChat}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-primary/15 text-primary text-sm rounded-md hover:bg-primary/25 hover:glow-primary transition-all border border-primary/30"
          title="Open Build chat"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
          </svg>
          <span className="hidden sm:inline mono uppercase tracking-wider text-xs font-semibold">Build</span>
        </button>

        <button
          onClick={onToggleChat}
          title="Toggle chat"
          className={`p-2 rounded-md transition-colors border ${isChatOpen
            ? 'bg-accent/20 text-accent border-accent/40'
            : 'border-transparent text-muted-foreground hover:bg-muted hover:text-foreground'}`}
        >
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
          </svg>
        </button>
      </div>
    </header>
  );
}
