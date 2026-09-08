import { useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  ArrowRight,
  Blocks,
  Boxes,
  Check,
  Cpu,
  Layers,
  Package,
  Play,
  ShieldCheck,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { hueTone } from '@/lib/hue'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { useIsDark, useSettingsStore, useUIStore } from '@/stores'
import { Badge, Button, Chip } from '@/components/ui'
import { InstanceTile } from '@/components/instance/InstanceTile'
import { Logo } from './Logo'

/* ------------------------------------------------------------------ *
 * illustrations — built from the app's own primitives, so the guide
 * teaches the actual interface rather than a picture of one
 * ------------------------------------------------------------------ */

function ProblemArt() {
  const dark = useIsDark()
  return (
    <div className="space-y-2">
      <div className="rounded-lg bg-surface-sunken p-3 ring-1 ring-inset ring-line">
        <div className="mb-2 text-2xs font-semibold uppercase tracking-wider text-ink-faint">
          没有 PHL 的时候
        </div>
        <div className="flex items-center gap-2 text-sm">
          <Chip>全局 DSH</Chip>
          <ArrowRight size={12} className="text-ink-faint" />
          <Chip>升级到 rc.8</Chip>
          <ArrowRight size={12} className="text-ink-faint" />
          <span className="text-danger">旧版本环境被覆盖</span>
        </div>
        <p className="mt-2 text-sm leading-relaxed text-ink-faint">
          想回到 rc.5 复现一个问题，就得把整套环境再装一遍；插件和配置也早就混在一起了。
        </p>
      </div>

      <div className="rounded-lg bg-accent-soft/50 p-3 ring-1 ring-inset ring-accent/15">
        <div className="mb-2 text-2xs font-semibold uppercase tracking-wider text-accent-ink">
          有了 PHL
        </div>
        <div className="flex flex-wrap gap-1.5">
          {[
            { name: 'Production', hue: 3, v: 'rc.7' },
            { name: 'Plugin Dev', hue: 0, v: 'rc.7' },
            { name: 'Legacy Test', hue: 4, v: 'rc.5' },
          ].map((i) => {
            const tone = hueTone(i.hue, dark)
            return (
              <span
                key={i.name}
                className="inline-flex items-center gap-1.5 rounded-full px-2 py-1 text-2xs"
                style={{ background: tone.soft, color: tone.text }}
              >
                <span className="h-[5px] w-[5px] rounded-full" style={{ background: tone.solid }} />
                {i.name}
                <span className="opacity-70">DSH {i.v}</span>
              </span>
            )
          })}
        </div>
        <p className="mt-2 text-sm leading-relaxed text-ink-muted">
          三套环境同时存在、可以同时运行，互相之间不会污染。
        </p>
      </div>
    </div>
  )
}

function InstanceArt() {
  return (
    <div className="rounded-lg bg-surface p-3 ring-1 ring-inset ring-line">
      <div className="flex items-center gap-2.5">
        <InstanceTile name="Plugin Development" hue={0} status="running" size={32} />
        <div className="min-w-0">
          <div className="text-base font-medium text-ink">Plugin Development</div>
          <div className="text-sm text-ink-faint">一个实例 = 一整套独立环境</div>
        </div>
      </div>

      <div className="mt-3 grid gap-1.5 sm:grid-cols-2">
        {[
          { icon: <Package size={12} />, k: 'DSH 版本', v: '0.1.0-rc.7' },
          { icon: <Cpu size={12} />, k: 'Runtime', v: 'Node 22' },
          { icon: <Blocks size={12} />, k: '插件', v: '12 个' },
          { icon: <Layers size={12} />, k: 'DSH_HOME', v: '独立目录' },
          { icon: <ShieldCheck size={12} />, k: 'Profile / 配置', v: '不共享' },
          { icon: <Play size={12} />, k: '端口', v: ':3081' },
        ].map((row) => (
          <div
            key={row.k}
            className="flex items-center gap-2 rounded-sm bg-surface-sunken px-2 py-1.5 text-sm"
          >
            <span className="text-ink-faint">{row.icon}</span>
            <span className="text-ink-muted">{row.k}</span>
            <span className="ml-auto font-medium text-ink">{row.v}</span>
          </div>
        ))}
      </div>

      <p className="mt-2.5 text-sm leading-relaxed text-ink-faint">
        删除实例时，这些东西会一起被清掉，不会在系统里留下残留。
      </p>
    </div>
  )
}

function NavArt() {
  return (
    <div className="space-y-1.5">
      {[
        {
          icon: <Boxes size={13} />,
          k: '实例',
          v: '主页。看状态、启动、停止、新建。日常只用这一页。',
        },
        {
          icon: <Package size={13} />,
          k: '版本',
          v: '安装 / 删除 DSH 版本。多个版本可以同时装着。',
        },
        {
          icon: <Blocks size={13} />,
          k: '插件',
          v: '装插件。注意左边要先选「装到哪个实例」。',
        },
        {
          icon: <Cpu size={13} />,
          k: '运行时',
          v: 'Node 版本。和 DSH 版本是分开的，实例各自绑定。',
        },
      ].map((row) => (
        <div
          key={row.k}
          className="flex gap-2.5 rounded-lg bg-surface px-3 py-2 ring-1 ring-inset ring-line"
        >
          <span className="mt-[3px] shrink-0 text-accent-ink">{row.icon}</span>
          <div className="min-w-0">
            <div className="text-base font-medium text-ink">{row.k}</div>
            <div className="mt-0.5 text-sm leading-relaxed text-ink-muted">{row.v}</div>
          </div>
        </div>
      ))}
      <p className="px-1 pt-1 text-sm leading-relaxed text-ink-faint">
        每页左侧那一栏是这一页的筛选和作用域，不会跳走；右边才是内容。
      </p>
    </div>
  )
}

function StartArt() {
  return (
    <div className="space-y-2">
      <ol className="space-y-1.5">
        {[
          '在「版本」里装一个 DSH 版本',
          '回到「实例」，点右上角「新建实例」',
          '按向导选版本、选 Runtime、选模板',
          '创建完成后，点底部那条栏上的「启动」',
        ].map((step, i) => (
          <li
            key={step}
            className="flex items-center gap-2.5 rounded-lg bg-surface px-3 py-2 ring-1 ring-inset ring-line"
          >
            <span className="num flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-accent-soft text-2xs font-semibold text-accent-ink">
              {i + 1}
            </span>
            <span className="text-base text-ink">{step}</span>
          </li>
        ))}
      </ol>

      <div className="rounded-lg bg-surface-sunken p-3 ring-1 ring-inset ring-line">
        <div className="mb-1.5 text-2xs font-semibold uppercase tracking-wider text-ink-faint">
          几个有用的快捷键
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-sm text-ink-muted">
          <span>
            <b className="font-mono text-ink">Ctrl K</b> 快速跳转
          </span>
          <span>
            <b className="font-mono text-ink">Ctrl ,</b> 打开设置
          </span>
          <span>
            <b className="font-mono text-ink">Esc</b> 关闭弹窗与返回
          </span>
        </div>
        <p className="mt-1.5 text-sm text-ink-faint">
          完整清单在「设置 → 快捷键」里，随时可以翻。
        </p>
      </div>

      <div className="rounded-lg bg-warn/[0.08] p-3 text-sm leading-relaxed text-ink-muted ring-1 ring-inset ring-warn/20">
        版本、插件、实例、Runtime 与启动都已是<b className="text-ink">真实实现</b>：下载与安装会写入磁盘，
        启动会拉起真实的 DSH 进程并占用所选端口，停止会终止整个进程树。
      </div>
    </div>
  )
}

/* ------------------------------------------------------------------ */

interface Step {
  id: string
  title: string
  lead: string
  art: ReactNode
}

const STEPS: Step[] = [
  {
    id: 'why',
    title: 'PHL 解决什么问题',
    lead: '让多个 DSH 版本在同一台机器上共存，而不是互相覆盖。',
    art: <ProblemArt />,
  },
  {
    id: 'instance',
    title: '实例是这里的核心',
    lead: '你管理的不是「某个 DSH 版本」，而是一整套固定下来的运行环境。',
    art: <InstanceArt />,
  },
  {
    id: 'nav',
    title: '五个页面分别做什么',
    lead: '顶部是主导航，左侧那一栏属于当前页面。',
    art: <NavArt />,
  },
  {
    id: 'start',
    title: '从这里开始',
    lead: '四步就能跑起第一个实例。',
    art: <StartArt />,
  },
]

/**
 * First-run guide. It runs on the app's own components rather than static
 * images, so what the user sees here is literally what they will meet on the
 * next screen — and it stays correct when the design changes.
 */
export function GuideOverlay() {
  const open = useUIStore((s) => s.guideOpen)
  const setOpen = useUIStore((s) => s.setGuideOpen)
  const markSeen = useSettingsStore((s) => s.markGuideSeen)
  const [index, setIndex] = useState(0)
  const { t, overlay, pop, scale } = useMotion()

  const step = STEPS[index]
  const last = index === STEPS.length - 1

  const finish = () => {
    markSeen()
    setOpen(false)
    setIndex(0)
  }

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="guide"
          className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-6`}
        >
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            className={MODAL_SCRIM}
          />

          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            role="dialog"
            aria-modal="true"
            aria-labelledby="phl-guide-title"
            className="relative flex max-h-full w-[520px] flex-col overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line"
          >
            <header className="flex items-center gap-2.5 border-b border-line px-4 py-3">
              <Logo size={20} />
              <div className="min-w-0 flex-1">
                <div id="phl-guide-title" className="text-base font-semibold tracking-tight text-ink">
                  欢迎使用 PHL
                </div>
                <div className="text-2xs text-ink-faint">DSH Instance &amp; Runtime Manager</div>
              </div>
              <Badge tone="neutral">
                {index + 1} / {STEPS.length}
              </Badge>
            </header>

            <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3.5">
              <AnimatePresence mode="wait" initial={false}>
                <motion.div
                  key={step.id}
                  initial={{ opacity: 0, x: scale === 0 ? 0 : 14 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: scale === 0 ? 0 : -10 }}
                  transition={t(0.2)}
                >
                  <h2 className="text-lg font-semibold tracking-tight text-ink">{step.title}</h2>
                  <p className="mb-3 mt-1 text-sm leading-relaxed text-ink-muted">{step.lead}</p>
                  {step.art}
                </motion.div>
              </AnimatePresence>
            </div>

            <footer className="flex items-center gap-2 border-t border-line bg-surface-sunken/60 px-4 py-2.5">
              <div className="flex items-center gap-1.5">
                {STEPS.map((s, i) => (
                  <button
                    key={s.id}
                    aria-label={s.title}
                    onClick={() => setIndex(i)}
                    className={cn(
                      'h-1.5 rounded-full transition-all duration-200',
                      i === index ? 'w-4 bg-accent' : 'w-1.5 bg-ink/15 hover:bg-ink/30',
                    )}
                  />
                ))}
              </div>

              <div className="ml-auto flex items-center gap-1.5">
                <Button variant="ghost" size="md" onClick={finish}>
                  跳过
                </Button>
                {index > 0 && (
                  <Button variant="secondary" size="md" onClick={() => setIndex(index - 1)}>
                    上一步
                  </Button>
                )}
                <Button
                  variant="primary"
                  size="md"
                  className="min-w-[74px]"
                  onClick={() => (last ? finish() : setIndex(index + 1))}
                >
                  {last ? (
                    <>
                      <Check size={12} /> 开始使用
                    </>
                  ) : (
                    <>
                      下一步 <ArrowRight size={12} />
                    </>
                  )}
                </Button>
              </div>
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
