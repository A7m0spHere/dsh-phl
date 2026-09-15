import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Regression guard for the "black bar" bug family: the instance card and
 * the detail page's sections clip their children with `overflow-hidden`
 * (rounded corners / height animations), and a tooltip that renders inside
 * that tree is cut down to a sliver of its dark bubble. `Tooltip` now has
 * exactly one placement path — every bubble portals to `document.body` and
 * is measured and clamped against the viewport — so there is no inline mode
 * left to forget opting out of (the source of the 2026-09-12 regressions:
 * the plugin page's「信任未知」sliver and the instance card's one-glyph-wide
 *「更多操作」strip).
 *
 * The test infra here is node-only (no DOM / layout engine), so this
 * verifies the invariant at the source level — same static-check style as
 * `scripts/check-tauri-bridge.mjs` — instead of rendering components. The
 * geometry itself is unit-tested in `../ui/tooltipPlacement.test.ts`.
 */

function source(name: string): string {
  return readFileSync(fileURLToPath(new URL(name, import.meta.url)), 'utf8')
}

describe('tooltip clipping inside overflow-hidden surfaces', () => {
  it('keeps InstanceCard clipping with overflow-hidden (rounded corners / EdgeProgress rely on it)', () => {
    expect(source('./InstanceCard.tsx')).toContain('overflow-hidden')
  })

  it('portals every bubble to document.body — no in-tree absolute branch', () => {
    const tip = source('../ui/Tooltip.tsx')
    expect(tip).toContain('createPortal(')
    expect(tip).not.toContain('allowOverflow')
  })

  it('sizes bubbles with w-max and clamps via the measured placement', () => {
    // Without a set width, the shrink-to-fit rules squeeze a `fixed` bubble
    // into whatever room is left between its anchor and the viewport edge —
    // the vertical one-glyph-per-line strip at the card list's right edge.
    const tip = source('../ui/Tooltip.tsx')
    expect(tip).toContain('w-max')
    expect(tip).toContain('resolveTooltipPlacement')
  })

  it('positions tooltip bubbles via the CSS translate property, not transform', () => {
    // motion.span animates x/y and writes its own inline transform (none
    // once settled), which clobbers any transform-based centering — both
    // the Tailwind -translate-* classes and the placement offsets. That
    // left bubbles left-anchored at the trigger, spilling past the
    // viewport right edge (2026-09-11 desktop acceptance: the detail-page
    // 编辑 tooltip). The CSS translate property is not transform; motion
    // cannot touch it.
    const tip = source('../ui/Tooltip.tsx')
    expect(tip).toContain('translate:')
    expect(tip).not.toMatch(/style=\{[\s\S]*?transform: `translate\(/)
  })
})
