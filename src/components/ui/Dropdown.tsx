import { useEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, ChevronDown } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

export interface DropdownOption<T extends string = string> {
  value: T
  label: ReactNode
}

/**
 * A styled dropdown that keeps the popover inside PHL's design language —
 * the native `<select>` list renders with OS chrome (grey highlight, square
 * corners) and reads as broken next to the rest of the UI. (The native
 * `Field.Select` remains for forms that want platform behaviour.)
 *
 * The list flips above the trigger when there is no room below, closes on
 * Escape / outside click, and supports arrow-key navigation.
 */
export function Dropdown<T extends string>({
  value,
  options,
  onChange,
  className,
  listClassName,
  align = 'start',
  placeholder,
  ariaLabel,
}: {
  value: T
  options: DropdownOption<T>[]
  onChange: (value: T) => void
  className?: string
  listClassName?: string
  align?: 'start' | 'end'
  placeholder?: string
  ariaLabel?: string
}) {
  const [open, setOpen] = useState(false)
  const [dropUp, setDropUp] = useState(false)
  const [active, setActive] = useState(0)
  const ref = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const { t, scale } = useMotion()

  const current = options.find((o) => o.value === value)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  // Keep the highlighted option in view while arrowing through the list.
  useEffect(() => {
    if (!open) return
    listRef.current
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: 'nearest' })
  }, [active, open])

  const toggle = () => {
    if (!open) {
      // Flip above the trigger when the list would run off the window.
      const rect = ref.current?.getBoundingClientRect()
      const listH = Math.min(options.length * 34 + 8, 288)
      setDropUp(!!rect && rect.bottom + listH > window.innerHeight && rect.top > listH)
      setActive(Math.max(0, options.findIndex((o) => o.value === value)))
    }
    setOpen((v) => !v)
  }

  const commit = (index: number) => {
    const option = options[index]
    if (!option) return
    onChange(option.value)
    setOpen(false)
  }

  const onTriggerKey = (e: React.KeyboardEvent) => {
    if (open) {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setActive((i) => Math.min(options.length - 1, i + 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setActive((i) => Math.max(0, i - 1))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        commit(active)
      }
    } else if (e.key === 'ArrowDown' || e.key === 'Enter') {
      e.preventDefault()
      toggle()
    }
  }

  return (
    <div ref={ref} className={cn('relative', className)}>
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={toggle}
        onKeyDown={onTriggerKey}
        className={cn(
          'flex h-[34px] w-full min-w-0 items-center justify-between gap-1.5 rounded bg-surface px-2.5 text-left text-base text-ink ring-1 ring-inset transition-colors duration-150',
          open ? 'ring-accent-ink/40' : 'ring-line hover:ring-line-strong/60',
        )}
      >
        <span className={cn('min-w-0 flex-1 truncate', !current && 'text-ink-faint')}>
          {current?.label ?? placeholder ?? '—'}
        </span>
        <ChevronDown
          size={14}
          className={cn('shrink-0 text-ink-faint transition-transform duration-200', open && 'rotate-180')}
        />
      </button>
      <AnimatePresence>
        {open && (
          <motion.div
            ref={listRef}
            role="listbox"
            initial={{ opacity: 0, y: scale === 0 ? 0 : dropUp ? 4 : -4, scale: scale === 0 ? 1 : 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: scale === 0 ? 0 : dropUp ? 2 : -2, scale: scale === 0 ? 1 : 0.98 }}
            transition={t(0.14)}
            style={{ transformOrigin: dropUp ? 'bottom left' : 'top left' }}
            className={cn(
              'absolute z-50 max-h-72 min-w-full overflow-y-auto overscroll-contain rounded-lg bg-surface-raised p-1 shadow-pop ring-1 ring-inset ring-line',
              dropUp ? 'bottom-full mb-1.5' : 'top-full mt-1.5',
              align === 'end' ? 'right-0' : 'left-0',
              listClassName,
            )}
          >
            {options.map((option, index) => {
              const selected = option.value === value
              return (
                <button
                  key={option.value}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  data-index={index}
                  onClick={() => commit(index)}
                  onMouseEnter={() => setActive(index)}
                  className={cn(
                    'flex w-full items-center gap-2 whitespace-nowrap rounded-sm px-2 py-[6px] text-left text-base transition-colors duration-100',
                    selected ? 'text-accent-ink' : 'text-ink',
                    index === active && !selected && 'bg-surface-hover',
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{option.label}</span>
                  {selected && <Check size={13} className="shrink-0 text-accent-ink" />}
                </button>
              )
            })}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
