import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Regression guard for the "black bar" bug: the instance card and the
 * detail page's sections clip their children with `overflow-hidden`
 * (rounded corners / height animations), and an inline absolute tooltip
 * that crosses such an edge is cut down to a sliver of its dark bubble —
 * the black bar in the screenshot. `Tooltip`'s `allowOverflow` mode is the
 * designed escape: it portals the bubble to `document.body` and pins it to
 * the viewport with `fixed`.
 *
 * The test infra here is node-only (no DOM / layout engine), so this
 * verifies the invariant at the source level — same static-check style as
 * `scripts/check-tauri-bridge.mjs` — instead of rendering components.
 */

/** Every `<Tooltip ...>` opening tag in `source` (attribute text included). */
function tooltipTags(source: string): string[] {
  return source.match(/<Tooltip\b[\s\S]*?(?:'[^']*'|"[^"]*"|[^>])>/g) ?? []
}

function source(name: string): string {
  return readFileSync(fileURLToPath(new URL(name, import.meta.url)), 'utf8')
}

describe('tooltip clipping inside overflow-hidden surfaces', () => {
  for (const file of ['./InstanceCard.tsx', '../../pages/InstanceDetailPage.tsx']) {
    it(`every Tooltip inside ${file} escapes its clip container via allowOverflow`, () => {
      const tags = tooltipTags(source(file))
      expect(tags.length).toBeGreaterThan(0)
      const clipped = tags.filter((tag) => !tag.includes('allowOverflow'))
      expect(clipped, `Tooltips missing allowOverflow: ${clipped.join('\n')}`).toEqual([])
    })
  }

  it('keeps InstanceCard clipping with overflow-hidden (rounded corners / EdgeProgress rely on it)', () => {
    expect(source('./InstanceCard.tsx')).toContain('overflow-hidden')
  })

  it('keeps Tooltip allowOverflow opt-in (global default would change every bubble)', () => {
    expect(source('../ui/Tooltip.tsx')).toContain('allowOverflow = false')
  })
})
