/**
 * Icon lab — renders PHL app-icon concepts to PNG with no image dependencies.
 *
 * Zero-dependency rasterizer (same technique as scripts/make-icon.mjs: 24-unit
 * design grid, 4x4 supersampling, hand-rolled PNG encoder). It exists so icon
 * concepts can be compared on real pixels at real sizes before one is adopted.
 *
 *   node scripts/icon-lab.mjs
 */
import { deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_DIR = resolve(ROOT, 'src-tauri/icons/candidates')

const GRID = 24
const SS = 4

/* ---- palette (aligned with src/index.css accent tokens) ---- */
const AZURE_TOP = [81, 138, 246]   // hsl(219 90% 64%)
const AZURE_BOT = [18, 84, 206]    // hsl(219 84% 44%)
const WHITE = [255, 255, 255]
const INK = [16, 26, 45]
/* DeepSeek brand blue #4D6BFE and a shell gradient built from it. */
const DS = [77, 107, 254]
const DS_TOP = [114, 145, 255]
const DS_BOT = [64, 92, 236]


/* ---- shape helpers ---- */
const R = (x, y, w, h, r, fill, alpha = 1, rot = 0) => ({ t: 'rrect', x, y, w, h, r, fill, alpha, rot })
const C = (cx, cy, r, fill, alpha = 1, mode) => ({ t: 'circle', cx, cy, r, fill, alpha, mode })
const RR = (x, y, w, h, ro, ri, fill, alpha = 1) => ({ t: 'rring', x, y, w, h, ro, ri, fill, alpha })
const CR = (cx, cy, ro, ri, fill, alpha = 1) => ({ t: 'cring', cx, cy, ro, ri, fill, alpha })

/** Rotate a sample point into a shape's own frame (degrees, around its centre). */
const unrotate = (px, py, s) => {
  if (!s.rot) return [px, py]
  const cx = s.t === 'circle' || s.t === 'ellipse' || s.t === 'cring' ? s.cx : s.x + s.w / 2
  const cy = s.t === 'circle' || s.t === 'ellipse' || s.t === 'cring' ? s.cy : s.y + s.h / 2
  const a = (-s.rot * Math.PI) / 180
  const dx = px - cx
  const dy = py - cy
  return [cx + dx * Math.cos(a) - dy * Math.sin(a), cy + dx * Math.sin(a) + dy * Math.cos(a)]
}

const inRRect = (px, py, x, y, w, h, r) => {
  const cx = Math.min(Math.max(px, x + r), x + w - r)
  const cy = Math.min(Math.max(py, y + r), y + h - r)
  const dx = px - cx
  const dy = py - cy
  return dx * dx + dy * dy <= r * r
}
const inCircle = (px, py, cx, cy, r) => {
  const dx = px - cx
  const dy = py - cy
  return dx * dx + dy * dy <= r * r
}

function insideShape(rawX, rawY, s) {
  const [px, py] = unrotate(rawX, rawY, s)
  switch (s.t) {
    case 'rrect': return inRRect(px, py, s.x, s.y, s.w, s.h, s.r)
    case 'circle': return inCircle(px, py, s.cx, s.cy, s.r)
    case 'ellipse': {
      const nx = (px - s.cx) / s.rx
      const ny = (py - s.cy) / s.ry
      return nx * nx + ny * ny <= 1
    }
    case 'rring':
      return inRRect(px, py, s.x, s.y, s.w, s.h, s.ro) && !inRRect(px, py, s.x + s.ro - s.ri, s.y + s.ro - s.ri, s.w - 2 * (s.ro - s.ri), s.h - 2 * (s.ro - s.ri), s.ri)
    case 'cring': return inCircle(px, py, s.cx, s.cy, s.ro) && !inCircle(px, py, s.cx, s.cy, s.ri)
    default: return false
  }
}

function fillAt(fill, px, py, s) {
  if (!fill.grad) return fill.c
  const y0 = s.t === 'circle' || s.t === 'cring' ? s.cy - s.ro : s.y
  const y1 = s.t === 'circle' || s.t === 'cring' ? s.cy + s.ro : s.y + s.h
  const t = Math.min(1, Math.max(0, (py - y0) / (y1 - y0)))
  const [a, b] = fill.grad
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/** Correct source-over compositing of a straight-alpha tile onto another. */
function over(dst, src, n) {
  for (let i = 0; i < n; i++) {
    const j = i * 4
    const sa = src[j + 3] / 255
    const da = dst[j + 3] / 255
    const oa = sa + da * (1 - sa)
    if (oa <= 0) { dst[j] = 0; dst[j + 1] = 0; dst[j + 2] = 0; dst[j + 3] = 0; continue }
    dst[j] = Math.round((src[j] * sa + dst[j] * da * (1 - sa)) / oa)
    dst[j + 1] = Math.round((src[j + 1] * sa + dst[j + 1] * da * (1 - sa)) / oa)
    dst[j + 2] = Math.round((src[j + 2] * sa + dst[j + 2] * da * (1 - sa)) / oa)
    dst[j + 3] = Math.round(oa * 255)
  }
}

/**
 * Layer-aware renderer. A shape of type 'layer' is rasterised on its own
 * transparent buffer (so its erase shapes cannot cut the layers beneath) and
 * then composited source-over. Everything else is composited in order.
 */
function render(shapes, size) {
  const out = new Uint8Array(size * size * 4)
  let segment = []
  const flush = () => {
    if (!segment.length) return
    over(out, renderFlat(segment, size), size * size)
    segment = []
  }
  for (const s of shapes) {
    if (s.t === 'layer') { flush(); over(out, render(s.shapes, size), size * size) }
    else segment.push(s)
  }
  flush()
  return out
}

function renderFlat(shapes, size) {
  const px = new Uint8Array(size * size * 4)
  const scale = GRID / size
  const step = 1 / SS
  const samples = SS * SS
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const ux = (x + (sx + 0.5) * step) * scale
          const uy = (y + (sy + 0.5) * step) * scale
          let cr = 0, cg = 0, cb = 0, ca = 0
          for (const s of shapes) {
            if (!insideShape(ux, uy, s)) continue
            const al = s.alpha
            if (s.mode === 'erase') {
              cr *= 1 - al; cg *= 1 - al; cb *= 1 - al; ca *= 1 - al
              continue
            }
            const col = fillAt(s.fill, ux, uy, s)
            cr = col[0] * al + cr * (1 - al)
            cg = col[1] * al + cg * (1 - al)
            cb = col[2] * al + cb * (1 - al)
            ca = al + ca * (1 - al)
          }
          r += cr * ca; g += cg * ca; b += cb * ca; a += ca
        }
      }
      const i = (y * size + x) * 4
      if (a > 0) {
        px[i] = Math.round(r / a)
        px[i + 1] = Math.round(g / a)
        px[i + 2] = Math.round(b / a)
        px[i + 3] = Math.round((a / samples) * 255)
      }
    }
  }
  return px
}

