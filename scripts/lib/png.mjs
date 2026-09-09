/**
 * Minimal PNG codec + raster helpers for the icon pipeline.
 *
 * Zero dependencies: node:zlib covers inflate/deflate, everything else is
 * hand-rolled. Supports 8-bit non-interlaced greyscale/RGB/RGBA PNGs, which
 * is what every icon asset in this repository is.
 */
import { inflateSync, deflateSync } from 'node:zlib'
import { readFileSync } from 'node:fs'

const CRC_TABLE = (() => {
  const t = new Int32Array(256)
  for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; t[n] = c }
  return t
})()
const crc32 = (b) => { let c = -1; for (let i = 0; i < b.length; i++) c = CRC_TABLE[(c ^ b[i]) & 0xff] ^ (c >>> 8); return (c ^ -1) >>> 0 }
const chunk = (type, data) => {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data])
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(body))
  return Buffer.concat([len, body, crc])
}

/** Encode straight-alpha RGBA pixels as a PNG buffer. */
export function encodePng(px, w, h) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 6
  const stride = w * 4
  const raw = Buffer.alloc((stride + 1) * h)
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0
    Buffer.from(px.buffer, px.byteOffset + y * stride, stride).copy(raw, y * (stride + 1) + 1)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

const paeth = (a, b, c) => {
  const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c)
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c
}

/** Decode an 8-bit non-interlaced PNG into a straight-alpha RGBA buffer. */
export function decodePng(pathOrBuffer) {
  const buf = typeof pathOrBuffer === 'string' ? readFileSync(pathOrBuffer) : pathOrBuffer
  let off = 8, w = 0, h = 0, depth = 8, color = 6, interlace = 0
  const idat = []
  while (off < buf.length) {
    const len = buf.readUInt32BE(off)
    const type = buf.toString('latin1', off + 4, off + 8)
    const data = buf.subarray(off + 8, off + 8 + len)
    if (type === 'IHDR') { w = data.readUInt32BE(0); h = data.readUInt32BE(4); depth = data[8]; color = data[9]; interlace = data[12] }
    else if (type === 'IDAT') idat.push(data)
    else if (type === 'IEND') break
    off += 12 + len
  }
  if (depth !== 8) throw new Error('unsupported bit depth ' + depth)
  if (interlace !== 0) throw new Error('interlaced PNG unsupported')
  const chan = color === 6 ? 4 : color === 2 ? 3 : color === 0 ? 1 : color === 4 ? 2 : null
  if (!chan) throw new Error('unsupported colour type ' + color)
  const raw = inflateSync(Buffer.concat(idat))
  const stride = w * chan
  const out = new Uint8Array(w * h * 4)
  const prev = new Uint8Array(stride)
  const line = new Uint8Array(stride)
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)]
    raw.copy(line, 0, y * (stride + 1) + 1, y * (stride + 1) + 1 + stride)
    for (let x = 0; x < stride; x++) {
      const a = x >= chan ? line[x - chan] : 0
      const b = prev[x]
      const c = x >= chan ? prev[x - chan] : 0
      if (f === 1) line[x] = (line[x] + a) & 255
      else if (f === 2) line[x] = (line[x] + b) & 255
      else if (f === 3) line[x] = (line[x] + ((a + b) >> 1)) & 255
      else if (f === 4) line[x] = (line[x] + paeth(a, b, c)) & 255
    }
    for (let x = 0; x < w; x++) {
      const s = x * chan, d = (y * w + x) * 4
      if (chan === 4) { out[d] = line[s]; out[d + 1] = line[s + 1]; out[d + 2] = line[s + 2]; out[d + 3] = line[s + 3] }
      else if (chan === 3) { out[d] = line[s]; out[d + 1] = line[s + 1]; out[d + 2] = line[s + 2]; out[d + 3] = 255 }
      else if (chan === 2) { out[d] = out[d + 1] = out[d + 2] = line[s]; out[d + 3] = line[s + 1] }
      else { out[d] = out[d + 1] = out[d + 2] = line[s]; out[d + 3] = 255 }
    }
    prev.set(line)
  }
  return { width: w, height: h, data: out }
}

/** Box-filter resample (alpha-aware). Good enough for icon scaling. */
export function resample(src, sw, sh, dw, dh) {
  const out = new Uint8Array(dw * dh * 4)
  for (let y = 0; y < dh; y++) {
    const y0 = (y * sh) / dh, y1 = ((y + 1) * sh) / dh
    for (let x = 0; x < dw; x++) {
      const x0 = (x * sw) / dw, x1 = ((x + 1) * sw) / dw
      let r = 0, g = 0, b = 0, a = 0, n = 0
      for (let sy = Math.floor(y0); sy < Math.ceil(y1); sy++) {
        for (let sx = Math.floor(x0); sx < Math.ceil(x1); sx++) {
          const i = (Math.min(sy, sh - 1) * sw + Math.min(sx, sw - 1)) * 4
          const w2 = src[i + 3] / 255
          r += src[i] * w2; g += src[i + 1] * w2; b += src[i + 2] * w2; a += w2; n++
        }
      }
      const d = (y * dw + x) * 4
      if (a > 0) {
        out[d] = Math.round(r / a); out[d + 1] = Math.round(g / a); out[d + 2] = Math.round(b / a)
        out[d + 3] = Math.round((a / n) * 255)
      }
    }
  }
  return out
}

