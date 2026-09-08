import { parseThrownError } from '@/lib/errorCodes'
import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, ArrowLeft, Check, Download, Loader2, ShieldCheck } from 'lucide-react'
import {
  choosePackSavePath,
  exportInstancePack,
  previewInstancePackExport,
  type PackExportOptions,
  type RemotePackExportPlan,
} from '@/lib/desktop'
import { isDesktop } from '@/lib/desktopCore'
import { useUIStore } from '@/stores'
import { Badge, Button, Card, Checkbox, EmptyState, ProgressBar, SectionCard, Skeleton } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'

/**
 * "导出整合包" (P1-3): the confirmation screen for `.phlpack` export. It shows
 * exactly what will travel and what will be kept back — local plugins the user
 * chooses to embed, optional history behind a privacy gate, and the credential
 * names that will *not* be included.
 */
export function PackExportPage({ id }: { id: string }) {
  const push = useUIStore((s) => s.push)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)
  const navigate = useUIStore((s) => s.navigate)

  const [plan, setPlan] = useState<RemotePackExportPlan | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [embed, setEmbed] = useState<Set<string>>(new Set())
  const [includeSessions, setIncludeSessions] = useState(false)
  const [working, setWorking] = useState(false)
  const [progress, setProgress] = useState(0)
  const exportController = useRef<AbortController | null>(null)

  useEffect(() => {
    let alive = true
    if (isDesktop) {
      previewInstancePackExport(id)
        .then((p) => {
          if (!alive) return
          setPlan(p)
          // Default the embed set to the plugins we can't re-download.
          setEmbed(new Set(p.plugins.filter((pl) => pl.embedRecommended).map((pl) => pl.registryId)))
        })
        .catch((e) => alive && setError(parseThrownError(e).message))
    }
    return () => {
      alive = false
      exportController.current?.abort()
    }
  }, [id])

  const startExport = async () => {
    if (!plan) return
    let sessionsPrivacyAck = false
    if (includeSessions && plan.sessionCount > 0) {
      const ok = await confirm({
        title: '包含历史对话？',
        message: '历史对话可能包含：用户输入、文件内容、项目路径、工具调用结果、私有代码，以及 Token 等敏感信息。',
        detail: '分享整合包前请确认你了解相关隐私风险。',
        tone: 'danger',
        confirmLabel: '我已了解，包含对话',
      })
      if (!ok) return
      sessionsPrivacyAck = true
    }
    const dest = await choosePackSavePath(plan.name)
    if (!dest) return
    const options: PackExportOptions = {
      embedRegistryIds: [...embed],
      includeSessions,
      sessionsPrivacyAck,
    }
    setWorking(true)
    setProgress(0.4)
    exportController.current = new AbortController()
    try {
      const report = await exportInstancePack(id, dest, options, exportController.current.signal)
      setProgress(1)
      // §12: if the core tree-walker withheld secret-named files from any
      // embedded plugin / the session tree, say exactly which — a pack that
      // silently drops them would be a surprise on the receiving end.
      if (report.secretFilesWithheld.length > 0) {
        toast({
          kind: 'warn',
          title: '已扣下疑似凭据文件',
          message: `以下内容按规格 §12 未打包（对方安装后需重新配置）：${report.secretFilesWithheld.join('、')}`,
        })
      }
      // Links never ride in a pack (machine paths are meaningless on another
      // machine); version-managed ones rebuild on install, so this is an
      // info line, not a warning — but the count belongs to the user.
      if (report.linksSkipped.length > 0) {
        toast({
          kind: 'info',
          title: `未打包 ${report.linksSkipped.length} 个文件系统链接`,
          message: '整合包只携带文件本体；受管的依赖链接会在对方安装时自动重建。',
        })
      }
      toast({
        kind: 'success',
        title: '整合包已导出',
        message:
          `${report.pluginCount} 个插件（内置 ${report.embeddedCount}）` +
          (report.sessionsIncluded ? ' · 含历史对话' : '') +
          (report.credentialNames.length ? ` · 未含凭据 ${report.credentialNames.length} 项` : ''),
      })
      navigate({ name: 'instance', id })
    } catch (e) {
      const msg = parseThrownError(e).message
      if (msg === 'cancelled') {
        toast({ kind: 'info', title: '已取消导出' })
      } else {
        setError(msg)
        toast({ kind: 'error', title: '导出失败', message: msg })
      }
    } finally {
      exportController.current = null
      setWorking(false)
    }
  }

  const toggleEmbed = (rid: string) => {
    setEmbed((prev) => {
      const next = new Set(prev)
      next.has(rid) ? next.delete(rid) : next.add(rid)
      return next
    })
  }

  return (
    <PageShell
      title="导出整合包"
      subtitle="把该实例变成一个可分享的 .phlpack：插件、配置与可选历史对话；凭据永不随包携带。"
      actions={
        <Button variant="ghost" onClick={() => push({ name: 'instance', id })}>
          <ArrowLeft size={13} />
          返回
        </Button>
      }
    >
      {!isDesktop ? (
        <EmptyState
          icon={<Download size={20} />}
          title="桌面端功能"
          description="导出整合包需要桌面版 PHL 读取实例目录。浏览器模式（npm run dev）下此流程不可用。"
        />
      ) : error && !plan ? (
        <Card tone="danger" className="p-4 text-sm text-ink">无法生成导出预览：{error}</Card>
      ) : !plan ? (
        <Skeleton className="h-[220px]" />
      ) : (
        <div className="flex flex-col gap-3">
          <SectionCard title={plan.name} description={`DSH ${plan.versionId} · ${plan.runtimeId} · Profile ${plan.profile}`}>
            {plan.warnings.length > 0 && (
              <ul className="mb-2 flex flex-col gap-1 text-xs text-ink-muted">
                {plan.warnings.map((w, i) => (
                  <li key={i} className="flex items-start gap-1.5">
                    <AlertTriangle size={12} className="mt-0.5 shrink-0 text-warn" />
                    {w}
                  </li>
                ))}
              </ul>
            )}
            <div className="flex items-center gap-2 text-xs text-ink-muted">
              <ShieldCheck size={13} className="text-ok" />
              凭据将被排除：{plan.credentialNames.length ? plan.credentialNames.join('、') : '无'}
            </div>
          </SectionCard>

          <SectionCard title="插件" description="勾选要内置进整合包的插件；未勾选的远程插件在安装时从注册表重新下载。">
            <div className="flex flex-col gap-1.5">
              {plan.plugins.length === 0 && <p className="text-sm text-ink-faint">该实例还没有插件。</p>}
              {plan.plugins.map((p) => (
                <label key={p.registryId || p.pluginId} className="flex items-center gap-2 text-sm">
                  <Checkbox checked={embed.has(p.registryId)} onChange={() => toggleEmbed(p.registryId)} />
                  <span className="min-w-0 flex-1 truncate">
                    <span className="text-ink">{p.pluginId}</span>
                    <span className="ml-2 text-xs text-ink-faint">v{p.version}</span>
                  </span>
                  {p.registryAvailable ? (
                    <Badge tone="neutral">可远程下载</Badge>
                  ) : (
                    <Badge tone="warn">建议内置</Badge>
                  )}
                  {p.licenseUnknown && <Badge tone="warn">许可未知</Badge>}
                </label>
              ))}
            </div>
          </SectionCard>

          <SectionCard title="历史对话" description={`该实例有 ${plan.sessionCount} 条历史对话。默认不包含。`}>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={includeSessions}
                disabled={plan.sessionCount === 0}
                onChange={() => setIncludeSessions((v) => !v)}
              />
              包含历史对话（勾选后将二次确认隐私风险）
            </label>
          </SectionCard>

          {working && (
            <Card className="p-4">
              <p className="mb-2 text-sm text-ink">正在打包…</p>
              <ProgressBar value={progress} active />
            </Card>
          )}

          <div className="flex justify-end">
            {working && (
              <Button variant="secondary" onClick={() => exportController.current?.abort()}>
                取消导出
              </Button>
            )}
            <Button variant="primary" disabled={working} onClick={() => void startExport()}>
              {working ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />}
              导出为 .phlpack
            </Button>
          </div>
        </div>
      )}
    </PageShell>
  )
}