/* ---- concepts ---- */
const CONTAINER = R(1, 1, 22, 22, 5.6, { grad: [AZURE_TOP, AZURE_BOT] })
/** Hairline inner rim: keeps the mark from bleeding into a dark taskbar. */
const RIM = RR(1, 1, 22, 22, 5.6, 5.2, { c: WHITE }, 0.18)
const SHELL_DS = R(1, 1, 22, 22, 5.6, { grad: [DS_TOP, DS_BOT] })
const SHELL_INK = R(1, 1, 22, 22, 5.6, { grad: [[30, 34, 48], [9, 11, 17]] })

/**
 * Whale fluke: two swept blades plus a peduncle. Built from rotated ellipses
 * so the lobes stay fat and rounded instead of reading as a letterform.
 */
const fluke = (dx = 0, dy = 0, color = WHITE, alpha = 1, k = 1) => {
  const S = (v) => v * k
  return [
    R(12 - S(0.95) + dx, 13.2 + dy, S(1.9), S(6.0), S(0.95), { c: color }, alpha),
    { t: 'ellipse', cx: 12 - S(3.5) + dx, cy: 11.3 + dy, rx: S(4.8), ry: S(2.95), rot: -15, fill: { c: color }, alpha },
    { t: 'ellipse', cx: 12 + S(3.5) + dx, cy: 11.3 + dy, rx: S(4.8), ry: S(2.95), rot: 15, fill: { c: color }, alpha },
    C(12 + dx, 13.2 + dy, S(2.1), { c: color }, alpha),
    { t: 'ellipse', cx: 12 + dx, cy: 9.4 + dy, rx: S(1.9), ry: S(3.1), fill: { c: color }, alpha: 1, mode: 'erase' },
  ]
}

