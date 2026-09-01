/**
 * Renders PHL's app icon to a 1024×1024 PNG with no image dependencies.
 *
 * The geometry is the same mark used by `src/components/layout/Logo.tsx`:
 * a rounded square holding three offset bars — several isolated environments
 * stacked in one container. Generating it here (rather than committing a
 * binary) keeps the icon in sync with the in-app logo and keeps every asset
 * in this repository independently authored.
 *
 *   node scripts/make-icon.mjs
 *   npx tauri icon src-tauri/icons/source.png
 */
import { deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const OUT = resolve(dirname(fileURLToPath(import.meta.url)), '../src-tauri/icons/source.png')

const SIZE = 1024
/** Samples per axis. 4×4 = 16 samples per pixel, enough for clean curves. */
const SS = 4
/** The logo is authored on a 24-unit grid, like the SVG. */
const GRID = 24

// Accent azure — hsl(219 84% 55%) resolved to sRGB.
const ACCENT = [44, 111, 237]
const WHITE = [255, 255, 255]

/** Rounded rectangle in grid units: [x, y, w, h, r, colour, alpha]. */
const SHAPES = [
  [1, 1, 22, 22, 5.6, ACCENT, 1],
  [6, 6.5, 12, 3, 1.5, WHITE, 0.95],
  [6, 11, 8.5, 3, 1.5, WHITE, 0.7],
  [6, 15.5, 5, 3, 1.5, WHITE, 0.45],
]

/** Signed containment test: clamp to the inner rect, then check the corner radius. */
function inside(px, py, [x, y, w, h, r]) {
  const cx = Math.min(Math.max(px, x + r), x + w - r)
  const cy = Math.min(Math.max(py, y + r), y + h - r)
  const dx = px - cx
  const dy = py - cy
  return dx * dx + dy * dy <= r * r
}

function render() {
  const pixels = new Uint8Array(SIZE * SIZE * 4)
  const scale = GRID / SIZE
  const step = 1 / SS
  const samples = SS * SS

  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      // Accumulate premultiplied colour across the supersample grid.
      let r = 0
      let g = 0
      let b = 0
      let a = 0

      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const px = (x + (sx + 0.5) * step) * scale
          const py = (y + (sy + 0.5) * step) * scale

          // Painter's algorithm, one sample at a time.
          let cr = 0
          let cg = 0
          let cb = 0
          let ca = 0
          for (const shape of SHAPES) {
            if (!inside(px, py, shape)) continue
            const [, , , , , colour, alpha] = shape
            cr = colour[0] * alpha + cr * (1 - alpha)
            cg = colour[1] * alpha + cg * (1 - alpha)
            cb = colour[2] * alpha + cb * (1 - alpha)
            ca = alpha + ca * (1 - alpha)
          }
          r += cr * ca
          g += cg * ca
          b += cb * ca
          a += ca
        }
      }

      const alpha = a / samples
      const i = (y * SIZE + x) * 4
      if (alpha > 0) {
        // Un-premultiply back to straight alpha for the PNG.
        pixels[i] = Math.round(r / a)
        pixels[i + 1] = Math.round(g / a)
        pixels[i + 2] = Math.round(b / a)
        pixels[i + 3] = Math.round(alpha * 255)
      }
    }
  }
  return pixels
}

/* ---------------- minimal PNG encoder ---------------- */

const CRC_TABLE = (() => {
  const table = new Int32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    table[n] = c
  }
  return table
})()

function crc32(buf) {
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

function encodePng(pixels, size) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // colour type: RGBA
  ihdr[10] = 0 // deflate
  ihdr[11] = 0 // adaptive filtering
  ihdr[12] = 0 // no interlace

  // One filter byte (0 = none) per scanline.
  const stride = size * 4
  const raw = Buffer.alloc((stride + 1) * size)
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0
    Buffer.from(pixels.buffer, y * stride, stride).copy(raw, y * (stride + 1) + 1)
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

mkdirSync(dirname(OUT), { recursive: true })
writeFileSync(OUT, encodePng(render(), SIZE))
console.log(`wrote ${OUT} (${SIZE}×${SIZE})`)
