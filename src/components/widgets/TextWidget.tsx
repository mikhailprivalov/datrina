import type { CSSProperties } from 'react';
import type { TextConfig, TextWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: TextConfig;
  data?: TextWidgetRuntimeData;
}

function renderInlineMarkdown(text: string) {
  return text.split(/(\*\*[^*]+\*\*)/g).map((part, index) => {
    if (part.startsWith('**') && part.endsWith('**')) {
      return <strong key={index}>{part.slice(2, -2)}</strong>;
    }
    return <span key={index}>{part}</span>;
  });
}

export function TextWidget({ config, data }: Props) {
  const { format, font_size, color, align } = config;

  const style: CSSProperties = {
    fontSize: font_size ? `${font_size}px` : '14px',
    color: color ?? 'inherit',
    textAlign: align ?? 'left',
  };
  const content = data?.content;

  if (!content) {
    return (
      <div className="flex h-full min-h-24 items-center justify-center text-center text-xs text-muted-foreground">
        Text data unavailable
      </div>
    );
  }

  if (format === 'markdown') {
    return (
      <div className="prose prose-sm max-w-none dark:prose-invert" style={style}>
        {content.split('\n').map((line, index) => {
          if (line.startsWith('# ')) {
            return <h1 key={index} className="mb-2 text-lg font-bold">{renderInlineMarkdown(line.slice(2))}</h1>;
          }
          if (line.startsWith('- ')) {
            return <li key={index} className="ml-4">{renderInlineMarkdown(line.slice(2))}</li>;
          }
          if (line.startsWith('> ')) {
            return (
              <blockquote key={index} className="border-l-2 border-primary/50 pl-3 italic text-muted-foreground">
                {renderInlineMarkdown(line.slice(2))}
              </blockquote>
            );
          }
          return <p key={index}>{renderInlineMarkdown(line)}</p>;
        })}
      </div>
    );
  }

  if (format === 'html') {
    return <p className="text-sm leading-relaxed whitespace-pre-wrap" style={style}>{content}</p>;
  }

  return <p className="text-sm leading-relaxed whitespace-pre-wrap" style={style}>{content}</p>;
}