/**
 * Geometric whale tail: a rounded diamond with a notch erased out of its lower
 * vertex — the classic "diving whale" silhouette, built only from primitives.
 */
const diamond = (cx, cy, half, color, alpha = 1, notch = 'bottom') => {
  const rOut = half * 0.3
  // Real distance from centre to a rounded vertex.
  const reach = (half - rOut) * Math.SQRT2 + rOut
  const e = reach * 0.42
  const vy = notch === 'top' ? cy - reach - e : cy + reach - e
  return [
    {
      t: 'layer',
      shapes: [
        { t: 'rrect', x: cx - half, y: cy - half, w: half * 2, h: half * 2, r: rOut, rot: 45, fill: { c: color }, alpha },
        // Notch: a smaller 45-degree diamond erased at a vertex, so the cut
        // edges stay parallel to the silhouette instead of a round bite.
        { t: 'rrect', x: cx - e, y: vy, w: e * 2, h: e * 2, r: e * 0.35, rot: 45, fill: { c: color }, alpha: 1, mode: 'erase' },
      ],
    },
  ]
}

/**
 * Whale fluke from pure geometry: two rounded diamonds overlapping so their
 * peaks form the lobe tips and the gap between them forms the notch, plus a
 * peduncle below. Reads as a diving whale without tracing anyone's artwork.
 */
const flukeGeom = (dx, dy, color, alpha = 1) => {
  const lobe = (cx) => [
    { t: 'rrect', x: cx - 5.2 + dx, y: 6.2 + dy, w: 10.4, h: 10.4, r: 1.7, rot: 45, fill: { c: color }, alpha },
  ]
  return [
    R(10.7 + dx, 13.4 + dy, 2.6, 7.0, 1.3, { c: color }, alpha),
    ...lobe(9.6),
    ...lobe(14.4),
  ]
}

