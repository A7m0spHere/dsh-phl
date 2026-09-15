// Pure placement math for the Tooltip bubble, kept free of React/motion so it
// can be reasoned about and exercised in isolation (same split as
// menuPlacement.ts for the Menu panel). The bubble is portalled to <body> and
// positioned `fixed` — that already escapes every ancestor's overflow
// clipping; this module only decides where on screen it lands.

export const TOOLTIP_GAP = 6
const EDGE = 8

/** The trigger's `getBoundingClientRect()` snapshot, taken when the tooltip arms. */
export interface TooltipAnchor {
  top: number
  right: number
  bottom: number
  left: number
  width: number
  height: number
}

export type TooltipSide = 'top' | 'bottom' | 'left' | 'right'

export interface TooltipPlacement {
  /** `fixed` anchor point; `tx`/`ty` are the CSS-translate percentages that
   * align the bubble to it (center / leading / trailing edge). */
  x: number
  y: number
  tx: '-50%' | '-100%' | '0%'
  ty: '-50%' | '-100%' | '0%'
  /** The side actually used — `top`/`bottom` flip when that side cannot fit. */
  side: TooltipSide
}

/**
 * Choose where the measured bubble (bw × bh) lands for a trigger rect.
 *
 * The bubble size must be *measured*, never estimated: a `fixed` element
 * without a set width shrink-to-fits into the space between its `left` and
 * the viewport edge, so a guessed flip threshold is how right-edge triggers
 * got a one-glyph-per-line black strip (2026-09-12 acceptance: the instance
 * card's「更多操作」). Horizontal: centered on the trigger, clamped into the
 * viewport — near an edge the bubble shifts rather than spills. Vertical:
 * the requested side wins unless it cannot fit and the other side can; when
 * neither fits, keep the request (a bubble aligned to its trigger beats a
 * silently reversed one). Left/right sides mirror the rules on the other axis.
 */
export function resolveTooltipPlacement(
  side: TooltipSide,
  r: TooltipAnchor,
  bw: number,
  bh: number,
  viewportW: number,
  viewportH: number,
): TooltipPlacement {
  const ax = r.left + r.width / 2
  const ay = r.top + r.height / 2

  if (side === 'left' || side === 'right') {
    let y = ay
    let ty: TooltipPlacement['ty'] = '-50%'
    if (y - bh / 2 < EDGE) {
      y = EDGE
      ty = '0%'
    } else if (y + bh / 2 > viewportH - EDGE) {
      y = viewportH - EDGE
      ty = '-100%'
    }
    return {
      x: side === 'left' ? r.left - TOOLTIP_GAP : r.right + TOOLTIP_GAP,
      y,
      tx: side === 'left' ? '-100%' : '0%',
      ty,
      side,
    }
  }

  const spaceTop = r.top - TOOLTIP_GAP
  const spaceBottom = viewportH - r.bottom - TOOLTIP_GAP
  let placed: 'top' | 'bottom' = side
  if (placed === 'top' && spaceTop < bh + EDGE && spaceBottom > spaceTop) placed = 'bottom'
  else if (placed === 'bottom' && spaceBottom < bh + EDGE && spaceTop > spaceBottom) placed = 'top'

  let x = ax
  let tx: TooltipPlacement['tx'] = '-50%'
  if (x - bw / 2 < EDGE) {
    x = EDGE
    tx = '0%'
  } else if (x + bw / 2 > viewportW - EDGE) {
    x = viewportW - EDGE
    tx = '-100%'
  }

  return {
    x,
    y: placed === 'top' ? r.top - TOOLTIP_GAP : r.bottom + TOOLTIP_GAP,
    tx,
    ty: placed === 'top' ? '-100%' : '0%',
    side: placed,
  }
}
