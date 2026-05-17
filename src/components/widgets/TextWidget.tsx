import type { CSSProperties } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { TextConfig, TextWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: TextConfig;
  data?: TextWidgetRuntimeData;
}

export function TextWidget({ config, data }: Props) {
  const format = config.format ?? 'markdown';
  const align = config.align ?? 'left';
  const style: CSSProperties = {
    fontSize: config.font_size ? `${config.font_size}px` : '13px',
    color: config.color ?? 'inherit',
    textAlign: align,
  };
  const content = data?.content;

  if (!content) {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
        <span className="text-xs text-muted-foreground">Text data unavailable</span>
      </div>
    );
  }

  if (format === 'html') {
    return (
      <div
        className="text-sm leading-relaxed [&_a]:text-primary [&_a]:underline"
        style={style}
        dangerouslySetInnerHTML={{ __html: content }}
      />
    );
  }

  if (format === 'plain') {
    return (
      <p className="text-sm leading-relaxed whitespace-pre-wrap break-words [overflow-wrap:anywhere]" style={style}>
        {content}
      </p>
    );
  }

  return (
    <div className="space-y-1.5 break-words [overflow-wrap:anywhere] text-sm leading-relaxed" style={style}>
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
            <a href={href} target="_blank" rel="noopener noreferrer" className="text-primary underline">
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
              return <code className="rounded-sm bg-primary/10 px-1 py-0.5 text-[11px] mono text-primary" {...rest}>{children}</code>;
            }
            return <code className={`${className ?? ''} mono text-[11px]`} {...rest}>{children}</code>;
          },
          pre: ({ children }) => (
            <pre className="overflow-x-auto rounded-md border border-border bg-muted/40 p-2 text-[11px]">{children}</pre>
          ),
          table: ({ children }) => (
            <div className="overflow-x-auto">
              <table className="min-w-full text-[11px] border-collapse">{children}</table>
            </div>
          ),
          th: ({ children }) => <th className="border border-border/60 px-2 py-1 bg-muted/40 text-left font-semibold uppercase tracking-wider text-[10px] mono">{children}</th>,
          td: ({ children }) => <td className="border border-border/60 px-2 py-1 align-top">{children}</td>,
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