const CONCEPTS = {
  // 1. Versions stacked — keeps today's continuity, fixed to read as a deck.
  stack: [
    CONTAINER,
    R(6.4, 6.2, 11.2, 3.3, 1.65, { c: WHITE }, 0.97),
    R(7.4, 10.8, 11.2, 3.3, 1.65, { c: WHITE }, 0.74),
    R(8.4, 15.4, 9.2, 3.3, 1.65, { c: WHITE }, 0.55),
  ],
  // 2. Isolated environments — 2x2 compartments, one active.
  cells: [
    CONTAINER,
    R(4.8, 4.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.97),
    R(12.8, 4.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.5),
    R(4.8, 12.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.5),
    R(12.8, 12.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.5),
  ],
  // 3. Same idea, empty slots drawn as outlines for stronger contrast.
  slots: [
    CONTAINER,
    R(4.8, 4.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.97),
    RR(12.8, 4.8, 6.4, 6.4, 2.1, 0.95, { c: WHITE }, 0.85),
    RR(4.8, 12.8, 6.4, 6.4, 2.1, 0.95, { c: WHITE }, 0.85),
    RR(12.8, 12.8, 6.4, 6.4, 2.1, 0.95, { c: WHITE }, 0.85),
  ],
  // 4. Sandbox nesting — runtime inside instance.
  nested: [
    CONTAINER,
    RR(5.4, 5.4, 13.2, 13.2, 3.6, 1.7, { c: WHITE }, 0.95),
    R(9.6, 9.6, 4.8, 4.8, 1.5, { c: WHITE }, 1),
  ],
  // 5. Monogram P — the bowl counter is the container showing through.
  monogram: [
    CONTAINER,
    R(7.6, 5.0, 3.8, 14.0, 1.9, { c: WHITE }, 1),
    CR(14.0, 9.8, 4.7, 2.5, { c: WHITE }, 1),
  ],
  // 6. Coexisting copies — two offset tiles, front one dominant.
  split: [
    CONTAINER,
    R(4.6, 7.4, 10.2, 10.2, 2.9, { c: WHITE }, 0.46),
    R(9.2, 6.2, 10.2, 10.2, 2.9, { c: WHITE }, 0.97),
  ],
  // 7. Dark counterweight — same grid, one cell in ink for a two-tone mark.
  twotone: [
    CONTAINER,
    R(4.8, 4.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.97),
    R(12.8, 4.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.55),
    R(4.8, 12.8, 6.4, 6.4, 2.1, { c: WHITE }, 0.55),
    R(12.8, 12.8, 6.4, 6.4, 2.1, { c: INK }, 0.92),
  ],
  // 8. Nested, retuned for 16px: thicker ring, tighter core, rimmed shell.
  nested2: [
    CONTAINER,
    RIM,
    RR(5.2, 5.2, 13.6, 13.6, 3.7, 1.8, { c: WHITE }, 0.97),
    R(9.8, 9.8, 4.4, 4.4, 1.4, { c: WHITE }, 1),
  ],
  // 10. Whale fluke alone on the DeepSeek shell — one shape, best at 16px.
  w_tail: [SHELL_DS, RIM, ...fluke(0, 0, WHITE, 1)],
  // 11. Two flukes: several instances, one active.
  w_tail_stack: [
    SHELL_DS,
    RIM,
    ...fluke(-0.5, -2.4, WHITE, 0.38, 0.86),
    ...fluke(0.5, 0.9, WHITE, 1),
  ],
  // 12. Dark shell (opencode/pi lesson) with the DeepSeek-blue fluke.
  w_tail_dark: [
    SHELL_INK,
    RR(1, 1, 22, 22, 5.6, 5.2, { c: WHITE }, 0.1),
    ...fluke(0, 0, DS, 1),
  ],
  // 22. Tail up: notch at the top vertex — flukes up, peduncle tapering down.
  w_tailup: [
    SHELL_DS,
    RIM,
    ...diamond(12, 12, 7.2, WHITE, 1, 'top'),
  ],
  // 23. Tail up with a peduncle below the taper.
  w_tailup_stem: [
    SHELL_DS,
    RIM,
    R(10.9, 16.0, 2.2, 5.2, 1.1, { c: WHITE }, 1),
    ...diamond(12, 10.8, 7.0, WHITE, 1, 'top'),
  ],
  // 24. Tail up on the dark shell.
  w_tailup_dark: [
    SHELL_INK,
    RR(1, 1, 22, 22, 5.6, 5.2, { c: WHITE }, 0.17),
    ...diamond(12, 12, 7.2, DS, 1, 'top'),
  ],
  // 19. True fluke: two diamonds unioned (two lobes, top notch) over a peduncle.
  w_fluke: [
    SHELL_DS,
    RIM,
    ...flukeGeom(0, 0, WHITE, 1),
  ],
  // 20. Same fluke on the dark shell.
  w_fluke_dark: [
    SHELL_INK,
    RR(1, 1, 22, 22, 5.6, 5.2, { c: WHITE }, 0.1),
    ...flukeGeom(0, 0, DS, 1),
  ],
  // 21. Fluke over one isolated slab — the instance that surfaced.
  w_fluke_layer: [
    SHELL_DS,
    RIM,
    R(6.2, 19.0, 11.6, 2.0, 1.0, { c: WHITE }, 0.4),
    ...flukeGeom(0, -1.2, WHITE, 1),
  ],
  // 15. Geometric whale tail: rounded diamond, notch erased from the bottom vertex.
  w_diamond: [
    SHELL_DS,
    RIM,
    ...diamond(12, 12.2, 6.9, WHITE, 1),
  ],
  // 16. Two diamonds — several instances, one active.
  w_diamond_stack: [
    SHELL_DS,
    RIM,
    ...diamond(13.6, 13.9, 6.3, WHITE, 0.42),
    ...diamond(10.4, 11.2, 6.6, WHITE, 1),
  ],
  // 17. Diamond as the whole silhouette — no shell at all (PCL lesson).
  w_diamond_bare: [
    ...diamond(12, 12, 10.4, DS, 1),
  ],
  // 18. Dark shell, DeepSeek-blue diamond.
  w_diamond_dark: [
    SHELL_INK,
    RR(1, 1, 22, 22, 5.6, 5.2, { c: WHITE }, 0.1),
    ...diamond(12, 12.2, 6.9, DS, 1),
  ],
  // 14. Fluke alone, no shell: the silhouette itself is the brand (PCL lesson).
  w_tail_bare: [
    { t: 'rrect', x: 1, y: 1, w: 22, h: 22, r: 5.6, fill: { grad: [DS_TOP, DS_BOT] }, alpha: 0, mode: 'erase' },
    ...fluke(0, 0, DS, 1, 1.18),
  ],
  // 13. Fluke above one isolated slab — instance surfacing out of its environment.
  w_tail_layer: [
    SHELL_DS,
    RIM,
    R(6.4, 18.4, 11.2, 2.2, 1.1, { c: WHITE }, 0.4),
    ...fluke(0, -1.4, WHITE, 1),
  ],
  // 9. Slots, retuned: heavier stroke so the empty slots survive at 16px.
  slots2: [
    CONTAINER,
    RIM,
    R(4.6, 4.6, 6.8, 6.8, 2.2, { c: WHITE }, 0.97),
    RR(12.6, 4.6, 6.8, 6.8, 2.2, 0.9, { c: WHITE }, 0.9),
    RR(4.6, 12.6, 6.8, 6.8, 2.2, 0.9, { c: WHITE }, 0.9),
    RR(12.6, 12.6, 6.8, 6.8, 2.2, 0.9, { c: WHITE }, 0.9),
  ],
}

