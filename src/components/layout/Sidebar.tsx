import { useState } from 'react';
import type { Dashboard } from '../../lib/api';

interface Props {
  dashboards: Dashboard[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onDelete: (id: string) => void;
  onOpenSettings: () => void;
  isCollapsed: boolean;
  onToggleCollapse: () => void;
}

export function Sidebar({ dashboards, activeId, onSelect, onCreate, onDelete, onOpenSettings, isCollapsed, onToggleCollapse }: Props) {
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
          <svg className={`w-4 h-4 text-muted-foreground transition-transform ${isCollapsed ? '' : 'rotate-180'}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
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
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 5a1 1 0 011-1h4a1 1 0 011 1v10a1 1 0 01-1 1H5a1 1 0 01-1-1V5zM14 5a1 1 0 011-1h4a1 1 0 011 1v6a1 1 0 01-1 1h-4a1 1 0 01-1-1V5zM4 17a1 1 0 011-1h4a1 1 0 011 1v2a1 1 0 01-1 1H5a1 1 0 01-1-1v-2zM14 17a1 1 0 011-1h4a1 1 0 011 1v2a1 1 0 01-1 1h-4a1 1 0 01-1-1v-2z" />
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
      <div className="border-t border-border p-2">
        <button onClick={onOpenSettings} className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 rounded-lg transition-colors ${isCollapsed ? 'justify-center' : ''}`}>
          <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
          </svg>
          {!isCollapsed && <span>Settings</span>}
        </button>
      </div>
    </aside>
  );
}
