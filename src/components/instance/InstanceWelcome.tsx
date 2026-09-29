import { Boxes, FolderInput, Package, Plus } from 'lucide-react'
import { Button } from '@/components/ui'
import { useUIStore } from '@/stores'

/** First-run choices use the same routes as the populated page's toolbar. */
export function InstanceWelcome() {
  const push = useUIStore((s) => s.push)
  return (
    <section aria-labelledby="instance-welcome-title" className="mt-4 overflow-hidden rounded-lg border border-line bg-surface">
      <div className="p-6 sm:p-8">
        <span className="mb-5 inline-flex rounded-lg bg-accent-soft p-3 text-accent-ink">
          <Boxes size={24} aria-hidden />
        </span>
        <h2 id="instance-welcome-title" className="text-xl font-semibold tracking-tight text-ink">从第一个实例开始</h2>
        <p className="mt-2 max-w-md text-base leading-relaxed text-ink-muted">
          为不同项目保留各自的 DSH 版本、运行时与插件。升级或试验新配置时，也能保留原来的工作环境。
        </p>
        <Button variant="primary" size="lg" className="mt-5" onClick={() => push({ name: 'create' })}>
          <Plus size={14} />新建实例
        </Button>
      </div>
      <div className="grid gap-4 border-t border-line bg-surface-sunken p-5 sm:grid-cols-2">
        <div>
          <h3 className="text-base font-medium text-ink">已经在使用 DSH？</h3>
          <p className="mb-3 mt-1 text-sm leading-relaxed text-ink-muted">发现本机已有环境，选择接入方式。</p>
          <Button onClick={() => push({ name: 'adopt' })}><FolderInput size={13} />接入本机 DSH</Button>
        </div>
        <div>
          <h3 className="text-base font-medium text-ink">有现成的整合包？</h3>
          <p className="mb-3 mt-1 text-sm leading-relaxed text-ink-muted">从整合包准备版本、插件和实例配置。</p>
          <Button onClick={() => push({ name: 'installPack' })}><Package size={13} />安装整合包</Button>
        </div>
      </div>
    </section>
  )
}