/* ---- PNG encoding ---- */
const CRC_TABLE = (() => {
  const table = new Int32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    table[n] = c
  }
  return table
})()
const crc32 = (buf) => {
  let c = -1
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8)
  return (c ^ -1) >>> 0
}
function chunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, crc])
}
function encodePng(pixels, width, height) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8
  ihdr[9] = 6
  const stride = width * 4
  const raw = Buffer.alloc((stride + 1) * height)
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0
    Buffer.from(pixels.buffer, pixels.byteOffset + y * stride, stride).copy(raw, y * (stride + 1) + 1)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

/* ---- compositing ---- */
function blit(dst, dw, dh, src, s, ox, oy) {
  for (let y = 0; y < s; y++) {
    const ty = oy + y
    if (ty < 0 || ty >= dh) continue
    for (let x = 0; x < s; x++) {
      const tx = ox + x
      if (tx < 0 || tx >= dw) continue
      const si = (y * s + x) * 4
      const a = src[si + 3] / 255
      if (a <= 0) continue
      const di = (ty * dw + tx) * 4
      dst[di] = Math.round(src[si] * a + dst[di] * (1 - a))
      dst[di + 1] = Math.round(src[si + 1] * a + dst[di + 1] * (1 - a))
      dst[di + 2] = Math.round(src[si + 2] * a + dst[di + 2] * (1 - a))
      dst[di + 3] = 255
    }
  }
}

function sheet(concepts, bg) {
  const names = Object.keys(concepts)
  const tile = 256
  const gap = 24
  const margin = 24
  const smalls = [48, 32, 16]
  const smallRow = 48
  const W = margin * 2 + names.length * tile + (names.length - 1) * gap
  const H = margin * 2 + tile + 16 + smallRow
  const buf = new Uint8Array(W * H * 4)
  for (let i = 0; i < W * H; i++) {
    buf[i * 4] = bg[0]; buf[i * 4 + 1] = bg[1]; buf[i * 4 + 2] = bg[2]; buf[i * 4 + 3] = 255
  }
  names.forEach((name, idx) => {
    const x0 = margin + idx * (tile + gap)
    blit(buf, W, H, render(concepts[name], tile), tile, x0, margin)
    let sx = x0
    for (const s of smalls) {
      blit(buf, W, H, render(concepts[name], s), s, sx, margin + tile + 16 + (smallRow - s))
      sx += s + 10
    }
  })
  return { png: encodePng(buf, W, H), W, H }
}

