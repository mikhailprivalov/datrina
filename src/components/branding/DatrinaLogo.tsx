const datrinaLogoUrl = new URL('../../assets/datrina-logo.svg', import.meta.url).href;

interface DatrinaLogoProps {
  alt?: string;
  className?: string;
  imageClassName?: string;
}

export function DatrinaLogo({ alt = 'Datrina logo', className = '', imageClassName = '' }: DatrinaLogoProps) {
  return (
    <span className={`block overflow-hidden ${className}`}>
      <img
        src={datrinaLogoUrl}
        alt={alt}
        className={`h-full w-full object-cover ${imageClassName}`}
        draggable={false}
      />
    </span>
  );
}
