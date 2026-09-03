/**
 * Sampling for the plugin discovery strip.
 *
 * The naive `array.sort(() => Math.random() - 0.5)` is deliberately not used
 * anywhere here: that comparator is not a consistent ordering, so the result
 * is measurably skewed rather than uniform — items keep a strong bias toward
 * their original position. Everything below is a partial Fisher–Yates draw.
 */

/**
 * `count` items drawn uniformly, without replacement.
 *
 * Only the first `count` positions are settled, so the shuffle itself is
 * O(count) rather than O(n). The array copy in front of it is deliberate:
 * rejection sampling would avoid it but degenerates into coupon-collector
 * behaviour once `count` approaches the pool size, and the pools here are
 * small enough (≤ a few thousand, once per click) that the copy is free.
 */
export function sampleUniform<T>(pool: readonly T[], count: number): T[] {
  const take = Math.min(count, pool.length)
  if (take <= 0) return []
  const items = pool.slice()
  for (let i = 0; i < take; i++) {
    const j = i + Math.floor(Math.random() * (items.length - i))
    const swap = items[i]
    items[i] = items[j]
    items[j] = swap
  }
  items.length = take
  return items
}

/**
 * `count` items split between the pool's popular head and its long tail.
 *
 * Neither extreme works on its own for a catalogue this shape. Drawing
 * uniformly from ~1800 registry entries returns mostly abandoned repos, so
 * the strip looks broken; drawing by popularity just reprints the market's
 * own first page, so it stops being a recommendation. Half the draw comes
 * from each side, which keeps a quality floor while still surfacing things
 * the user would never have scrolled to.
 *
 * `headShare` is a *fraction* of the pool rather than a fixed rank so the
 * split keeps its meaning as the registry grows — a hardcoded "top 200"
 * would silently become "top 2%" at 10k entries.
 */
export function sampleStratified<T>(
  pool: readonly T[],
  count: number,
  score: (item: T) => number,
  headShare = 0.1,
): T[] {
  if (count <= 0 || pool.length === 0) return []
  if (pool.length <= count) return sampleUniform(pool, count)

  const ranked = pool.slice().sort((a, b) => score(b) - score(a))
  const fromHead = Math.ceil(count / 2)
  // The head must be wide enough to draw from without exhausting it, or the
  // same few plugins would come back on every reroll.
  const headSize = Math.min(
    ranked.length - 1,
    Math.max(Math.ceil(ranked.length * headShare), fromHead * 2),
  )

  const head = sampleUniform(ranked.slice(0, headSize), fromHead)
  const tail = sampleUniform(ranked.slice(headSize), count - head.length)
  // Whichever side ran short, top up from the other so the strip is always
  // full — a half-empty recommendation reads as a bug.
  const picked = [...head, ...tail]
  if (picked.length < count) {
    const chosen = new Set(picked)
    const rest = ranked.filter((item) => !chosen.has(item))
    picked.push(...sampleUniform(rest, count - picked.length))
  }

  // Shuffle the union so the block does not visibly separate into a popular
  // half followed by an obscure one.
  return sampleUniform(picked, picked.length)
}