/** Max-filter the alpha channel: thickens a stencil so it survives 16px. */
export function dilateAlpha(px, w, h, radius) {
  if (radius <= 0) return px
  const out = new Uint8Array(px.length)
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let m = 0
      for (let dy = -radius; dy <= radius; dy++) {
        const ny = y + dy; if (ny < 0 || ny >= h) continue
        for (let dx = -radius; dx <= radius; dx++) {
          const nx = x + dx; if (nx < 0 || nx >= w) continue
          const a = px[(ny * w + nx) * 4 + 3]
          if (a > m) m = a
        }
      }
      const d = (y * w + x) * 4
      out[d] = px[d]; out[d + 1] = px[d + 1]; out[d + 2] = px[d + 2]; out[d + 3] = m
    }
  }
  return out
}

/** Min-filter the alpha channel — the erosion half of a morphological open. */
export function erodeAlpha(px, w, h, radius) {
  if (radius <= 0) return px
  const out = new Uint8Array(px.length)
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let m = 255
      for (let dy = -radius; dy <= radius; dy++) {
        const ny = y + dy
        for (let dx = -radius; dx <= radius; dx++) {
          const nx = x + dx
          const a = ny < 0 || ny >= h || nx < 0 || nx >= w ? 0 : px[(ny * w + nx) * 4 + 3]
          if (a < m) m = a
        }
      }
      const d = (y * w + x) * 4
      out[d] = px[d]; out[d + 1] = px[d + 1]; out[d + 2] = px[d + 2]; out[d + 3] = m
    }
  }
  return out
}

/**
 * Squeeze the alpha ramp into [lo, hi] (smoothstepped), so an edge is either
 * covered or not instead of trailing a wide halo of half-transparent pixels.
 * This is what makes a downscaled organic shape read sharp at icon sizes.
 */
export function contrastAlpha(px, w, h, lo, hi) {
  const out = new Uint8Array(px)
  for (let i = 0; i < w * h; i++) {
    const a = out[i * 4 + 3] / 255
    let t = (a - lo) / (hi - lo)
    t = t < 0 ? 0 : t > 1 ? 1 : t
    out[i * 4 + 3] = Math.round(255 * t * t * (3 - 2 * t))
  }
  return out
}

/** 3x3 median filter on alpha: kills single-pixel spikes without moving edges. */
export function medianAlpha(px, w, h) {
  const out = new Uint8Array(px)
  const v = new Uint8Array(9)
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let n = 0
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const ny = y + dy, nx = x + dx
          v[n++] = ny < 0 || ny >= h || nx < 0 || nx >= w ? 0 : px[(ny * w + nx) * 4 + 3]
        }
      }
      for (let i = 1; i < 9; i++) {
        const key = v[i]
        let j = i - 1
        while (j >= 0 && v[j] > key) { v[j + 1] = v[j]; j-- }
        v[j + 1] = key
      }
      out[(y * w + x) * 4 + 3] = v[4]
    }
  }
  return out
}

/** 3x3 box blur on alpha: the one-pixel anti-aliasing ring a hard edge needs. */
export function blurAlpha(px, w, h) {
  const out = new Uint8Array(px)
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let s = 0, n = 0
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const ny = y + dy, nx = x + dx
          if (ny < 0 || ny >= h || nx < 0 || nx >= w) continue
          s += px[(ny * w + nx) * 4 + 3]
          n++
        }
      }
      out[(y * w + x) * 4 + 3] = Math.round(s / n)
    }
  }
  return out
}

/** Source-over compositing of one RGBA tile onto another. */
export function over(dst, dw, dh, src, sw, sh, ox, oy) {
  for (let y = 0; y < sh; y++) {
    const ty = oy + y; if (ty < 0 || ty >= dh) continue
    for (let x = 0; x < sw; x++) {
      const tx = ox + x; if (tx < 0 || tx >= dw) continue
      const s = (y * sw + x) * 4, d = (ty * dw + tx) * 4
      const sa = src[s + 3] / 255, da = dst[d + 3] / 255
      const oa = sa + da * (1 - sa)
      if (oa <= 0) { dst[d] = 0; dst[d + 1] = 0; dst[d + 2] = 0; dst[d + 3] = 0; continue }
      dst[d] = Math.round((src[s] * sa + dst[d] * da * (1 - sa)) / oa)
      dst[d + 1] = Math.round((src[s + 1] * sa + dst[d + 1] * da * (1 - sa)) / oa)
      dst[d + 2] = Math.round((src[s + 2] * sa + dst[d + 2] * da * (1 - sa)) / oa)
      dst[d + 3] = Math.round(oa * 255)
    }
  }
}