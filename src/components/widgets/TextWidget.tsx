import type { CSSProperties } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { TextConfig, TextWidgetRuntimeData, WidgetStreamState } from '../../lib/api';

interface Props {
  config: TextConfig;
  data?: TextWidgetRuntimeData;
  /** W42: live streaming state. While set, partial provider output is
   *  shown in place of (or alongside) the committed `data.content`. */
  streamState?: WidgetStreamState;
}

const markdownComponents = {
  p: ({ children }: { children?: React.ReactNode }) => <p className="whitespace-pre-wrap">{children}</p>,
  h1: ({ children }: { children?: React.ReactNode }) => <h1 className="text-base font-semibold mt-1">{children}</h1>,
  h2: ({ children }: { children?: React.ReactNode }) => <h2 className="text-sm font-semibold mt-1">{children}</h2>,
  h3: ({ children }: { children?: React.ReactNode }) => <h3 className="text-sm font-medium mt-1">{children}</h3>,
  h4: ({ children }: { children?: React.ReactNode }) => <h4 className="text-xs font-medium mt-1 uppercase tracking-wide">{children}</h4>,
  strong: ({ children }: { children?: React.ReactNode }) => <strong className="font-semibold">{children}</strong>,
  em: ({ children }: { children?: React.ReactNode }) => <em className="italic">{children}</em>,
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    <a href={href} target="_blank" rel="noopener noreferrer" className="text-primary underline">
      {children}
    </a>
  ),
  ul: ({ children }: { children?: React.ReactNode }) => <ul className="list-disc pl-5 space-y-0.5">{children}</ul>,
  ol: ({ children }: { children?: React.ReactNode }) => <ol className="list-decimal pl-5 space-y-0.5">{children}</ol>,
  li: ({ children }: { children?: React.ReactNode }) => <li className="leading-snug">{children}</li>,
  blockquote: ({ children }: { children?: React.ReactNode }) => (
    <blockquote className="border-l-2 border-border/70 pl-2 text-muted-foreground italic">{children}</blockquote>
  ),
  hr: () => <hr className="my-2 border-border/60" />,
  code: ({ className, children, ...rest }: { className?: string; children?: React.ReactNode }) => {
    const inline = !className;
    if (inline) {
      return <code className="rounded-sm bg-primary/10 px-1 py-0.5 text-[11px] mono text-primary" {...rest}>{children}</code>;
    }
    return <code className={`${className ?? ''} mono text-[11px]`} {...rest}>{children}</code>;
  },
  pre: ({ children }: { children?: React.ReactNode }) => (
    <pre className="overflow-x-auto rounded-md border border-border bg-muted/40 p-2 text-[11px]">{children}</pre>
  ),
  table: ({ children }: { children?: React.ReactNode }) => (
    <div className="overflow-x-auto">
      <table className="min-w-full text-[11px] border-collapse">{children}</table>
    </div>
  ),
  th: ({ children }: { children?: React.ReactNode }) => <th className="border border-border/60 px-2 py-1 bg-muted/40 text-left font-semibold uppercase tracking-wider text-[10px] mono">{children}</th>,
  td: ({ children }: { children?: React.ReactNode }) => <td className="border border-border/60 px-2 py-1 align-top">{children}</td>,
};

export function TextWidget({ config, data, streamState }: Props) {
  const format = config.format ?? 'markdown';
  const align = config.align ?? 'left';
  const style: CSSProperties = {
    fontSize: config.font_size ? `${config.font_size}px` : '13px',
    color: config.color ?? 'inherit',
    textAlign: align,
  };

  // W42 partial-text precedence:
  //  - `streamState.partialText` (or the failed-partial buffer) wins
  //    over `data.content` while the refresh is active so the user
  //    sees the in-flight provider output.
  //  - Committed `data.content` paints once the refresh terminates.
  const liveText = streamState
    ? streamState.status === 'failed'
      ? streamState.partialOnFail ?? streamState.partialText
      : streamState.partialText
    : undefined;
  const isStreamingActive = !!streamState && streamState.status !== 'failed';
  const content = (liveText && liveText.length > 0) ? liveText : data?.content;

  // Empty-and-thinking: only a reasoning summary so far, no text — show
  // an explicit "reasoning…" placeholder rather than the empty-state.
  const reasoningOnly =
    streamState &&
    streamState.hasReasoning &&
    (!liveText || liveText.length === 0) &&
    !data?.content;

  if (!content && reasoningOnly) {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-2 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-primary">// reasoning</span>
        <span className="text-xs text-muted-foreground italic">LLM is thinking…</span>
        {streamState?.reasoningText && (
          <p className="max-w-prose text-[11px] text-muted-foreground/70 line-clamp-3 px-3">
            {streamState.reasoningText}
          </p>
        )}
      </div>
    );
  }

  if (!content && streamState && streamState.status !== 'failed') {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-primary">// streaming</span>
        <span className="text-xs text-muted-foreground italic">
          {streamState.statusHint ?? 'Provider response in flight…'}
        </span>
      </div>
    );
  }

  if (!content) {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">// no data</span>
        <span className="text-xs text-muted-foreground">
          {streamState?.error ? streamState.error : 'Text data unavailable'}
        </span>
      </div>
    );
  }

  const partialClass = isStreamingActive
    ? 'opacity-80 [&_p::after]:content-["▍"] [&_p::after]:ml-0.5 [&_p::after]:animate-pulse [&_p::after]:text-primary'
    : streamState?.status === 'failed' && liveText
      ? 'opacity-80 ring-1 ring-destructive/40 rounded p-1.5 bg-destructive/5'
      : '';

  if (format === 'html') {
    return (
      <div
        className={`text-sm leading-relaxed [&_a]:text-primary [&_a]:underline ${partialClass}`}
        style={style}
        dangerouslySetInnerHTML={{ __html: content }}
      />
    );
  }

  if (format === 'plain') {
    return (
      <p
        className={`text-sm leading-relaxed whitespace-pre-wrap break-words [overflow-wrap:anywhere] ${partialClass}`}
        style={style}
      >
        {content}
        {isStreamingActive && <span className="ml-0.5 text-primary animate-pulse">▍</span>}
      </p>
    );
  }

  return (
    <div className={`space-y-1.5 break-words [overflow-wrap:anywhere] text-sm leading-relaxed ${partialClass}`} style={style}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
