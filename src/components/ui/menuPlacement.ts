// Pure placement math for the Menu popover, kept free of React/motion so it
// can be reasoned about and exercised in isolation. The panel itself is
// portalled to <body> and positioned `fixed` — that already escapes every
// ancestor's `overflow`/`transform` clipping; this module only decides where
// on screen the (unclipped) panel lands.

export const MENU_GAP = 6
const EDGE = 8

interface Rect {
  top: number
  bottom: number
  left: number
  right: number
}

export interface PlacementInput {
  rect: Rect
  /** Panel height (measured once mounted, else estimated). */
  height: number
  /** Panel width. */
  width: number
  align?: 'start' | 'end'
  side?: 'top' | 'bottom'
  viewportW: number
  viewportH: number
}

export interface Placement {
  top: number
  left: number
  dropUp: boolean
}

/**
 * Choose a `fixed` top/left for the panel, flipping it above the trigger when
 * it would run off the bottom of the viewport (and honouring an explicit
 * `side: 'top'` preference from bottom-anchored surfaces like the launch dock).
 * The panel stays at least EDGE px inside the viewport on all four sides.
 */
export function computePlacement({
  rect,
  height,
  width,
  align = 'end',
  side = 'bottom',
  viewportW,
  viewportH,
}: PlacementInput): Placement {
  const belowFits = rect.bottom + MENU_GAP + height <= viewportH - EDGE
  const aboveFits = rect.top - MENU_GAP - height >= EDGE
  // Prefer the requested side; flip only when that side cannot fit and the
  // other one can. When neither fits, keep the requested side (a long list is
  // still better aligned to its trigger than silently reversed).
  const dropUp =
    side === 'top' ? aboveFits || !belowFits : !belowFits && aboveFits
  const rawTop = dropUp ? rect.top - MENU_GAP - height : rect.bottom + MENU_GAP
  const top = Math.max(EDGE, Math.min(rawTop, viewportH - height - EDGE))
  const rawLeft = align === 'end' ? rect.right - width : rect.left
  const left = Math.max(EDGE, Math.min(rawLeft, viewportW - width - EDGE))
  return { top, left, dropUp }
}

/**
 * Height estimate used for the pre-mount flip decision: p-1 (8px) + one row
 * per item (py-6px top/bottom + ~17px line) + 9px per separator (my-1 8px +
 * the 1px hairline). Only the *decision* needs this; the layout effect
 * re-runs with the measured height before paint.
 */
export function estimateMenuHeight(
  items: { separated?: boolean }[],
  rowHeight = 30,
): number {
  return 8 + items.reduce((h, it) => h + rowHeight + (it.separated ? 9 : 0), 0)
}
