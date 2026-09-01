import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { AlertTriangle } from 'lucide-react'
import { useMotion } from '@/lib/motion'
import { useUIStore } from '@/stores/uiStore'
import { Button } from './Button'
import { Field, Input } from './Field'

export function DialogHost() {
  const dialog = useUIStore((s) => s.dialog)
  const close = useUIStore((s) => s.closeDialog)
  const { t, pop, overlay } = useMotion()

  const [value, setValue] = useState('')
  const [typed, setTyped] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (!dialog) return
    setValue(dialog.kind === 'prompt' ? (dialog.spec.defaultValue ?? '') : '')
    setTyped('')
    const id = window.setTimeout(() => {
      inputRef.current?.focus()
      inputRef.current?.select()
    }, 60)
    return () => window.clearTimeout(id)
  }, [dialog])

  useEffect(() => {
    if (!dialog) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close(dialog.kind === 'confirm' ? false : null)
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [dialog, close])

  const promptError =
    dialog?.kind === 'prompt' ? (dialog.spec.validate?.(value) ?? null) : null
  const confirmLocked =
    dialog?.kind === 'confirm' && dialog.spec.typeToConfirm
      ? typed.trim() !== dialog.spec.typeToConfirm
      : false

  const submit = () => {
    if (!dialog) return
    if (dialog.kind === 'prompt') {
      if (promptError || !value.trim()) return
      close(value.trim())
    } else {
      if (confirmLocked) return
      close(true)
    }
  }

  return (
    <AnimatePresence>
      {dialog && (
        <motion.div key="dialog" className="fixed inset-0 z-[80] flex items-center justify-center p-8">
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            onClick={() => close(dialog.kind === 'confirm' ? false : null)}
            className="absolute inset-0 bg-canvas/55 backdrop-blur-[2px]"
          />
          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            transition={t(0.22)}
            role="dialog"
            aria-modal
            className="relative w-[380px] overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="px-4 pb-3 pt-3.5">
              <div className="flex items-start gap-3">
                {dialog.kind === 'confirm' && dialog.spec.tone === 'danger' && (
                  <span className="mt-px flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-danger/10 text-danger">
                    <AlertTriangle size={15} />
                  </span>
                )}
                <div className="min-w-0 flex-1">
                  <h2 className="text-base font-medium text-ink">{dialog.spec.title}</h2>
                  {dialog.kind === 'confirm' && (
                    <p className="mt-1 text-sm leading-relaxed text-ink-muted">
                      {dialog.spec.message}
                    </p>
                  )}
                  {dialog.kind === 'confirm' && dialog.spec.detail && (
                    <p className="mt-2 whitespace-pre-line rounded bg-surface-sunken px-2.5 py-2 font-mono text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
                      {dialog.spec.detail}
                    </p>
                  )}
                </div>
              </div>

              {dialog.kind === 'prompt' && (
                <div className="mt-3.5">
                  <Field label={dialog.spec.label} hint={dialog.spec.helper} error={promptError}>
                    <Input
                      ref={inputRef}
                      value={value}
                      placeholder={dialog.spec.placeholder}
                      invalid={!!promptError}
                      onChange={(e) => setValue(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && submit()}
                    />
                  </Field>
                </div>
              )}

              {dialog.kind === 'confirm' && dialog.spec.typeToConfirm && (
                <div className="mt-3.5">
                  <Field
                    label={
                      <span>
                        输入 <code className="font-mono text-ink">{dialog.spec.typeToConfirm}</code>{' '}
                        以确认
                      </span>
                    }
                  >
                    <Input
                      ref={inputRef}
                      value={typed}
                      onChange={(e) => setTyped(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && submit()}
                    />
                  </Field>
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 border-t border-line bg-surface-sunken/60 px-4 py-2.5">
              <Button
                variant="ghost"
                onClick={() => close(dialog.kind === 'confirm' ? false : null)}
              >
                {(dialog.kind === 'confirm' && dialog.spec.cancelLabel) || '取消'}
              </Button>
              <Button
                variant={
                  dialog.kind === 'confirm' && dialog.spec.tone === 'danger' ? 'danger' : 'primary'
                }
                disabled={
                  confirmLocked || (dialog.kind === 'prompt' && (!!promptError || !value.trim()))
                }
                onClick={submit}
              >
                {dialog.kind === 'confirm'
                  ? (dialog.spec.confirmLabel ?? '确定')
                  : (dialog.spec.confirmLabel ?? '确定')}
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