/** Small-size sheet: renders at real 16px/32px then magnifies with hard pixels. */
function zoomSheet(concepts, bg) {
  const names = Object.keys(concepts)
  const cols = names.length
  const cell = 160
  const gap = 16
  const margin = 24
  const W = margin * 2 + cols * cell + (cols - 1) * gap
  const H = margin * 2 + cell * 2 + 16
  const buf = new Uint8Array(W * H * 4)
  for (let i = 0; i < W * H; i++) {
    buf[i * 4] = bg[0]; buf[i * 4 + 1] = bg[1]; buf[i * 4 + 2] = bg[2]; buf[i * 4 + 3] = 255
  }
  const magnify = (src, s, f) => {
    const out = new Uint8Array(s * f * s * f * 4)
    for (let y = 0; y < s * f; y++) {
      for (let x = 0; x < s * f; x++) {
        const si = (Math.floor(y / f) * s + Math.floor(x / f)) * 4
        const di = (y * s * f + x) * 4
        out[di] = src[si]; out[di + 1] = src[si + 1]; out[di + 2] = src[si + 2]; out[di + 3] = src[si + 3]
      }
    }
    return out
  }
  names.forEach((name, idx) => {
    const x0 = margin + idx * (cell + gap)
    const big = magnify(render(concepts[name], 16), 16, 10)
    blit(buf, W, H, big, 160, x0, margin)
    const mid = magnify(render(concepts[name], 32), 32, 5)
    blit(buf, W, H, mid, 160, x0, margin + cell + 16)
  })
  return { png: encodePng(buf, W, H), W, H }
}

/**
 * Review sheet: the shortlist at 256px plus native 48/32/16 and a 3x
 * magnified 16px so the small-size drawing can be judged honestly.
 */
const REVIEW = ['w_tailup', 'w_tailup_dark', 'nested2', 'monogram', 'twotone']

function reviewSheet(bg) {
  const cols = REVIEW.length
  const big = 256
  const gap = 24
  const margin = 24
  const smallRow = 48
  const zoomRow = 48
  const W = margin * 2 + cols * big + (cols - 1) * gap
  const H = margin * 2 + big + 16 + smallRow + 12 + zoomRow
  const buf = new Uint8Array(W * H * 4)
  for (let i = 0; i < W * H; i++) {
    buf[i * 4] = bg[0]; buf[i * 4 + 1] = bg[1]; buf[i * 4 + 2] = bg[2]; buf[i * 4 + 3] = 255
  }
  const magnify = (src, s, f) => {
    const out = new Uint8Array(s * f * s * f * 4)
    for (let y = 0; y < s * f; y++) {
      for (let x = 0; x < s * f; x++) {
        const si = (Math.floor(y / f) * s + Math.floor(x / f)) * 4
        const di = (y * s * f + x) * 4
        out[di] = src[si]; out[di + 1] = src[si + 1]; out[di + 2] = src[si + 2]; out[di + 3] = src[si + 3]
      }
    }
    return out
  }
  REVIEW.forEach((name, idx) => {
    const x0 = margin + idx * (big + gap)
    blit(buf, W, H, render(CONCEPTS[name], big), big, x0, margin)
    let sx = x0
    for (const s of [48, 32, 16]) {
      blit(buf, W, H, render(CONCEPTS[name], s), s, sx, margin + big + 16 + (smallRow - s))
      sx += s + 8
    }
    blit(buf, W, H, magnify(render(CONCEPTS[name], 16), 16, 3), 48, x0, margin + big + 16 + smallRow + 12)
  })
  return encodePng(buf, W, H)
}

mkdirSync(OUT_DIR, { recursive: true })
for (const [name, shapes] of Object.entries(CONCEPTS)) {
  writeFileSync(resolve(OUT_DIR, name + '-512.png'), encodePng(render(shapes, 512), 512, 512))
}
const light = sheet(CONCEPTS, [244, 246, 250])
writeFileSync(resolve(OUT_DIR, 'sheet-light.png'), light.png)
const dark = sheet(CONCEPTS, [22, 24, 30])
writeFileSync(resolve(OUT_DIR, 'sheet-dark.png'), dark.png)
const zoom = zoomSheet(CONCEPTS, [244, 246, 250])
writeFileSync(resolve(OUT_DIR, 'zoom-16-32.png'), zoom.png)
writeFileSync(resolve(OUT_DIR, 'review-light.png'), reviewSheet([244, 246, 250]))
writeFileSync(resolve(OUT_DIR, 'review-dark.png'), reviewSheet([22, 24, 30]))
console.log('concepts: ' + Object.keys(CONCEPTS).join(', '))
console.log('sheet: ' + light.W + 'x' + light.H + ' -> ' + OUT_DIR)
