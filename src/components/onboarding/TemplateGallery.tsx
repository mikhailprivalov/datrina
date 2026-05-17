import { useEffect, useState } from 'react';
import { mcpApi } from '../../lib/api';
import type { MCPServer } from '../../lib/api';
import {
  DASHBOARD_TEMPLATES,
  missingMcpDependencies,
} from '../../lib/templates';
import type { DashboardTemplate } from '../../lib/templates';

interface Props {
  onSelect: (template: DashboardTemplate) => void;
  onOpenPlayground: () => void;
  onOpenMcpSettings?: () => void;
  variant?: 'page' | 'modal';
  onClose?: () => void;
}

export function TemplateGallery({
  onSelect,
  onOpenPlayground,
  onOpenMcpSettings,
  variant = 'page',
  onClose,
}: Props) {
  const [servers, setServers] = useState<MCPServer[]>([]);

  useEffect(() => {
    let cancelled = false;
    mcpApi
      .listServers()
      .then(list => {
        if (!cancelled) setServers(list);
      })
      .catch(() => {
        /* gallery still renders without dependency check */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const serverNames = servers.map(s => s.name);

  const grid = (
    <div className="grid w-full grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
      {DASHBOARD_TEMPLATES.map(template => (
        <TemplateCard
          key={template.id}
          template={template}
          missing={missingMcpDependencies(template, serverNames)}
          onLaunch={() => {
            if (template.launch === 'playground') {
              onOpenPlayground();
              onClose?.();
              return;
            }
            onSelect(template);
            onClose?.();
          }}
          onOpenMcpSettings={onOpenMcpSettings}
        />
      ))}
    </div>
  );

  if (variant === 'modal') {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 backdrop-blur-sm">
        <div className="flex max-h-[85vh] w-[min(96vw,68rem)] flex-col rounded-md border border-border bg-card shadow-2xl">
          <div className="flex items-center justify-between border-b border-border px-5 py-3 bg-muted/20">
            <div>
              <p className="text-[10px] mono uppercase tracking-[0.18em] text-primary">// templates</p>
              <h2 className="mt-0.5 text-sm font-semibold tracking-tight">New dashboard from template</h2>
            </div>
            <button
              onClick={onClose}
              className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
              aria-label="Close"
            >
              <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
          <div className="overflow-auto p-5">{grid}</div>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col items-center gap-6 py-8">
      <div className="space-y-3 text-center">
        <p className="mono text-[10px] uppercase tracking-[0.2em] text-primary">// welcome</p>
        <h1 className="text-2xl font-semibold text-foreground tracking-tight">Datrina <span className="text-primary">·</span> local AI console</h1>
        <p className="mx-auto max-w-xl text-sm text-muted-foreground">
          Pick a starting point. Each template seeds the Build Chat with a tailored prompt, so the agent already knows what to build.
        </p>
      </div>
      {grid}
    </div>
  );
}

function TemplateCard({
  template,
  missing,
  onLaunch,
  onOpenMcpSettings,
}: {
  template: DashboardTemplate;
  missing: string[];
  onLaunch: () => void;
  onOpenMcpSettings?: () => void;
}) {
  const hasMissing = missing.length > 0;
  return (
    <div className="group flex flex-col gap-3 rounded-md border border-border bg-card p-4 shadow-sm transition-all hover:border-primary/40 hover:shadow-[0_0_20px_-8px_hsl(var(--primary)/0.35)]">
      <div className="flex items-start gap-3">
        <div className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary border border-primary/20 group-hover:border-primary/50 transition-colors">
          <svg className="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d={template.icon_path} />
          </svg>
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold text-foreground tracking-tight">{template.title}</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{template.description}</p>
        </div>
      </div>

      {template.example_widgets.length > 0 && (
        <ul className="flex flex-wrap gap-1">
          {template.example_widgets.map(widget => (
            <li
              key={widget}
              className="rounded-sm border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] mono uppercase tracking-wider text-muted-foreground"
            >
              {widget}
            </li>
          ))}
        </ul>
      )}

      {hasMissing && (
        <div className="rounded-md border border-neon-amber/40 bg-neon-amber/10 px-2 py-1.5 text-[11px] text-neon-amber">
          <span className="mono uppercase tracking-wider text-[10px]">// needs MCP:</span> {missing.join(', ')}.{' '}
          {onOpenMcpSettings && (
            <button
              onClick={e => {
                e.stopPropagation();
                onOpenMcpSettings();
              }}
              className="underline hover:no-underline"
            >
              Add server
            </button>
          )}
        </div>
      )}

      <button
        onClick={onLaunch}
        className="mt-auto rounded-md bg-primary border border-primary px-3 py-1.5 text-xs mono uppercase tracking-wider font-semibold text-primary-foreground hover:glow-primary transition-all"
      >
        {template.launch === 'playground' ? 'Open Playground' : 'Start building'}
      </button>
    </div>
  );
}
