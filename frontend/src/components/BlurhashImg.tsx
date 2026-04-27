/**
 * Image with a BlurHash placeholder. While `src` is loading (or if it
 * fails), renders the blurhash as a canvas fill underneath. Once the
 * image is decoded, fades it in over the blur.
 *
 * Non-blurhash callers can omit the `blurhash` prop — the component
 * degrades to a plain <img>.
 */

import { decode } from 'blurhash';
import { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';

interface Props {
  src?: string;
  blurhash?: string | null;
  alt?: string;
  className?: string;
  /** Decoded blurhash canvas size (small = fast; browsers upscale). */
  hashResolution?: number;
  loading?: 'eager' | 'lazy';
}

export function BlurhashImg({
  src,
  blurhash,
  alt = '',
  className,
  hashResolution = 32,
  loading = 'lazy',
}: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!blurhash || !canvasRef.current) return;
    try {
      const pixels = decode(blurhash, hashResolution, hashResolution);
      const ctx = canvasRef.current.getContext('2d');
      if (!ctx) return;
      const imageData = ctx.createImageData(hashResolution, hashResolution);
      imageData.data.set(pixels);
      ctx.putImageData(imageData, 0, 0);
    } catch {
      // Malformed blurhash — ignore, plain image still renders.
    }
  }, [blurhash, hashResolution]);

  return (
    <div className={cn('relative overflow-hidden', className)}>
      {blurhash && (
        // biome-ignore lint/a11y/noAriaHiddenOnFocusable: canvas isn't focusable
        <canvas
          ref={canvasRef}
          width={hashResolution}
          height={hashResolution}
          aria-hidden="true"
          className={cn(
            'absolute inset-0 w-full h-full transition-opacity duration-500',
            loaded ? 'opacity-0' : 'opacity-100'
          )}
        />
      )}
      {src && (
        <img
          src={src}
          alt={alt}
          loading={loading}
          onLoad={() => setLoaded(true)}
          className={cn(
            'relative w-full h-full object-cover transition-opacity duration-500',
            loaded ? 'opacity-100' : 'opacity-0'
          )}
        />
      )}
    </div>
  );
}
