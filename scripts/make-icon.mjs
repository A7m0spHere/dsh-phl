/**
 * Builds every PHL icon asset from the master stencil — no image dependencies.
 *
 *   node scripts/make-icon.mjs                      # default treatment into src-tauri/icons
 *   node scripts/make-icon.mjs --treatment=blue      # pick a treatment
 *   node scripts/make-icon.mjs --all --out=some/dir  # every treatment, side by side
 *
 * Output per treatment: the PNG ladder Tauri needs, a hand-built multi-size
 * icon.ico (Windows) and icon.icns (macOS). Small sizes are re-drawn rather
 * than downscaled: the stencil's edge is firmed up with an alpha gamma below
 * 20px, the internal negative space is dropped below 24px, and the mark gets
 * more of the canvas below 24px — none of which survives a blind resample.
 * See src-tauri/icons/master/PROVENANCE.md for the mark's origin.
 */
import {
  decodePng,
  encodePng,
  resample,
  contrastAlpha,
  medianAlpha,
  blurAlpha,
  over,
} from './lib/png.mjs'
import { mkdirSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const MASTER_DIR = resolve(ROOT, 'src-tauri/icons/master')

const DS = [77, 107, 254]        // DeepSeek brand blue #4D6BFE
const WHITE = [255, 255, 255]
const SOFT = [238, 243, 255]
const INK = [13, 16, 23]
const BORDER = [214, 224, 245]

/** PNG ladder Tauri's bundler and every desktop platform expect. */
const SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256, 512, 1024]
/** Sizes stored inside icon.ico, in the order Windows prefers them. */
const ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
/** icns type codes -> pixel size. */
const ICNS_TYPES = [['ic11', 32], ['ic12', 64], ['ic07', 128], ['ic08', 256], ['ic09', 512], ['ic10', 1024]]

const TREATMENTS = {
  bare: { shell: null, mark: DS, inset: 0.06 },
  white: { shell: WHITE, mark: DS, inset: 0.12, border: BORDER },
  soft: { shell: SOFT, mark: DS, inset: 0.12, border: BORDER },
  blue: { shell: DS, mark: WHITE, inset: 0.12 },
  dark: { shell: INK, mark: DS, inset: 0.12, rim: [255, 255, 255, 0.16] },
}
const DEFAULT_TREATMENT = 'bare'

const args = process.argv.slice(2)
const arg = (name, fallback) => {
  const hit = args.find((a) => a.startsWith('--' + name + '='))
  return hit ? hit.slice(name.length + 3) : fallback
}
const flag = (name) => args.some((a) => a === '--' + name || a.startsWith('--' + name + '='))
/** Small-size rendering strategy; see markFor. */
const SMALL_MODE = arg('small', 'hard')

const master = decodePng(resolve(MASTER_DIR, 'tail.png'))
const folds = decodePng(resolve(MASTER_DIR, 'tail-folds.png'))

/* ---------------- geometry ---------------- */

function rrectCov(px, py, x, y, w, h, r) {
  const cx = Math.min(Math.max(px, x + r), x + w - r)
  const cy = Math.min(Math.max(py, y + r), y + h - r)
  const dx = px - cx, dy = py - cy
  return dx * dx + dy * dy <= r * r
}

