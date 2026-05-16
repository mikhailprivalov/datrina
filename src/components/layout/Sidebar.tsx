import { useState } from 'react';
import type { Dashboard } from '../../lib/api';

const DASHBOARD_ICONS = [
  'M4 5a1 1 0 011-1h4a1 1 0 011 1v10a1 1 0 01-1 1H5a1 1 0 01-1-1V5zM14 5a1 1 0 011-1h4a1 1 0 011 1v6a1 1 0 01-1 1h-4a1 1 0 01-1-1V5zM4 17a1 1 0 011-1h4a1 1 0 011 1v2a1 1 0 01-1 1H5a1 1 0 01-1-1v-2zM14 17a1 1 0 011-1h4a1 1 0 011 1v2a1 1 0 01-1 1h-4a1 1 0 01-1-1v-2z',
  'M3 3h7v7H3V3zm11 0h7v7h-7V3zM3 14h7v7H3v-7zm11 0h7v7h-7v-7z',
  'M3 12l9-9 9 9M5 10v10h14V10',
  'M12 2l3 6 6 .9-4.5 4.4 1 6.2L12 16.5 6.5 19.5l1-6.2L3 8.9 9 8z',
  'M3 13h2v8H3v-8zm4-5h2v13H7V8zm4-3h2v16h-2V5zm4 7h2v9h-2v-9zm4-4h2v13h-2V8z',
  'M3 12c0-4.97 4.03-9 9-9s9 4.03 9 9-4.03 9-9 9-9-4.03-9-9zm9-5v5l3 2',
  'M4 6h16M4 12h10M4 18h16',
  'M12 2L2 22h20L12 2zm0 4l7 14H5l7-14zm-1 5v3h2v-3h-2zm0 4v2h2v-2h-2z',
  'M21 7H3a1 1 0 00-1 1v8a1 1 0 001 1h18a1 1 0 001-1V8a1 1 0 00-1-1zm-9 3a3 3 0 100 6 3 3 0 000-6z',
  'M12 3l9 4-9 4-9-4 9-4zm-9 9l9 4 9-4M3 16l9 4 9-4',
];

function pickIconPath(id: string): string {
  let hash = 0;
  for (let i = 0; i < id.length; i++) {
    hash = (hash * 31 + id.charCodeAt(i)) | 0;
  }
  const index = Math.abs(hash) % DASHBOARD_ICONS.length;
  return DASHBOARD_ICONS[index];
}

interface Props {
  dashboards: Dashboard[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onCreateFromTemplate?: () => void;
  onDelete: (id: string) => void;
  onOpenSettings: () => void;
  onOpenMcpSettings?: () => void;
  onOpenMemorySettings?: () => void;
  /** W22: open the provider-cost dashboard. */
  onOpenCostsView?: () => void;
  onOpenPlayground?: () => void;
  isPlaygroundActive?: boolean;
  onOpenAlerts?: () => void;
  isAlertsActive?: boolean;
  unacknowledgedAlertCount?: number;
  isCollapsed: boolean;
  onToggleCollapse: () => void;
  theme: 'light' | 'dark';
  onToggleTheme: () => void;
}

export function Sidebar({ dashboards, activeId, onSelect, onCreate, onCreateFromTemplate, onDelete, onOpenSettings, onOpenMcpSettings, onOpenMemorySettings, onOpenCostsView, onOpenPlayground, isPlaygroundActive, onOpenAlerts, isAlertsActive, unacknowledgedAlertCount = 0, isCollapsed, onToggleCollapse, theme, onToggleTheme }: Props) {
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; id: string } | null>(null);

  const handleContextMenu = (e: React.MouseEvent, id: string) => {
    e.preventDefault();
    setCtxMenu({ x: e.clientX, y: e.clientY, id });
  };

