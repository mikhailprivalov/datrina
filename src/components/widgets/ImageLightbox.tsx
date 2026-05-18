import { useCallback, useEffect, useRef, useState } from 'react';
import type { GalleryItem } from '../../lib/api';

interface Props {
  items: GalleryItem[];
  startIndex: number;
  onClose: () => void;
  showSource?: boolean;
}

type FitMode = 'contain' | 'cover' | 'fill';

const FIT_CYCLE: FitMode[] = ['contain', 'cover', 'fill'];

export function ImageLightbox({ items, startIndex, onClose, showSource = true }: Props) {
  const [index, setIndex] = useState(() =>
    Math.max(0, Math.min(startIndex, items.length - 1)),
  );
  const [fit, setFit] = useState<FitMode>('contain');
  const [broken, setBroken] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);

  const goPrev = useCallback(() => {
    setIndex((current) => (items.length === 0 ? 0 : (current - 1 + items.length) % items.length));
    setBroken(false);
  }, [items.length]);

  const goNext = useCallback(() => {
    setIndex((current) => (items.length === 0 ? 0 : (current + 1) % items.length));
    setBroken(false);
  }, [items.length]);

  const cycleFit = useCallback(() => {
    setFit((current) => FIT_CYCLE[(FIT_CYCLE.indexOf(current) + 1) % FIT_CYCLE.length]);
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === 'ArrowRight' || event.key === 'PageDown') {
        event.preventDefault();
        goNext();
        return;
      }
      if (event.key === 'ArrowLeft' || event.key === 'PageUp') {
        event.preventDefault();
        goPrev();
        return;
      }
      if (event.key.toLowerCase() === 'f') {
        event.preventDefault();
        cycleFit();
      }
    }
    document.addEventListener('keydown', onKey);
    const previousActive = document.activeElement as HTMLElement | null;
    wrapperRef.current?.focus();
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = previousOverflow;
      previousActive?.focus?.();
    };
  }, [onClose, goNext, goPrev, cycleFit]);

  if (items.length === 0) return null;
  const active = items[index];
  const total = items.length;
  const fitClass =
    fit === 'cover' ? 'object-cover' : fit === 'fill' ? 'object-fill' : 'object-contain';

  return (
    <div
      ref={wrapperRef}
      role="dialog"
      aria-modal="true"
      aria-label={active.title ?? active.alt ?? 'Image viewer'}
      tabIndex={-1}
      className="fixed inset-0 z-[1000] flex flex-col items-stretch bg-black/90 outline-none"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="flex items-center justify-between px-4 py-2 text-xs text-white/70 mono">
        <span className="uppercase tracking-wider">
          // {index + 1} / {total}
        </span>
        <div className="flex items-center gap-3">
          <button
            type="button"
            className="rounded border border-white/20 px-2 py-0.5 uppercase tracking-wider hover:bg-white/10"
            onClick={cycleFit}
            title="Cycle fit (f)"
          >
            fit: {fit}
          </button>
          <button
            type="button"
            className="rounded border border-white/20 px-2 py-0.5 uppercase tracking-wider hover:bg-white/10"
            onClick={onClose}
            title="Close (Esc)"
          >
            close
          </button>
        </div>
      </div>

      <div className="relative flex-1 flex items-center justify-center px-12">
        {total > 1 && (
          <button
            type="button"
            className="absolute left-2 top-1/2 -translate-y-1/2 rounded-full border border-white/20 bg-black/40 px-3 py-2 text-white hover:bg-white/10"
            onClick={goPrev}
            aria-label="Previous image"
          >
            ‹
          </button>
        )}
        {broken ? (
          <div className="flex flex-col items-center gap-2 text-center text-white/70">
            <span className="text-[10px] mono uppercase tracking-wider">// broken image</span>
            <span className="text-xs">Source failed to load:</span>
            <code className="max-w-md break-all rounded bg-white/10 px-2 py-1 text-[11px]">
              {active.src}
            </code>
          </div>
        ) : (
          <img
            key={active.src}
            src={active.src}
            alt={active.alt ?? active.title ?? ''}
            className={`max-h-full max-w-full ${fitClass}`}
            onError={() => setBroken(true)}
          />
        )}
        {total > 1 && (
          <button
            type="button"
            className="absolute right-2 top-1/2 -translate-y-1/2 rounded-full border border-white/20 bg-black/40 px-3 py-2 text-white hover:bg-white/10"
            onClick={goNext}
            aria-label="Next image"
          >
            ›
          </button>
        )}
      </div>

      {(active.title || active.caption || (showSource && active.source) || active.link) && (
        <div className="border-t border-white/10 px-4 py-2 text-white/80">
          {active.title && <div className="text-sm font-semibold">{active.title}</div>}
          {active.caption && (
            <div className="mt-0.5 text-xs text-white/70 line-clamp-3">{active.caption}</div>
          )}
          <div className="mt-1 flex items-center justify-between text-[10px] mono uppercase tracking-wider text-white/50">
            {showSource && active.source ? <span>// {active.source}</span> : <span />}
            {active.link && (
              <a
                href={active.link}
                target="_blank"
                rel="noopener noreferrer"
                className="text-white/70 underline hover:text-white"
              >
                open source ↗
              </a>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
