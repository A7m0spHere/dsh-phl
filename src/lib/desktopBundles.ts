import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { RemoteInstanceManifest, RemoteInstanceRecord } from './desktopInstances'

/**
 * The bundle half of the desktop bridge (§M2 domain split): the save/open
 * dialogs scoped to bundle files, plus export preview/write and read/import.
 * Imports the instance record/manifest types from `desktopInstances` (one-way
 * dep). Re-exported from `desktop.ts` so consumers are unchanged.
 */

/** Native save dialog. Returns null in the browser or when cancelled. */
export async function chooseSaveFile(
  title: string,
  defaultPath: string,
  filters: { name: string; extensions: string[] }[],
): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { save } = await import('@tauri-apps/plugin-dialog')
    const picked = await save({ title, defaultPath, filters })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/** Native file-open dialog restricted to bundle files. */
export async function chooseBundleFile(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      title: '选择 PHL Bundle 文件',
      multiple: false,
      directory: false,
      filters: [{ name: 'PHL Bundle', extensions: ['json'] }],
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

export interface RemoteBundlePreview {
  name: string
  versionId: string
  runtimeId: string
  port: number
  pluginCount: number
  exportedAt: string
  /** 凭据名:导入后需要用户重新配置的变量(值从不随 Bundle 携带)。 */
  credentials: string[]
  /** 机器本地变量(PATH、DSH_HOME……):导入时会丢弃其值。 */
  machineOnly: string[]
}

/** Mirrors the Rust `BundleExportReport` — what an export kept back. */
export interface RemoteBundleExportReport {
  credentials: string[]
  machineOnly: string[]
}

/** Mirrors the Rust `ImportOutcome` — the created record plus the credential names to re-enter. */
export interface RemoteBundleImportOutcome {
  record: RemoteInstanceRecord
  credentials: string[]
}

/** 导出预览:列出将不写入 Bundle 的字段,不产生任何文件。 */
export async function previewInstanceExport(id: string): Promise<RemoteBundleExportReport> {
  if (!isDesktop) throw new Error('导出 Bundle 仅在桌面端可用')
  return invoke('preview_instance_export', { id })
}

export async function exportInstanceBundle(
  id: string,
  dest: string,
): Promise<RemoteBundleExportReport> {
  if (!isDesktop) throw new Error('导出 Bundle 仅在桌面端可用')
  return invoke('export_instance_bundle', { id, dest })
}

export async function readInstanceBundle(path: string): Promise<RemoteBundlePreview> {
  if (!isDesktop) throw new Error('导入 Bundle 仅在桌面端可用')
  return invoke('read_instance_bundle', { path })
}

export async function importInstanceBundle(
  path: string,
  manifest: RemoteInstanceManifest,
): Promise<RemoteBundleImportOutcome> {
  if (!isDesktop) throw new Error('导入 Bundle 仅在桌面端可用')
  return invoke('import_instance_bundle', { path, manifest })
}
