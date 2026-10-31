import { useState } from 'react'
import { FileInput, Loader2 } from 'lucide-react'
import { Badge, Button, Input, Notice } from '@/components/ui'
import { useInstanceStore, useUIStore } from '@/stores'
import { parseThrownError } from '@/lib/errorCodes'
import { isDesktop } from '@/lib/desktopCore'
import {
  chooseTemplateFile,
  previewInstanceTemplate,
  readInstanceTemplate,
  type InstanceTemplateFile,
  type InstanceTemplatePreview,
} from '@/lib/desktop'
import { installTemplatePlugins, templateToDraft } from '@/services/templates'

const VERDICT_BADGE: Record<string, { tone: 'ok' | 'danger' | 'warn'; label: string }> = {
  compatible: { tone: 'ok', label: '自述兼容' },
  incompatible: { tone: 'danger', label: '自述不兼容' },
  unknown: { tone: 'warn', label: '自述未知' },
}

/**
 * 「从模板文件创建」(issue #13): pick a .phl-template.json, preview it
 * offline (downloads needed, per-plugin engines.dsh verdicts, exclusions),
 * then create through the regular pipeline and replay the plugin list
 * through the regular install transaction. Deterministic refusals: a
 * blocked preview or a missing version/runtime disables creation — the
 * template flow never downloads on its own.
 */
export function TemplateImportSection() {
  const push = useUIStore((s) => s.push)
  const [file, setFile] = useState<InstanceTemplateFile | null>(null)
  const [preview, setPreview] = useState<InstanceTemplatePreview | null>(null)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [installLine, setInstallLine] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  if (!isDesktop) {
    return (
      <Notice tone="info" title="从模板文件创建">
        实例模板的导入与创建仅在桌面应用内可用。
      </Notice>
    )
  }

  const pick = async () => {
    setError(null)
    const path = await chooseTemplateFile()
    if (!path) return
    setBusy(true)
    try {
      const tpl = await readInstanceTemplate(path)
      const pv = await previewInstanceTemplate(tpl)
      setFile(tpl)
      setPreview(pv)
      setName(tpl.name)
    } catch (err) {
      setFile(null)
      setPreview(null)
      setError(parseThrownError(err).message)
    } finally {
      setBusy(false)
    }
  }

  const blocked = preview && preview.blocked.length > 0
  const missingComponents =
    preview && (!preview.dshVersionInstalled || !preview.runtimeInstalled)

  const create = async () => {
    if (!file || !preview || blocked || missingComponents) return
    setBusy(true)
    setError(null)
    try {
      // S6 (review): the wizard's duplicate-name gate must hold here too —
      // two instances with the same display name are a silent UX trap.
      const takenNames = new Set(
        useInstanceStore.getState().instances.map((i) => i.name),
      )
      const finalName = name.trim() || file.name
      if (takenNames.has(finalName)) {
        setError(`已存在同名实例「${finalName}」，请改一个名字`)
        setBusy(false)
        return
      }
      const draft = templateToDraft(file, finalName)
      const instance = await useInstanceStore.getState().createInstance(draft)
      if (!instance) {
        setBusy(false)
        return
      }
      if (file.plugins.length > 0) {
        const controller = new AbortController()
        const outcome = await installTemplatePlugins(
          instance,
          file,
          (p) =>
            setInstallLine(
              `${p.index + 1}/${p.total} · ${p.registryId} · ${
                p.stage === 'installing'
                  ? '安装中…'
                  : p.stage === 'done'
                    ? '完成'
                    : p.stage === 'failed'
                      ? `失败：${p.detail ?? ''}`
                      : p.stage === 'skipped'
                        ? '跳过（需手动安装）'
                        : '排队中'
              }`,
            ),
          controller.signal,
        )
        setInstallLine(null)
        // M1 (review): the replay bypasses catalogPluginActions, so nothing
        // patched the store — reload the list or the detail page shows 0
        // plugins right after the toast claims success.
        await useInstanceStore.getState().reload()
        const parts = [`已安装 ${outcome.installed} 个`]
        if (outcome.disabled > 0) parts.push(`其中 ${outcome.disabled} 个按模板停用`)
        if (outcome.skipped.length > 0) parts.push(`跳过 ${outcome.skipped.length} 个（缺来源记录）`)
        if (outcome.failed.length > 0) {
          useUIStore.getState().toast({
            kind: 'error',
            title: '部分模板插件未装成',
            message: outcome.failed.join('；'),
            duration: 10000,
          })
        }
        useUIStore.getState().toast({
          kind: outcome.failed.length > 0 ? 'info' : 'success',
          title: '模板插件安装完成',
          message: parts.join('，') + '。',
        })
      }
      push({ name: 'instance', id: instance.id })
    } catch (err) {
      setError(parseThrownError(err).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-base font-medium text-ink">从模板文件创建</h2>
        <span className="text-xs text-ink-faint">
          模板是复现清单（版本、Runtime、设置、插件清单）——不是备份；凭据值不在其中。
        </span>
        <div className="ml-auto">
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void pick()}>
            {busy ? <Loader2 size={13} className="animate-spin" /> : <FileInput size={13} />}
            选择模板文件
          </Button>
        </div>
      </div>

      {error && (
        <div className="mt-2">
          <Notice tone="warn">{error}</Notice>
        </div>
      )}

      {file && preview && (
        <div className="mt-3 space-y-2 text-sm text-ink-muted">
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone="accent">dsh-{file.dshVersion || '未绑定'}</Badge>
            <Badge tone={preview.dshVersionInstalled ? 'ok' : 'warn'}>
              {preview.dshVersionInstalled ? '版本已安装' : '版本未安装'}
            </Badge>
            <Badge tone={preview.runtimeInstalled ? 'ok' : 'warn'}>
              {preview.runtimeInstalled ? 'Runtime 就绪' : 'Runtime 未安装'}
            </Badge>
            <Badge tone="neutral">{file.plugins.length} 个插件</Badge>
          </div>
          {preview.blocked.map((b) => (
            <Notice key={b} tone="danger">
              {b}
            </Notice>
          ))}
          {preview.notes.map((n) => (
            <p key={n} className="text-ink-faint">
              {n}
            </p>
          ))}
          {preview.plugins.length > 0 && (
            <ul className="space-y-1 rounded-md bg-surface-sunken/60 p-2.5">
              {preview.plugins.map((p) => {
                const badge = VERDICT_BADGE[p.engineVerdict]
                return (
                  <li key={p.registryId} className="flex flex-wrap items-center gap-2">
                    <Badge tone={badge.tone}>{badge.label}</Badge>
                    <span className="text-ink">{p.registryId}</span>
                    <span className="font-mono text-xs text-ink-faint">{p.version}</span>
                    {!p.enabled && <Badge tone="neutral">按模板停用</Badge>}
                    {p.sourceStatus === 'missing' && (
                      <span className="text-xs text-warn">缺来源记录，需手动安装</span>
                    )}
                  </li>
                )
              })}
            </ul>
          )}
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="实例名称"
              className="w-64"
              aria-label="模板实例名称"
            />
            <Button
              variant="primary"
              disabled={busy || blocked || missingComponents || !name.trim()}
              onClick={() => void create()}
            >
              {busy ? '创建中…' : '创建实例并安装模板插件'}
            </Button>
            {installLine && <span className="text-xs text-ink-faint">{installLine}</span>}
          </div>
        </div>
      )}
    </section>
  )
}
