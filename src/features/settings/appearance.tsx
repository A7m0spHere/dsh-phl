/** Theme accent + the per-instance colour legend (settings → 通用). */
import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { ACCENTS, useInstanceStore, useIsDark, useUIStore, type Accent } from '@/stores'

export function AccentPicker() {
  const accent = useUIStore((s) => s.accent)
  const setAccent = useUIStore((s) => s.setAccent)
  const dark = useIsDark()
  const { spring } = useMotion()

  return (
    <div className="flex gap-1.5">
      {ACCENTS.map((a) => (
        <button
          key={a.id}
          title={a.label}
          onClick={() => setAccent(a.id as Accent)}
          className="relative flex h-7 w-7 items-center justify-center rounded-lg transition-transform duration-150 hover:scale-105"
        >
          {/* the swatch re-declares the accent tokens locally, so each chip
              paints itself in the palette it would apply */}
          <span
            className={cn('h-5 w-5 rounded-md', dark && 'dark')}
            data-accent={a.id}
            style={{ background: 'hsl(var(--c-accent))' }}
          />
          {accent === a.id && (
            <motion.span
              layoutId="accent-ring"
              transition={spring}
              className="absolute inset-0 rounded-lg ring-2 ring-accent ring-offset-1 ring-offset-canvas"
            />
          )}
        </button>
      ))}
    </div>
  )
}

export function InstanceColourLegend() {
  const instances = useInstanceStore((s) => s.instances)
  const dark = useIsDark()
  return (
    <div className="flex flex-wrap gap-1.5">
      {instances.slice(0, 6).map((i) => {
        const tone = hueTone(i.hue, dark)
        return (
          <span
            key={i.id}
            className="inline-flex items-center gap-1.5 rounded-full px-2 py-1 text-2xs"
            style={{ background: tone.soft, color: tone.text }}
          >
            <span className="h-[6px] w-[6px] rounded-full" style={{ background: tone.solid }} />
            {i.name}
          </span>
        )
      })}
    </div>
  )
}

/* ------------------------------------------------------------------ */
