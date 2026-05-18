import { useState } from 'react';
import type { ImageConfig, ImageWidgetRuntimeData } from '../../lib/api';
import { ImageLightbox } from './ImageLightbox';

interface Props {
  config: ImageConfig;
  data?: ImageWidgetRuntimeData;
}

export function ImageWidget({ config, data }: Props) {
  const { fit, border_radius = 4 } = config;
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [broken, setBroken] = useState(false);

  if (!data?.src) {
    return (
      <div className="w-full h-full flex items-center justify-center">
        <div
          className="w-full h-full bg-muted/30 border border-dashed border-border flex items-center justify-center"
          style={{ borderRadius: `${border_radius}px` }}
        >
          <div className="text-center">
            <svg className="w-12 h-12 mx-auto mb-2 opacity-40 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1} d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
            </svg>
            <p className="text-[10px] mono uppercase tracking-wider text-muted-foreground">// no image</p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="w-full h-full flex items-center justify-center">
      {broken ? (
        <div
          className="w-full h-full bg-muted/30 border border-dashed border-destructive/40 flex items-center justify-center"
          style={{ borderRadius: `${border_radius}px` }}
        >
          <div className="text-center">
            <p className="text-[10px] mono uppercase tracking-wider text-destructive/80">// broken image</p>
            <p className="mt-1 max-w-xs truncate px-2 text-[10px] mono text-muted-foreground">{data.src}</p>
          </div>
        </div>
      ) : (
        <button
          type="button"
          className="h-full w-full overflow-hidden p-0 focus:outline-none focus:ring-2 focus:ring-primary"
          style={{ borderRadius: `${border_radius}px` }}
          onClick={() => setIsFullscreen(true)}
          aria-label={data.alt ?? 'Open image fullscreen'}
        >
          <img
            src={data.src}
            alt={data.alt ?? ''}
            className="h-full w-full"
            style={{ borderRadius: `${border_radius}px`, objectFit: fit }}
            onError={() => setBroken(true)}
          />
        </button>
      )}
      {isFullscreen && !broken && (
        <ImageLightbox
          items={[{ src: data.src, alt: data.alt, title: data.alt }]}
          startIndex={0}
          onClose={() => setIsFullscreen(false)}
        />
      )}
    </div>
  );
}
