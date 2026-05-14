interface Props {
  dashboardCount: number;
  widgetCount: number;
  status: string;
  isBusy: boolean;
}

export function StatusBar({ dashboardCount, widgetCount, status, isBusy }: Props) {
  return (
    <footer className="flex items-center justify-between h-7 px-3 bg-muted/50 border-t border-border text-xs text-muted-foreground">
      <div className="flex items-center gap-3">
        <span>Dashboards: {dashboardCount}</span>
        <span>Widgets: {widgetCount}</span>
      </div>
      <div className="flex items-center gap-1.5">
        <span className={`w-1.5 h-1.5 rounded-full ${isBusy ? 'bg-amber-500 animate-pulse' : 'bg-green-500'}`} />
        <span>{status}</span>
      </div>
    </footer>
  );
}
