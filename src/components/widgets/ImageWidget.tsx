import type { ImageConfig, ImageWidgetRuntimeData } from '../../lib/api';

interface Props {
  config: ImageConfig;
  data?: ImageWidgetRuntimeData;
}

export function ImageWidget({ config, data }: Props) {
  const { fit, border_radius = 4 } = config;

  return (
    <div className="w-full h-full flex items-center justify-center">
      {data?.src ? (
        <img
          src={data.src}
          alt={data.alt ?? ''}
          className="h-full w-full"
          style={{ borderRadius: `${border_radius}px`, objectFit: fit }}
        />
      ) : (
        <div
          className="w-full h-full bg-muted/30 flex items-center justify-center"
          style={{ borderRadius: `${border_radius}px` }}
        >
          <div className="text-center text-muted-foreground">
            <svg className="w-12 h-12 mx-auto mb-2 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1} d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
            </svg>
            <p className="text-sm">Image data unavailable</p>
          </div>
        </div>
      )}
    </div>
  );
}