/** The mark alone, scaled and colourised for a target canvas size. */
function markFor(treatment, size) {
  // At icon sizes the mark claims more of the canvas — the shell padding that
  // looks generous at 256px eats the whole silhouette at 16px.
  const inset = treatment.inset * (size <= 24 ? 0.5 : 1)
  const box = size * (1 - inset * 2)
  const k = Math.min(box / master.width, box / master.height)
  const mw = Math.max(1, Math.round(master.width * k))
  const mh = Math.max(1, Math.round(master.height * k))
  let px = resample(master.data, master.width, master.height, mw, mh)
  // Small sizes need a firmer edge, not more pixels. A plain downscale leaves
  // the fluke tips a fraction of a pixel wide, so a third of the silhouette is
  // semi-transparent and the taskbar icon reads hazy.
  //
  // Modes (--small=...), all of which keep the approved `crisp` one reachable:
  //   soft   — gamma only; the original hazy look
  //   crisp  — gamma + alpha-contrast squeeze (default, user-approved)
  //   hard   — same squeeze, tighter band: harder edge, squarer corners
  //   median — binarise, 3x3 median clean-up, then a 1px feather
  // Rejected outright: max-dilation (closes the notch into a blob) and a
  // morphological opening (blunts the flukes without reducing the haze).
  const tiny = size <= 20
  if (tiny && SMALL_MODE === 'median') {
    const bin = new Uint8Array(px)
    for (let i = 0; i < mw * mh; i++) bin[i * 4 + 3] = px[i * 4 + 3] > 110 ? 255 : 0
    px = blurAlpha(medianAlpha(bin, mw, mh), mw, mh)
  } else {
    const gamma = tiny ? (SMALL_MODE === 'hard' ? 0.5 : SMALL_MODE === 'soft' ? 0.7 : 0.55) : size <= 32 ? 0.85 : 1
    if (gamma !== 1) {
      for (let i = 0; i < mw * mh; i++) {
        px[i * 4 + 3] = Math.round(255 * Math.pow(px[i * 4 + 3] / 255, gamma))
      }
    }
    if (size <= 32) {
      const band = tiny
        ? SMALL_MODE === 'hard'
          ? [0.45, 0.55]
          : SMALL_MODE === 'soft'
            ? null
            : [0.4, 0.6]
        : [0.3, 0.7]
      if (band) px = contrastAlpha(px, mw, mh, band[0], band[1])
    }
  }
  // Internal negative space is dropped below 24px — it would read as dirt.
  if (size > 24 && !flag('no-folds')) {
    const f = resample(folds.data, folds.width, folds.height, mw, mh)
    for (let i = 0; i < mw * mh; i++) {
      px[i * 4 + 3] = Math.round(px[i * 4 + 3] * (1 - f[i * 4 + 3] / 255))
    }
  }
  const c = treatment.mark
  for (let i = 0; i < mw * mh; i++) { px[i * 4] = c[0]; px[i * 4 + 1] = c[1]; px[i * 4 + 2] = c[2] }
  return { data: px, w: mw, h: mh }
}

function compose(treatment, size) {
  const out = new Uint8Array(size * size * 4)
  const SS = 4, step = 1 / SS, r = size * 0.225
  if (treatment.shell) {
    const fill = treatment.shell
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        let a = 0
        for (let sy = 0; sy < SS; sy++) for (let sx = 0; sx < SS; sx++) {
          if (rrectCov(x + (sx + 0.5) * step, y + (sy + 0.5) * step, 0, 0, size, size, r)) a++
        }
        a /= SS * SS
        const i = (y * size + x) * 4
        out[i] = fill[0]; out[i + 1] = fill[1]; out[i + 2] = fill[2]; out[i + 3] = Math.round(a * 255)
      }
    }
    if (treatment.border || treatment.rim) {
      const bc = treatment.rim ? treatment.rim.slice(0, 3) : treatment.border
      const strength = treatment.rim ? treatment.rim[3] : 1
      const band = size * (treatment.rim ? 0.010 : 0.008)
      for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
          let inside = 0, inner = 0
          for (let sy = 0; sy < SS; sy++) for (let sx = 0; sx < SS; sx++) {
            const px = x + (sx + 0.5) * step, py = y + (sy + 0.5) * step
            if (rrectCov(px, py, 0, 0, size, size, r)) inside++
            if (rrectCov(px, py, band, band, size - band * 2, size - band * 2, r - band)) inner++
          }
          const edge = ((inside - inner) / (SS * SS)) * strength
          if (edge <= 0) continue
          const i = (y * size + x) * 4
          out[i] = Math.round(bc[0] * edge + out[i] * (1 - edge))
          out[i + 1] = Math.round(bc[1] * edge + out[i + 1] * (1 - edge))
          out[i + 2] = Math.round(bc[2] * edge + out[i + 2] * (1 - edge))
        }
      }
    }
  }
  const m = markFor(treatment, size)
  over(out, size, size, m.data, m.w, m.h, Math.round((size - m.w) / 2), Math.round((size - m.h) / 2))
  return out
}

/* ---------------- containers ---------------- */

function buildIco(pngs) {
  const head = Buffer.alloc(6)
  head.writeUInt16LE(0, 0); head.writeUInt16LE(1, 2); head.writeUInt16LE(pngs.length, 4)
  const dir = Buffer.alloc(16 * pngs.length)
  let offset = 6 + 16 * pngs.length
  pngs.forEach((p, i) => {
    const o = i * 16
    dir[o] = p.size >= 256 ? 0 : p.size
    dir[o + 1] = p.size >= 256 ? 0 : p.size
    dir[o + 2] = 0; dir[o + 3] = 0
    dir.writeUInt16LE(1, o + 4)
    dir.writeUInt16LE(32, o + 6)
    dir.writeUInt32LE(p.png.length, o + 8)
    dir.writeUInt32LE(offset, o + 12)
    offset += p.png.length
  })
  return Buffer.concat([head, dir, ...pngs.map((p) => p.png)])
}

