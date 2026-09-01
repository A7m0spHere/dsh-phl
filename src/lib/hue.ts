/**
 * Every instance carries a stable identity colour so it stays recognisable
 * across the list, the detail header, the launch dock and toasts. The palette
 * is deliberately desaturated — it marks identity, it doesn't decorate.
 */
export const HUES = [
  { id: 0, label: '钢蓝', h: 219, s: 72 },
  { id: 1, label: '紫罗兰', h: 258, s: 62 },
  { id: 2, label: '青', h: 184, s: 58 },
  { id: 3, label: '苔绿', h: 150, s: 44 },
  { id: 4, label: '琥珀', h: 34, s: 74 },
  { id: 5, label: '绯红', h: 344, s: 64 },
] as const

export interface HueTone {
  /** Fill for the identity tile. */
  soft: string
  /** Saturated line/dot colour. */
  solid: string
  /** Text on top of `soft`. */
  text: string
  /** Hairline border. */
  ring: string
  /** Ambient glow used behind the running state. */
  glow: string
}

export function hueTone(hue: number, dark: boolean): HueTone {
  const { h, s } = HUES[((hue % HUES.length) + HUES.length) % HUES.length]
  return dark
    ? {
        soft: `hsl(${h} ${s}% 60% / 0.16)`,
        solid: `hsl(${h} ${s}% 62%)`,
        text: `hsl(${h} ${Math.min(s + 12, 90)}% 76%)`,
        ring: `hsl(${h} ${s}% 60% / 0.30)`,
        glow: `hsl(${h} ${s}% 55% / 0.22)`,
      }
    : {
        soft: `hsl(${h} ${s}% 52% / 0.11)`,
        solid: `hsl(${h} ${s}% 48%)`,
        text: `hsl(${h} ${Math.min(s + 6, 88)}% 34%)`,
        ring: `hsl(${h} ${s}% 48% / 0.22)`,
        glow: `hsl(${h} ${s}% 50% / 0.18)`,
      }
}

/** Two-letter mark derived from the instance name, used in the identity tile. */
export function initials(name: string): string {
  const trimmed = name.trim()
  if (!trimmed) return '··'
  if (/^[一-龥]/.test(trimmed)) return trimmed.slice(0, 2)
  const words = trimmed.split(/[\s_-]+/).filter(Boolean)
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase()
  return (words[0][0] + words[1][0]).toUpperCase()
}
