import { describe, expect, it } from 'vitest'
import { resolveTooltipPlacement, type TooltipAnchor } from './tooltipPlacement'

const rect = (left: number, top: number, width = 40, height = 24): TooltipAnchor => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height,
})

describe('resolveTooltipPlacement', () => {
  const vw = 1252
  const vh = 800

  it('centers a top tooltip above the trigger', () => {
    const p = resolveTooltipPlacement('top', rect(600, 400), 96, 28, vw, vh)
    expect(p.side).toBe('top')
    expect(p.y).toBe(394)
    expect(p.ty).toBe('-100%')
    expect(p.x).toBe(620)
    expect(p.tx).toBe('-50%')
  })

  it('clamps a right-edge trigger instead of shrinking the bubble', () => {
    // Regression: a fixed bubble without a set width shrink-to-fits into the
    // space between its left edge and the viewport — the one-glyph-per-line
    // black strip on the instance card's「更多操作」.
    const p = resolveTooltipPlacement('top', rect(1200, 400, 44, 24), 96, 28, vw, vh)
    expect(p.x).toBe(vw - 8)
    expect(p.tx).toBe('-100%')
  })

  it('clamps a left-edge trigger', () => {
    const p = resolveTooltipPlacement('top', rect(2, 400), 96, 28, vw, vh)
    expect(p.x).toBe(8)
    expect(p.tx).toBe('0%')
  })

  it('flips below when there is no room above (page-top rows)', () => {
    const p = resolveTooltipPlacement('top', rect(600, 10), 96, 28, vw, vh)
    expect(p.side).toBe('bottom')
    expect(p.y).toBe(40)
    expect(p.ty).toBe('0%')
  })

  it('bottom preference mirrors the flip near the viewport bottom', () => {
    const p = resolveTooltipPlacement('bottom', rect(600, vh - 30), 96, 28, vw, vh)
    expect(p.side).toBe('top')
    expect(p.ty).toBe('-100%')
  })

  it('keeps the requested side when neither side fits', () => {
    const p = resolveTooltipPlacement('top', rect(600, 200, 40, 590), 96, 900, vw, vh)
    expect(p.side).toBe('top')
  })

  it('left/right sides anchor to the facing edge, vertically centered and clamped', () => {
    const p = resolveTooltipPlacement('right', rect(600, 400), 96, 28, vw, vh)
    expect(p.x).toBe(646)
    expect(p.tx).toBe('0%')
    expect(p.y).toBe(412)
    expect(p.ty).toBe('-50%')

    const clipped = resolveTooltipPlacement('right', rect(600, vh - 20), 96, 80, vw, vh)
    expect(clipped.y).toBe(vh - 8)
    expect(clipped.ty).toBe('-100%')
  })
})
