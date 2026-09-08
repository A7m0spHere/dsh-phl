import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { AlertTriangle } from 'lucide-react'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { useUIStore } from '@/stores/uiStore'
import { Button } from './Button'
import { Field, Input } from './Field'

export function DialogHost() {
  const dialog = useUIStore((s) => s.dialog)
  const close = useUIStore((s) => s.closeDialog)
  const { pop, overlay } = useMotion()

  const [value, setValue] = useState('')
  const [typed, setTyped] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  /** Where focus came from, so closing can give it back. */
  const restoreRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    if (!dialog) return
    setValue(dialog.kind === 'prompt' ? (dialog.spec.defaultValue ?? '') : '')
    setTyped('')
    restoreRef.current = document.activeElement as HTMLElement | null
    const id = window.setTimeout(() => {
      const input = inputRef.current
      if (input) {
        input.focus()
        input.select()
      } else {
        // A confirmation has no field: the panel itself takes focus so the
        // dialog is reachable by keyboard and Escape keeps working.
        panelRef.current?.focus()
      }
    }, 60)
    return () => {
      window.clearTimeout(id)
      // Returning focus is part of the contract: without it the keyboard user
      // lands back at the top of the document after every dialog.
      restoreRef.current?.focus?.()
    }
  }, [dialog])

  /** Keep Tab inside the dialog — a modal that leaks focus is not modal. */
  const trapTab = (event: React.KeyboardEvent) => {
    if (event.key !== 'Tab') return
    const panel = panelRef.current
    if (!panel) return
    const focusable = Array.from(
      panel.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((node) => node.offsetParent !== null)
    if (focusable.length === 0) return
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    const active = document.activeElement
    if (!panel.contains(active)) {
      event.preventDefault()
      first.focus()
    } else if (event.shiftKey && active === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && active === last) {
      event.preventDefault()
      first.focus()
    }
  }

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
        <motion.div key="dialog" className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-8`}>
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            onClick={() => close(dialog.kind === 'confirm' ? false : null)}
            className={MODAL_SCRIM}
          />
          <motion.div
            ref={panelRef}
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            role="dialog"
            aria-modal="true"
            aria-labelledby="phl-dialog-title"
            aria-describedby={dialog.kind === 'confirm' ? 'phl-dialog-message' : undefined}
            tabIndex={-1}
            onKeyDown={trapTab}
            className="relative w-[380px] overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line outline-none"
          >
            <div className="px-4 pb-3 pt-3.5">
              <div className="flex items-start gap-3">
                {dialog.kind === 'confirm' && dialog.spec.tone === 'danger' && (
                  <span className="mt-px flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-danger/10 text-danger">
                    <AlertTriangle size={15} />
                  </span>
                )}
                <div className="min-w-0 flex-1">
                  <h2 id="phl-dialog-title" className="text-base font-medium text-ink">
                    {dialog.spec.title}
                  </h2>
                  {dialog.kind === 'confirm' && (
                    <p id="phl-dialog-message" className="mt-1 text-sm leading-relaxed text-ink-muted">
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
