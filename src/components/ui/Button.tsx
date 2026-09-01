import { forwardRef, type ReactNode } from 'react'
import { motion, type HTMLMotionProps } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotionScale } from '@/lib/motion'
import { Spinner } from './Spinner'

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'quiet'
export type ButtonSize = 'xs' | 'sm' | 'md' | 'lg' | 'hero'

const VARIANTS: Record<ButtonVariant, string> = {
  primary:
    'bg-accent text-[hsl(var(--c-accent-on))] shadow-accent hover:bg-accent-hover active:bg-accent-press',
  secondary:
    'bg-surface text-ink ring-1 ring-inset ring-line-strong/70 hover:bg-surface-hover hover:ring-line-strong active:bg-surface-sunken',
  ghost: 'text-ink-muted hover:text-ink hover:bg-surface-hover active:bg-surface-sunken',
  quiet:
    'bg-accent-soft text-accent-ink hover:bg-accent-soft/70 dark:hover:bg-accent-soft/80 active:bg-accent-soft',
  danger:
    'bg-surface text-danger ring-1 ring-inset ring-danger/25 hover:bg-danger hover:text-white hover:ring-danger active:bg-danger/90',
}

const SIZES: Record<ButtonSize, string> = {
  xs: 'h-[20px] px-1.5 text-2xs gap-1 rounded-xs',
  sm: 'h-6 px-2 text-xs gap-1 rounded-sm',
  md: 'h-7 px-2.5 text-sm gap-1.5 rounded-sm',
  lg: 'h-8 px-3 text-base gap-1.5 rounded',
  hero: 'h-9 px-5 text-base font-medium gap-2 rounded-lg',
}

export interface ButtonProps extends Omit<HTMLMotionProps<'button'>, 'ref' | 'children'> {
  variant?: ButtonVariant
  size?: ButtonSize
  loading?: boolean
  block?: boolean
  /** Adds the moving sheen used on the primary launch action. */
  sheen?: boolean
  children?: ReactNode
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = 'secondary',
    size = 'md',
    loading = false,
    block = false,
    sheen = false,
    className,
    children,
    disabled,
    ...rest
  },
  ref,
) {
  const scale = useMotionScale()
  const isDisabled = disabled || loading

  return (
    <motion.button
      ref={ref}
      type="button"
      disabled={isDisabled}
      whileTap={scale === 0 || isDisabled ? undefined : { scale: size === 'hero' ? 0.98 : 0.97 }}
      transition={{ duration: 0.09 }}
      className={cn(
        'relative inline-flex select-none items-center justify-center overflow-hidden whitespace-nowrap font-medium',
        'transition-[background-color,color,box-shadow] duration-150 ease-out',
        'disabled:pointer-events-none disabled:opacity-45',
        VARIANTS[variant],
        SIZES[size],
        block && 'w-full',
        className,
      )}
      {...rest}
    >
      {sheen && !isDisabled && (
        <span
          aria-hidden
          className="pointer-events-none absolute inset-0 -translate-x-full bg-gradient-to-r from-transparent via-white/22 to-transparent transition-transform duration-[650ms] ease-out group-hover/sheen:translate-x-full"
        />
      )}
      {loading && <Spinner size={size === 'hero' ? 16 : 13} className="shrink-0" />}
      {children}
    </motion.button>
  )
})

export interface IconButtonProps extends ButtonProps {
  label: string
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, size = 'md', className, ...rest },
  ref,
) {
  const box =
    size === 'xs'
      ? 'h-[20px] w-[20px]'
      : size === 'sm'
        ? 'h-6 w-6'
        : size === 'lg'
          ? 'h-8 w-8'
          : size === 'hero'
            ? 'h-9 w-9'
            : 'h-7 w-7'
  return (
    <Button
      ref={ref}
      aria-label={label}
      title={label}
      size={size}
      className={cn('!px-0', box, className)}
      {...rest}
    />
  )
})
