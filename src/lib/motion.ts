import { useMemo } from 'react'
import type { Transition, Variants } from 'motion/react'
import { useUIStore, type MotionLevel } from '@/stores/uiStore'

export type Cubic = [number, number, number, number]

/**
 * One easing vocabulary for the whole app.
 *
 * `EASE` is a fast-out / slow-in curve: motion leaves immediately when the
 * user acts and settles gently. Everything that responds to input uses it, so
 * hovering a card, opening a page and expanding a panel all feel related.
 */
export const EASE: Cubic = [0.22, 0.61, 0.36, 1]
export const EASE_IN_OUT: Cubic = [0.65, 0, 0.35, 1]
/** For things that should overshoot a hair — pills, toggles, sliding markers. */
export const SPRING: Transition = { type: 'spring', stiffness: 520, damping: 38, mass: 0.9 }
export const SPRING_SOFT: Transition = { type: 'spring', stiffness: 320, damping: 34, mass: 1 }

export const MOTION_SCALE: Record<MotionLevel, number> = {
  full: 1,
  reduced: 0.55,
  off: 0,
}

/**
 * Durations, in seconds, at `full` intensity. Kept deliberately short — a
 * desktop tool should never make you wait for its own chrome.
 */
export const D = {
  micro: 0.12,
  fast: 0.16,
  base: 0.22,
  page: 0.26,
  slow: 0.36,
} as const

export function useMotionScale(): number {
  return MOTION_SCALE[useUIStore((s) => s.motion)]
}

export interface MotionKit {
  scale: number
  /** Build a tween transition scaled by the user's animation setting. */
  t: (duration?: number, delay?: number, ease?: Cubic) => Transition
  spring: Transition
  springSoft: Transition
  /** Page-level enter/exit. `custom` is the navigation direction (1 fwd, -1 back). */
  page: Variants
  /** Container that reveals its children in sequence. */
  stagger: (step?: number, delay?: number) => Variants
  /** Child of `stagger` — a short rise plus fade. */
  riseItem: Variants
  /** Modal / popover surface. */
  pop: Variants
  overlay: Variants
}

export function useMotion(): MotionKit {
  const scale = useMotionScale()

  return useMemo<MotionKit>(() => {
    const t = (duration: number = D.base, delay = 0, ease: Cubic = EASE): Transition =>
      scale === 0 ? { duration: 0, delay: 0 } : { duration: duration * scale, delay: delay * scale, ease }

    const spring: Transition = scale === 0 ? { duration: 0 } : SPRING
    const springSoft: Transition = scale === 0 ? { duration: 0 } : SPRING_SOFT

    const shift = scale === 0 ? 0 : 22

    const page: Variants = {
      enter: (dir: number = 1) => ({ opacity: 0, x: dir * shift, filter: 'blur(1px)' }),
      center: { opacity: 1, x: 0, filter: 'blur(0px)', transition: t(D.page) },
      exit: (dir: number = 1) => ({
        opacity: 0,
        x: dir * -shift * 0.6,
        filter: 'blur(1px)',
        transition: t(D.fast),
      }),
    }

    const stagger = (step = 0.035, delay = 0.02): Variants => ({
      hidden: {},
      show: {
        transition: {
          staggerChildren: scale === 0 ? 0 : step * scale,
          delayChildren: scale === 0 ? 0 : delay * scale,
        },
      },
    })

    const riseItem: Variants = {
      hidden: { opacity: 0, y: scale === 0 ? 0 : 10 },
      show: { opacity: 1, y: 0, transition: t(D.base) },
      out: { opacity: 0, y: scale === 0 ? 0 : -6, transition: t(D.micro) },
    }

    const pop: Variants = {
      hidden: { opacity: 0, y: scale === 0 ? 0 : 10, scale: scale === 0 ? 1 : 0.97 },
      show: { opacity: 1, y: 0, scale: 1, transition: t(D.base) },
      out: { opacity: 0, y: scale === 0 ? 0 : 6, scale: scale === 0 ? 1 : 0.98, transition: t(D.fast) },
    }

    const overlay: Variants = {
      hidden: { opacity: 0 },
      show: { opacity: 1, transition: t(D.fast) },
      out: { opacity: 0, transition: t(D.fast) },
    }

    return { scale, t, spring, springSoft, page, stagger, riseItem, pop, overlay }
  }, [scale])
}

/**
 * The single scrim used behind every full-screen modal (dialog, command
 * palette, guide, discovery). Sharing one class means a modal always dims and
 * blurs the page the same way, instead of each surface inventing its own
 * opacity/blur. Applied to an absolutely-positioned backdrop filling the
 * fixed modal wrapper.
 */
export const MODAL_SCRIM = 'absolute inset-0 bg-canvas/60 backdrop-blur-[2px]'
/** Stacking level for every top-level modal wrapper (they are mutually exclusive). */
export const MODAL_Z = 'z-[80]'
