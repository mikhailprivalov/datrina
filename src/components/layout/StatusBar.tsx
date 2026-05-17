interface Props {
  dashboardCount: number;
  widgetCount: number;
  status: string;
  isBusy: boolean;
}

export function StatusBar({ dashboardCount, widgetCount, status, isBusy }: Props) {
  return (
    <footer className="flex items-center justify-between h-7 px-3 bg-muted/40 border-t border-border text-[11px] text-muted-foreground">
      <div className="flex items-center gap-4 mono uppercase tracking-wider">
        <span><span className="opacity-60">DASH</span> <span className="tabular text-foreground">{dashboardCount}</span></span>
        <span className="text-border">|</span>
        <span><span className="opacity-60">WIDGETS</span> <span className="tabular text-foreground">{widgetCount}</span></span>
      </div>
      <div className="flex items-center gap-2 mono uppercase tracking-wider">
        <span className={`relative inline-block w-1.5 h-1.5 rounded-full ${isBusy ? 'bg-neon-amber animate-pulse' : 'bg-neon-lime'}`} aria-hidden />
        <span className="truncate max-w-md text-foreground">{status}</span>
      </div>
    </footer>
  );
}
