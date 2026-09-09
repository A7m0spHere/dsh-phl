import markUrl from '@/assets/phl-mark.png'
import { cn } from '@/lib/cn'

/**
 * PHL's mark: a whale tail — the instance surfacing out of its own isolated
 * environment. The artwork is a flat single-colour stencil (see
 * `src-tauri/icons/master/PROVENANCE.md`), so it is painted as a CSS mask over
 * the accent colour: the shape stays crisp at any size and still follows the
 * user's theme. Regenerate the mask with `npm run icon -- --mark`.
 */
export function Logo({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <span
      role="img"
      aria-label="PHL"
      className={cn('inline-block shrink-0 bg-accent', className)}
      style={{
        width: size,
        height: size,
        maskImage: `url(${markUrl})`,
        WebkitMaskImage: `url(${markUrl})`,
        maskSize: 'contain',
        WebkitMaskSize: 'contain',
        maskRepeat: 'no-repeat',
        WebkitMaskRepeat: 'no-repeat',
        maskPosition: 'center',
        WebkitMaskPosition: 'center',
      }}
    />
  )
}
