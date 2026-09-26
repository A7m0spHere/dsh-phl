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
  /** Press feedback — must land inside the click, not after it. */
  press: 0.09,
  micro: 0.12,
  fast: 0.16,
  base: 0.22,
  page: 0.26,
  slow: 0.36,
  /** The travelling sheen sweep — long, ambient, only on hero/primary chrome. */
  sheen: 0.65,
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
  /**
   * Motion-safe value: returns `v` while motion is on, `off` when the user
   * turned it off. Collapses the `scale === 0 ? off : v` ternary that used to
   * be retyped at every call site.
   */
  v: <T>(v: T, off: T) => T
  /** Motion-safe translate distance in px: 0 when motion is off. */
  shift: (px: number) => number
  /** Page-level enter/exit. `custom` is the navigation direction (1 fwd, -1 back, 0 lateral cross-fade). */
  page: Variants
  /** Container that reveals its children in sequence. */
  stagger: (step?: number, delay?: number) => Variants
  /** Child of `stagger` — a short rise plus fade. */
  riseItem: Variants
  /** Modal / popover surface. */
  pop: Variants
  /** In-place content swap inside an AnimatePresence — meta lines, inline forms. */
  swap: Variants
  overlay: Variants
}

export function useMotion(): MotionKit {
  const scale = useMotionScale()

  return useMemo<MotionKit>(() => {
    const t = (duration: number = D.base, delay = 0, ease: Cubic = EASE): Transition =>
      scale === 0 ? { duration: 0, delay: 0 } : { duration: duration * scale, delay: delay * scale, ease }

    const spring: Transition = scale === 0 ? { duration: 0 } : SPRING
    const springSoft: Transition = scale === 0 ? { duration: 0 } : SPRING_SOFT
    const v = <T,>(value: T, off: T): T => (scale === 0 ? off : value)
    const shift = (px: number): number => (scale === 0 ? 0 : px)

    const pageShift = shift(22)

    /**
     * The one page variant. `custom` is the direction from the nav store:
     * 1 = drill-in (list → detail), -1 = back, 0 = lateral (a title-bar tab
     * switch). Zero is not a degenerate case — it means the two screens are
     * peers, so it cross-fades with the same blur and no travel at all; only
     * an actual journey moves. One variant covers both, because a tab switch
     * and a drill-in must not be told apart by remembering two code paths.
     */
    const page: Variants = {
      enter: (dir: number = 1) => ({ opacity: 0, x: dir * pageShift, filter: 'blur(1px)' }),
      center: { opacity: 1, x: 0, filter: 'blur(0px)', transition: t(D.page) },
      exit: (dir: number = 1) => ({
        opacity: 0,
        x: dir * -pageShift * 0.6,
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
      hidden: { opacity: 0, y: shift(10) },
      show: { opacity: 1, y: 0, transition: t(D.base) },
      out: { opacity: 0, y: shift(-6), transition: t(D.micro) },
    }

    const pop: Variants = {
      hidden: { opacity: 0, y: shift(10), scale: v(0.97, 1) },
      show: { opacity: 1, y: 0, scale: 1, transition: t(D.base) },
      out: { opacity: 0, y: shift(6), scale: v(0.98, 1), transition: t(D.fast) },
    }

    const swap: Variants = {
      hidden: { opacity: 0, y: shift(5) },
      show: { opacity: 1, y: 0, transition: t(D.fast) },
      out: { opacity: 0, y: shift(-4), transition: t(D.micro) },
    }

    const overlay: Variants = {
      hidden: { opacity: 0 },
      show: { opacity: 1, transition: t(D.fast) },
      out: { opacity: 0, transition: t(D.fast) },
    }

    return {
      scale,
      t,
      v,
      shift,
      spring,
      springSoft,
      page,
      stagger,
      riseItem,
      pop,
      swap,
      overlay,
    }
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

/* ------------------------------ overlay layers ----------------------------- *
 * The global stacking ladder, lowest → highest. Every overlay references a
 * token from here instead of a raw z-[..], so the relative order lives in
 * exactly one place: page-local scrims and menus under toasts under modals,
 * and tooltips — which can be opened from inside any of the others — above
 * everything. (Tooltips once shared the modal level and only out-stacked it by
 * DOM append order — an accident, not a rule; 2026-09-12 review. The raw
 * z-10/20/30/40/50 values that had grown up beside it were pulled in here so
 * "who is on top" is one readable list instead of a grep.)
 */
/** Page-local sticky bars, cleared by their own scrolling content. */
export const STICKY_Z = 'z-10'
/** The launch dock pinned to the shell, under the page it serves. */
export const DOCK_Z = 'z-20'
/** The title bar: above the dock, below every overlay it can open. */
export const CHROME_Z = 'z-30'
/** In-page scrims and progress covers — above their page, below app chrome. */
export const LOCAL_OVERLAY_Z = 'z-40'
/** Anchored dropdown panels (the Menu, the task centre, custom selects). */
export const MENU_Z = 'z-[60]'
/** The toast stack under the title bar. */
export const TOAST_Z = 'z-[70]'
/** Full-screen modal wrappers (dialog, palette, guide) — mutually exclusive. */
export const MODAL_Z = 'z-[80]'
/** Floating hint bubbles; may surface from inside any layer above. */
export const TOOLTIP_Z = 'z-[90]'