  return (
    <aside className={`flex flex-col bg-card border-r border-border transition-all duration-200 ${isCollapsed ? 'w-14' : 'w-64'}`}>
      {/* Header */}
      <div className="flex items-center justify-between h-14 px-3 border-b border-border">
        {!isCollapsed && (
          <div className="flex items-center gap-2 min-w-0">
            <div className="w-7 h-7 rounded-lg bg-primary flex items-center justify-center flex-shrink-0">
              <svg className="w-4 h-4 text-primary-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
              </svg>
            </div>
            <span className="font-semibold text-sm truncate">Datrina</span>
          </div>
        )}
        <button onClick={onToggleCollapse} className="p-1.5 rounded-md hover:bg-muted transition-colors flex-shrink-0">
          <svg className={`w-4 h-4 text-muted-foreground transition-transform ${isCollapsed ? 'rotate-180' : ''}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 17l-5-5m0 0l5-5m-5 5h12" />
          </svg>
        </button>
      </div>

      {/* Dashboard list */}
      <div className="flex-1 overflow-y-auto py-2 scrollbar-thin">
        {!isCollapsed && (
          <div className="px-3 mb-2">
            <span className="text-xs font-medium text-muted-foreground uppercase tracking-wider">Dashboards</span>
          </div>
        )}
        {dashboards.map(d => (
          <button
            key={d.id}
            onClick={() => onSelect(d.id)}
            onContextMenu={(e) => handleContextMenu(e, d.id)}
            className={`w-full flex items-center gap-2 px-3 py-2 text-sm transition-colors ${
              activeId === d.id
                ? 'bg-primary/10 text-primary border-r-2 border-primary'
                : 'text-foreground/80 hover:bg-muted/50'
            } ${isCollapsed ? 'justify-center' : ''}`}
            title={d.name}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d={pickIconPath(d.id)} />
            </svg>
            {!isCollapsed && <span className="truncate text-left">{d.name}</span>}
          </button>
        ))}

        <button onClick={onCreate} className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors mt-1 ${isCollapsed ? 'justify-center' : ''}`}>
          <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
          </svg>
          {!isCollapsed && <span>New Dashboard</span>}
        </button>
        {onCreateFromTemplate && (
          <button
            onClick={onCreateFromTemplate}
            title="New from template"
            className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors ${isCollapsed ? 'justify-center' : ''}`}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 5a1 1 0 011-1h4a1 1 0 011 1v4a1 1 0 01-1 1H5a1 1 0 01-1-1V5zm10 0a1 1 0 011-1h4a1 1 0 011 1v4a1 1 0 01-1 1h-4a1 1 0 01-1-1V5zM4 15a1 1 0 011-1h4a1 1 0 011 1v4a1 1 0 01-1 1H5a1 1 0 01-1-1v-4zm10 1.5h6m-3-3v6" />
            </svg>
            {!isCollapsed && <span>From template…</span>}
          </button>
        )}
        {onOpenPlayground && (
          <button
            onClick={onOpenPlayground}
            title="Open Data Playground"
            className={`w-full flex items-center gap-2 px-3 py-2 text-sm transition-colors ${
              isPlaygroundActive
                ? 'bg-primary/10 text-primary border-r-2 border-primary'
                : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
            } ${isCollapsed ? 'justify-center' : ''}`}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
            </svg>
            {!isCollapsed && <span>Explore (Playground)</span>}
          </button>
        )}
        {onOpenAlerts && (
          <button
            onClick={onOpenAlerts}
            title="Alerts"
            className={`relative w-full flex items-center gap-2 px-3 py-2 text-sm transition-colors ${
              isAlertsActive
                ? 'bg-primary/10 text-primary border-r-2 border-primary'
                : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
            } ${isCollapsed ? 'justify-center' : ''}`}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9" />
            </svg>
            {!isCollapsed && (
              <span className="flex-1 text-left">Alerts</span>
            )}
            {unacknowledgedAlertCount > 0 && !isCollapsed && (
              <span className="min-w-[1.25rem] rounded-full bg-destructive px-1.5 text-center text-[10px] font-medium text-destructive-foreground">
                {unacknowledgedAlertCount > 99 ? '99+' : unacknowledgedAlertCount}
              </span>
            )}
            {unacknowledgedAlertCount > 0 && isCollapsed && (
              <span className="absolute right-1 top-1 h-2 w-2 rounded-full bg-destructive" aria-hidden />
            )}
          </button>
        )}
      </div>

      {/* Context menu */}
      {ctxMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setCtxMenu(null)} />
          <div className="fixed z-50 bg-card border border-border rounded-lg shadow-lg py-1 min-w-[140px]" style={{ left: ctxMenu.x, top: ctxMenu.y }}>
            <button
              onClick={() => { onDelete(ctxMenu.id); setCtxMenu(null); }}
              className="w-full text-left px-3 py-1.5 text-sm text-destructive hover:bg-muted transition-colors flex items-center gap-2"
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
              </svg>
              Delete
            </button>
          </div>
        </>
      )}

      {/* Footer */}
      <div className="border-t border-border p-2 space-y-1">
        <button
          onClick={onToggleTheme}
          title={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
          className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}
        >
          {theme === 'dark' ? (
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
            </svg>
          ) : (
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
            </svg>
          )}
          {!isCollapsed && <span>{theme === 'dark' ? 'Light theme' : 'Dark theme'}</span>}
        </button>
        {onOpenMcpSettings && (
          <button onClick={onOpenMcpSettings} className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}>
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M13 10V3L4 14h7v7l9-11h-7z" />
            </svg>
            {!isCollapsed && <span>MCP servers</span>}
          </button>
        )}
        {onOpenMemorySettings && (
          <button
            onClick={onOpenMemorySettings}
            title="Agent memory"
            className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4" />
            </svg>
            {!isCollapsed && <span>Agent memory</span>}
          </button>
        )}
        {onOpenCostsView && (
          <button
            onClick={onOpenCostsView}
            title="Provider costs"
            className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}
          >
            <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            {!isCollapsed && <span>Costs</span>}
          </button>
        )}
        <button onClick={onOpenSettings} className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}>
          <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
          </svg>
          {!isCollapsed && <span>Providers</span>}
        </button>
      </div>
    </aside>
  );
}