function buildIcns(pngs) {
  const parts = []
  let total = 8
  for (const [type, size] of ICNS_TYPES) {
    const hit = pngs.find((p) => p.size === size)
    if (!hit) continue
    const head = Buffer.alloc(8)
    head.write(type, 0, 'latin1')
    head.writeUInt32BE(8 + hit.png.length, 4)
    parts.push(head, hit.png)
    total += 8 + hit.png.length
  }
  const head = Buffer.alloc(8)
  head.write('icns', 0, 'latin1')
  head.writeUInt32BE(total, 4)
  return Buffer.concat([head, ...parts])
}

/* ---------------- output ---------------- */

const PNG_NAMES = {
  16: '16x16.png', 20: '20x20.png', 24: '24x24.png', 32: '32x32.png', 40: '40x40.png',
  48: '48x48.png', 64: '64x64.png', 128: '128x128.png', 256: '128x128@2x.png',
  512: 'icon.png', 1024: 'source.png',
}

function build(name, outDir) {
  const treatment = TREATMENTS[name]
  if (!treatment) throw new Error('unknown treatment: ' + name)
  mkdirSync(outDir, { recursive: true })
  const pngs = SIZES.map((size) => ({ size, png: encodePng(compose(treatment, size), size, size) }))
  for (const p of pngs) {
    const file = PNG_NAMES[p.size]
    if (file) writeFileSync(resolve(outDir, file), p.png)
  }
  writeFileSync(resolve(outDir, '256x256.png'), pngs.find((p) => p.size === 256).png)
  writeFileSync(resolve(outDir, '512x512.png'), pngs.find((p) => p.size === 512).png)
  writeFileSync(resolve(outDir, 'icon.ico'), buildIco(pngs.filter((p) => ICO_SIZES.includes(p.size))))
  writeFileSync(resolve(outDir, 'icon.icns'), buildIcns(pngs))
  return pngs
}

/** Contact sheet of the real output sizes, so the result can be judged at a glance. */
function contact(pngs, outDir, bg, file) {
  const tiles = [1024, 256, 128, 64, 48, 32, 24, 16]
  const gap = 24, margin = 24
  const W = margin * 2 + tiles.reduce((s, t) => s + Math.min(t, 256), 0) + gap * (tiles.length - 1)
  const H = margin * 2 + 256
  const buf = new Uint8Array(W * H * 4)
  for (let i = 0; i < W * H; i++) { buf[i * 4] = bg[0]; buf[i * 4 + 1] = bg[1]; buf[i * 4 + 2] = bg[2]; buf[i * 4 + 3] = 255 }
  let x = margin
  for (const t of tiles) {
    const show = Math.min(t, 256)
    const p = pngs.find((q) => q.size === t)
    const px = decodePng(Buffer.from(p.png))
    const scaled = t === show ? px.data : resample(px.data, t, t, show, show)
    over(buf, W, H, scaled, show, show, x, margin + (256 - show) / 2)
    x += show + gap
  }
  writeFileSync(resolve(outDir, file), encodePng(buf, W, H))
}

/* --mark=<path>: emit only the stencil (no shell, no negative space), used as
 * a CSS mask by src/components/layout/Logo.tsx so the in-app mark inherits the
 * accent colour. */
if (flag('mark')) {
  const file = resolve(ROOT, arg('mark', 'src/assets/phl-mark.png'))
  const w = 512
  const h = Math.round((master.height / master.width) * w)
  const px = resample(master.data, master.width, master.height, w, h)
  for (let i = 0; i < w * h; i++) { px[i * 4] = 255; px[i * 4 + 1] = 255; px[i * 4 + 2] = 255 }
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, encodePng(px, w, h))
  console.log('mark -> ' + file + ' (' + w + 'x' + h + ')')
} else {

const outRoot = resolve(ROOT, arg('out', 'src-tauri/icons'))
if (flag('all')) {
  for (const name of Object.keys(TREATMENTS)) {
    const dir = resolve(outRoot, name)
    const pngs = build(name, dir)
    contact(pngs, dir, [244, 246, 250], 'contact-light.png')
    contact(pngs, dir, [22, 24, 30], 'contact-dark.png')
    console.log(name + ' -> ' + dir)
  }
} else {
  const name = arg('treatment', DEFAULT_TREATMENT)
  const pngs = build(name, outRoot)
  contact(pngs, outRoot, [244, 246, 250], 'contact-light.png')
  console.log(name + ' -> ' + outRoot)
}
}