import * as desktop from '@/lib/desktop'
import type { InstanceTemplateFile, InstanceTemplatePreview } from '@/lib/desktop'
import type { Instance, InstanceDraft } from '@/types'
import { Cancelled, newTransferId } from './repository'
import { npmRegistryBase } from './tauriPlugins'
import { parseThrownError } from '@/lib/errorCodes'

/**
 * Instance-environment template service (issue #13). A template is a
 * reproduction list, NOT a backup — these helpers move the file, drive the
 * offline preview and replay the plugin list through the EXISTING install
 * transaction, one plugin at a time. No second installer lives here.
 */

/** The wizard-independent draft a template produces (auto port, web profile). */
export function templateToDraft(tpl: InstanceTemplateFile, name: string): InstanceDraft {
  return {
    name,
    note: tpl.description,
    kind: (tpl.kind as InstanceDraft['kind']) || 'sandbox',
    hue: 0,
    versionId: tpl.dshVersion ? `dsh-${tpl.dshVersion}` : null,
    runtimeId: tpl.runtimeId || null,
    templateId: 'blank',
    autoPort: true,
    port: 0,
    apiInheritance: 'default',
    env: Object.fromEntries(
      tpl.env.filter((e) => e.value !== null).map((e) => [e.key, e.value as string]),
    ),
    args: [...tpl.args],
  }
}

export interface TemplateInstallProgress {
  pluginId: string
  registryId: string
  index: number
  total: number
  stage: 'queued' | 'installing' | 'done' | 'failed' | 'skipped'
  detail?: string
}

export interface TemplateInstallOutcome {
  installed: number
  disabled: number
  failed: string[]
  skipped: string[]
}

/**
 * Replay the template's plugin list through the regular install transaction,
 * sequentially (two npm installs racing split bandwidth, and a serial loop
 * makes per-plugin failure reporting honest). A failure never aborts the
 * remaining plugins — every outcome lands in the summary. `enabled: false`
 * entries are installed and then switched off, so the copy reproduces the
 * source's enable state exactly.
 */
export async function installTemplatePlugins(
  instance: Instance,
  tpl: InstanceTemplateFile,
  onProgress: (p: TemplateInstallProgress) => void,
  signal: AbortSignal,
): Promise<TemplateInstallOutcome> {
  const outcome: TemplateInstallOutcome = {
    installed: 0,
    disabled: 0,
    failed: [],
    skipped: [],
  }
  const queue = tpl.plugins
  let index = 0
  for (const entry of queue) {
    const base: TemplateInstallProgress = {
      pluginId: entry.pluginId,
      registryId: entry.registryId,
      index,
      total: queue.length,
      stage: 'queued',
    }
    index += 1
    if (!entry.source || entry.sourceStatus !== 'recorded') {
      outcome.skipped.push(entry.registryId)
      onProgress({ ...base, stage: 'skipped', detail: '安装记录缺少来源，需手动安装' })
      continue
    }
    onProgress({ ...base, stage: 'installing' })
    const transferId = newTransferId(`tpl:${instance.id}:${entry.registryId}`)
    const onAbort = () => void desktop.cancelTransfer(transferId)
    if (signal.aborted) throw new Cancelled()
    signal.addEventListener('abort', onAbort, { once: true })
    try {
      await desktop.installPlugin({
        transferId,
        pluginId: entry.pluginId,
        source: entry.source,
        version: entry.version || undefined,
        registryBase: npmRegistryBase(),
        instanceId: instance.id,
        onProgress: () => {},
      })
      outcome.installed += 1
      if (!entry.enabled) {
        await desktop.setPluginEnabled(instance.id, entry.registryId, false)
        outcome.disabled += 1
      }
      onProgress({ ...base, stage: 'done' })
    } catch (err) {
      if (signal.aborted) throw new Cancelled()
      outcome.failed.push(`${entry.registryId}：${parseThrownError(err).message}`)
      onProgress({ ...base, stage: 'failed', detail: parseThrownError(err).message })
    } finally {
      signal.removeEventListener('abort', onAbort)
    }
  }
  return outcome
}

export type { InstanceTemplateFile, InstanceTemplatePreview }
