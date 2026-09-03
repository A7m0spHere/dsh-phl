import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Blocks,
  Boxes,
  CornerDownLeft,
  Cpu,
  Package,
  Play,
  Plus,
  Search,
  Settings,
  Square,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { useCatalogStore, useInstanceStore, useUIStore, type Route } from '@/stores'
import { Kbd } from '@/components/ui'
import { InstanceTile } from '@/components/instance'

interface Command {
  id: string
  label: string
  hint?: string
  group: string
  icon: ReactNode
  run: () => void
}

/**
 * Quick switcher. A launcher's job is to get you into an environment fast;
 * once there are a dozen instances, typing three letters beats scanning a
 * grid.
 */
export function QuickSwitcher() {
  const open = useUIStore((s) => s.paletteOpen)
  const setOpen = useUIStore((s) => s.setPaletteOpen)
  const navigate = useUIStore((s) => s.navigate)
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const toggle = useInstanceStore((s) => s.toggle)
  const versions = useCatalogStore((s) => s.versions)
  const { overlay, pop, scale } = useMotion()

  const [query, setQuery] = useState('')
  const [cursor, setCursor] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    setQuery('')
    setCursor(0)
    const id = window.setTimeout(() => inputRef.current?.focus(), 50)
    return () => window.clearTimeout(id)
  }, [open])

  const commands = useMemo<Command[]>(() => {
    const go = (route: Route) => () => {
      navigate(route)
      setOpen(false)
    }

    const instanceCommands: Command[] = instances.flatMap((i) => {
      const status = states[i.id]?.status ?? 'stopped'
      const version = versions.find((v) => v.id === i.versionId)?.name
      return [
        {
          id: `open-${i.id}`,
          label: i.name,
          hint: `DSH ${version} · :${i.port}`,
          group: '实例',
          icon: <InstanceTile name={i.name} hue={i.hue} status={status} size={22} />,
          run: go({ name: 'instance', id: i.id }),
        },
        {
          id: `toggle-${i.id}`,
          label: `${status === 'running' ? '停止' : '启动'} ${i.name}`,
          hint: `:${i.port}`,
          group: '操作',
          icon:
            status === 'running' ? (
              <Square size={13} className="text-ink-faint" />
            ) : (
              <Play size={13} className="fill-current text-ink-faint" />
            ),
          run: () => {
            toggle(i.id)
            setOpen(false)
          },
        },
      ]
    })

    const pageCommands: Command[] = [
      {
        id: 'new',
        label: '新建实例',
        group: '操作',
        icon: <Plus size={13} className="text-ink-faint" />,
        run: go({ name: 'create' }),
      },
      {
        id: 'p-instances',
        label: '实例',
        group: '页面',
        icon: <Boxes size={13} className="text-ink-faint" />,
        run: go({ name: 'instances' }),
      },
      {
        id: 'p-versions',
        label: 'DSH 版本',
        group: '页面',
        icon: <Package size={13} className="text-ink-faint" />,
        run: go({ name: 'versions' }),
      },
      {
        id: 'p-plugins',
        label: '插件',
        group: '页面',
        icon: <Blocks size={13} className="text-ink-faint" />,
        run: go({ name: 'plugins' }),
      },
      {
        id: 'p-runtimes',
        label: '运行时',
        group: '页面',
        icon: <Cpu size={13} className="text-ink-faint" />,
        run: go({ name: 'runtimes' }),
      },
      {
        id: 'p-settings',
        label: '设置',
        group: '页面',
        icon: <Settings size={13} className="text-ink-faint" />,
        run: go({ name: 'settings' }),
      },
    ]

    return [...instanceCommands, ...pageCommands]
  }, [instances, states, versions, navigate, setOpen, toggle])

  const results = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return commands.filter((c) => c.group !== '操作' || c.id === 'new').slice(0, 9)
    return commands
      .filter((c) => c.label.toLowerCase().includes(q) || (c.hint ?? '').toLowerCase().includes(q))
      .slice(0, 10)
  }, [commands, query])

  useEffect(() => setCursor(0), [query])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false)
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setCursor((c) => Math.min(results.length - 1, c + 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setCursor((c) => Math.max(0, c - 1))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        results[cursor]?.run()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [open, results, cursor, setOpen])

  useEffect(() => {
    listRef.current
      ?.querySelectorAll('[data-row]')
      [cursor]?.scrollIntoView({ block: 'nearest' })
  }, [cursor])

  let lastGroup = ''

  return (
    <AnimatePresence>
      {open && (
        <motion.div key="palette" className={`fixed inset-0 ${MODAL_Z} flex items-start justify-center pt-[14vh]`}>
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            onClick={() => setOpen(false)}
            className={MODAL_SCRIM}
          />
          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            className="relative w-[520px] overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="flex items-center gap-2.5 border-b border-line px-3.5 py-3">
              <Search size={15} className="shrink-0 text-ink-faint" />
              <input
                ref={inputRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="搜索实例、页面或操作"
                className="min-w-0 flex-1 bg-transparent text-md text-ink outline-none placeholder:text-ink-faint/70"
              />
              <Kbd>Esc</Kbd>
            </div>

            <div ref={listRef} className="max-h-[336px] overflow-y-auto p-1.5">
              {results.length === 0 ? (
                <div className="px-3 py-8 text-center text-base text-ink-faint">没有匹配的结果</div>
              ) : (
                results.map((cmd, index) => {
                  const showGroup = cmd.group !== lastGroup
                  lastGroup = cmd.group
                  return (
                    <div key={cmd.id}>
                      {showGroup && (
                        <div className="px-2 pb-1 pt-2 text-2xs font-semibold uppercase tracking-wider text-ink-faint">
                          {cmd.group}
                        </div>
                      )}
                      <button
                        data-row
                        onMouseEnter={() => setCursor(index)}
                        onClick={cmd.run}
                        className={cn(
                          'flex w-full items-center gap-2.5 rounded-sm px-2 py-2 text-left transition-colors duration-100',
                          index === cursor ? 'bg-accent-soft' : 'hover:bg-surface-hover',
                        )}
                      >
                        <span className="flex h-[22px] w-[22px] shrink-0 items-center justify-center">
                          {cmd.icon}
                        </span>
                        <span
                          className={cn(
                            'min-w-0 flex-1 truncate text-base',
                            index === cursor ? 'text-accent-ink' : 'text-ink',
                          )}
                        >
                          {cmd.label}
                        </span>
                        {cmd.hint && (
                          <span className="num shrink-0 text-sm text-ink-faint">{cmd.hint}</span>
                        )}
                        {index === cursor && scale !== 0 && (
                          <motion.span
                            initial={{ opacity: 0, x: -4 }}
                            animate={{ opacity: 1, x: 0 }}
                            className="shrink-0 text-ink-faint"
                          >
                            <CornerDownLeft size={12} />
                          </motion.span>
                        )}
                      </button>
                    </div>
                  )
                })
              )}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
