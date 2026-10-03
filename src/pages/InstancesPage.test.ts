import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Regression guard for the "card view renders as full-width bars" bug.
 *
 * The instances grid picks its column count from a CSS **container query** on
 * the content column, not from viewport breakpoints. Viewport breakpoints were
 * wrong here for two compounding reasons: the fixed sidebar eats part of the
 * window, and at 125% system DPI a ~1250px window is only ~1000px of CSS
 * width — under `lg` — so `grid-cols-1` won and every card rendered as a
 * one-row-wide bar indistinguishable from the list view.
 *
 * Two invariants keep that from coming back, and both fail *silently* if
 * broken, which is why they are pinned here rather than left to a screenshot:
 *
 *   1. The container must be declared (`container-type: inline-size`) on the
 *      scope wrapper AND the grid must actually sit inside it, or the
 *      `@container` rules match nothing and every card is full width.
 *   2. A `@supports not (container-type: inline-size)` fallback must exist. An
 *      engine without container queries ignores the `@container` block
 *      wholesale — no error, no warning — and the bars come back. The fallback
 *      is the only thing standing between an older engine and the bug.
 *
 * The test infra here is node-only (no DOM / layout engine), so this checks
 * the invariant at the source level — same static-check style as
 * `scripts/check-tauri-bridge.mjs` and `../components/instance/InstanceCard.test.ts`.
 * The rendered column counts were measured against a real browser instead
 * (container width → 480px: 2 cols, 900px: 3 cols, 1140px: 4 cols).
 */

function read(relative: string): string {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')
}

const css = read('../index.css')
const page = read('./InstancesPage.tsx')

/** The grid block only; a stray `@container` elsewhere must not satisfy these. */
function gridBlock(): string {
  const start = css.indexOf('.inst-grid-scope')
  expect(start, 'grid block (.inst-grid-scope) is missing from index.css').toBeGreaterThan(-1)
  // The block runs until the next top-level comment, which is the motion block.
  const end = css.indexOf("/* The in-app \"动画：关闭\"", start)
  return css.slice(start, end === -1 ? undefined : end)
}

describe('instances card grid', () => {
  it('declares the container on the scope wrapper', () => {
    expect(gridBlock()).toContain('container-type: inline-size')
  })

  it('wraps the grid in that scope, so the @container rules have a container to match', () => {
    // Without the wrapper the query has no container and silently never fires.
    // Assert on the className literal rather than a substring: `inst-grid-scope`
    // is a prefix of `inst-grid-scope-foo`, so a rename that keeps the prefix
    // would slip past a naive `toContain`.
    expect(page).toMatch(/className=(["'])inst-grid-scope\1/)
    const scopeAt = page.indexOf('className="inst-grid-scope"')
    const gridAt = page.indexOf("'inst-grid'")
    expect(gridAt, "the grid's `inst-grid` class is missing from InstancesPage").toBeGreaterThan(-1)
    expect(scopeAt).toBeLessThan(gridAt)
  })

  it('does not wrap the grid in a <div> that breaks the container', () => {
    // `display: contents` on the scope wrapper would drop the box the query
    // measures against. `width: 100%` keeps the scope a real layout box.
    expect(gridBlock()).toContain('.inst-grid-scope')
    expect(gridBlock()).toMatch(/\.inst-grid-scope\s*\{[^}]*width:\s*100%/)
  })

  it('keeps the three container thresholds in ascending order', () => {
    const widths = [...gridBlock().matchAll(/@container \(min-width: (\d+)px\)/g)].map((m) => Number(m[1]))
    expect(widths).toEqual([...widths].sort((a, b) => a - b))
    expect(widths).toHaveLength(3)
  })

  it('ships the @supports fallback for engines without container queries', () => {
    // An engine that does not understand `container-type` drops the whole
    // @container block with no warning; the fallback is the only net.
    const block = gridBlock()
    expect(block).toContain('@supports not (container-type: inline-size)')
    // The fallback must carry viewport breakpoints of its own, or it is empty.
    const fallbackAt = block.indexOf('@supports not (container-type: inline-size)')
    expect(block.slice(fallbackAt)).toMatch(/@media \(min-width: \d+px\)/)
  })
})
