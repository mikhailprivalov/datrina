import { useMemo, useState } from 'react';
import type { GalleryConfig, GalleryItem, GalleryWidgetRuntimeData } from '../../lib/api';
import { ImageLightbox } from './ImageLightbox';

interface Props {
  config: GalleryConfig;
  data?: GalleryWidgetRuntimeData;
}

const ASPECT_PADDING: Record<NonNullable<GalleryConfig['thumbnail_aspect']>, string> = {
  square: 'aspect-square',
  landscape: 'aspect-[16/10]',
  portrait: 'aspect-[3/4]',
  original: '',
};

const LAYOUT_CLASS: Record<NonNullable<GalleryConfig['layout']>, string> = {
  grid: 'grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-2',
  row: 'flex gap-2 overflow-x-auto pb-1',
  masonry: 'columns-2 md:columns-3 lg:columns-4 gap-2 space-y-2',
};

export function GalleryWidget({ config, data }: Props) {
  const layout = config.layout ?? 'grid';
  const aspect = config.thumbnail_aspect ?? 'landscape';
  const showCaption = config.show_caption !== false;
  const showSource = config.show_source === true;
  const fullscreenEnabled = config.fullscreen_enabled !== false;
  const fit = config.fit ?? 'cover';
  const borderRadius = config.border_radius ?? 4;
  const maxVisible = config.max_visible_items && config.max_visible_items > 0
    ? config.max_visible_items
    : data?.items.length ?? 0;

  const items: GalleryItem[] = useMemo(() => {
    if (!data?.items) return [];
    return data.items.slice(0, maxVisible);
  }, [data, maxVisible]);

  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const [brokenIds, setBrokenIds] = useState<Record<number, boolean>>({});

  if (items.length === 0) {
    return (
      <div className="flex h-full min-h-24 flex-col items-center justify-center gap-1 text-center">
        <span className="text-[10px] mono uppercase tracking-wider text-muted-foreground/60">
          // no images
        </span>
        <span className="text-xs text-muted-foreground">Gallery has no items yet.</span>
      </div>
    );
  }

  const fitClass = fit === 'cover' ? 'object-cover' : fit === 'contain' ? 'object-contain' : 'object-fill';
  const containerClass = LAYOUT_CLASS[layout];
  const aspectClass = ASPECT_PADDING[aspect];

  return (
    <div className="relative h-full w-full overflow-auto">
      <div className={containerClass}>
        {items.map((item, index) => {
          const isBroken = brokenIds[index] === true;
          const onActivate = () => {
            if (!fullscreenEnabled) return;
            setLightboxIndex(index);
          };
          return (
            <button
              type="button"
              key={`${item.id ?? item.src}-${index}`}
              className={`group relative block w-full overflow-hidden border border-border/60 bg-muted/30 text-left transition hover:border-primary/60 focus:outline-none focus:ring-2 focus:ring-primary ${
                layout === 'row' ? 'flex-shrink-0 w-40' : 'mb-2 break-inside-avoid'
              }`}
              style={{ borderRadius }}
              onClick={onActivate}
              aria-label={item.title ?? item.alt ?? `Image ${index + 1}`}
            >
              <div className={`relative w-full ${aspectClass}`}>
                {isBroken ? (
                  <div className="flex h-full min-h-16 w-full items-center justify-center text-[10px] mono uppercase tracking-wider text-muted-foreground/70">
                    // broken
                  </div>
                ) : (
                  <img
                    src={item.src}
                    alt={item.alt ?? item.title ?? ''}
                    loading="lazy"
                    className={`h-full w-full ${fitClass}`}
                    onError={() => setBrokenIds((prev) => ({ ...prev, [index]: true }))}
                  />
                )}
              </div>
              {(showCaption && (item.title || item.caption)) || (showSource && item.source) ? (
                <div className="px-2 py-1">
                  {showCaption && item.title && (
                    <div className="truncate text-[11px] font-medium">{item.title}</div>
                  )}
                  {showCaption && item.caption && (
                    <div className="truncate text-[10px] text-muted-foreground">{item.caption}</div>
                  )}
                  {showSource && item.source && (
                    <div className="truncate text-[9px] mono uppercase tracking-wider text-muted-foreground/70">
                      {item.source}
                    </div>
                  )}
                </div>
              ) : null}
            </button>
          );
        })}
      </div>
      {fullscreenEnabled && lightboxIndex !== null && (
        <ImageLightbox
          items={items}
          startIndex={lightboxIndex}
          showSource={showSource}
          onClose={() => setLightboxIndex(null)}
        />
      )}
    </div>
  );
}
