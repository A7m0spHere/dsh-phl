import {
  forwardRef,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from 'react'
import { motion } from 'motion/react'
import { Check, ChevronDown } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

/* ---------------- layout ---------------- */

export function Field({
  label,
  hint,
  error,
  children,
  required,
  className,
}: {
  label?: ReactNode
  hint?: ReactNode
  error?: ReactNode
  children: ReactNode
  required?: boolean
  className?: string
}) {
  return (
    <label className={cn('block', className)}>
      {label && (
        <span className="mb-1.5 flex items-center gap-1 text-sm font-medium text-ink-muted">
          {label}
          {required && <span className="text-danger">*</span>}
        </span>
      )}
      {children}
      {(hint || error) && (
        <span
          className={cn('mt-1.5 block text-sm', error ? 'text-danger' : 'text-ink-faint')}
        >
          {error || hint}
        </span>
      )}
    </label>
  )
}

/** A settings row: title + description on the left, control on the right. */
export function SettingRow({
  title,
  description,
  control,
  className,
}: {
  title: ReactNode
  description?: ReactNode
  control: ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        'flex items-center gap-6 border-b border-line py-2 last:border-b-0',
        className,
      )}
    >
      <div className="min-w-0 flex-1">
        <div className="text-base text-ink">{title}</div>
        {description && <div className="mt-0.5 text-sm text-ink-faint">{description}</div>}
      </div>
      <div className="shrink-0">{control}</div>
    </div>
  )
}

/* ---------------- text ---------------- */

// Placeholders keep the full-strength token: any alpha over the surface drops
// them well below the 4.5:1 the token itself is tuned for.
const inputBase =
  'w-full rounded-sm bg-surface px-2 text-base text-ink ring-1 ring-inset ring-line-strong/60 placeholder:text-ink-faint transition-[box-shadow,background-color] duration-150 ease-out hover:ring-line-strong focus:outline-none focus:ring-[1.5px] focus:ring-accent disabled:opacity-50'

export interface InputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'prefix'> {
  invalid?: boolean
  prefix?: ReactNode
  suffix?: ReactNode
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, invalid, prefix, suffix, ...rest },
  ref,
) {
  if (prefix || suffix) {
    return (
      <div
        className={cn(
          'flex h-7 items-center gap-1.5 rounded-sm bg-surface px-2 ring-1 ring-inset ring-line-strong/60 transition-shadow duration-150 focus-within:ring-[1.5px] focus-within:ring-accent hover:ring-line-strong',
          invalid && 'ring-danger focus-within:ring-danger',
          rest.disabled && 'pointer-events-none opacity-50',
          className,
        )}
      >
        {prefix && <span className="shrink-0 text-sm text-ink-faint">{prefix}</span>}
        <input
          ref={ref}
          className="min-w-0 flex-1 bg-transparent text-base text-ink outline-none placeholder:text-ink-faint"
          {...rest}
        />
        {suffix && <span className="shrink-0 text-sm text-ink-faint">{suffix}</span>}
      </div>
    )
  }
  return (
    <input
      ref={ref}
      className={cn(inputBase, 'h-7', invalid && 'ring-danger focus:ring-danger', className)}
      {...rest}
    />
  )
})

export const TextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function TextArea({ className, ...rest }, ref) {
    return <textarea ref={ref} className={cn(inputBase, 'py-2 leading-5', className)} {...rest} />
  },
)

/* ---------------- select ---------------- */

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  children: ReactNode
}

export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select(
  { className, children, ...rest },
  ref,
) {
  return (
    <div className="relative">
      <select
        ref={ref}
        className={cn(inputBase, 'h-7 cursor-pointer appearance-none pr-6', className)}
        {...rest}
      >
        {children}
      </select>
      <ChevronDown
        size={13}
        className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-ink-faint"
      />
    </div>
  )
})

/* ---------------- switch ---------------- */

export function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (v: boolean) => void
  disabled?: boolean
  label?: string
}) {
  const { spring } = useMotion()
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-[20px] w-[34px] shrink-0 items-center rounded-full px-[2px] transition-colors duration-200 ease-out disabled:opacity-45',
        checked ? 'bg-accent' : 'bg-ink/20',
      )}
    >
      <motion.span
        initial={false}
        animate={{ x: checked ? 14 : 0 }}
        transition={spring}
        className="h-4 w-4 rounded-full bg-white shadow-sm"
      />
    </button>
  )
}

/* ---------------- checkbox ---------------- */

export function Checkbox({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean
  onChange: (v: boolean) => void
  label?: ReactNode
  disabled?: boolean
}) {
  const { t } = useMotion()
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="inline-flex items-center gap-2 text-base text-ink disabled:opacity-45"
    >
      <span
        className={cn(
          'flex h-[15px] w-[15px] items-center justify-center rounded-xs ring-1 ring-inset transition-colors duration-150',
          checked ? 'bg-accent ring-accent' : 'bg-surface ring-line-strong',
        )}
      >
        <motion.span
          initial={false}
          animate={{ scale: checked ? 1 : 0, opacity: checked ? 1 : 0 }}
          transition={t(0.16)}
          className="text-[hsl(var(--c-accent-on))]"
        >
          <Check size={11} strokeWidth={3} />
        </motion.span>
      </span>
      {label}
    </button>
  )
}
