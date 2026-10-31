import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { PluginSource } from '@/types'

/**
 * The instance-template half of the desktop bridge (issue #13). A template
 * is a REPRODUCTION LIST distilled from one managed instance — never a
 * backup: no plugin files, no sessions/attachments/workspace, credential
 * values stripped to key-only entries. Reproduction runs through the regular
 * create + install-transaction machinery; this bridge only moves the file
 * and its offline preview.
 */

export interface TemplateEnvEntry {
  key: string
  /** `Some` = non-sensitive value that reproduces verbatim; `null` = key-only. */
  value: string | null
}

export interface TemplatePluginEntry {
  pluginId: string
  registryId: string
  version: string
  enabled: boolean
  source: PluginSource | null
  sourceStatus: 'recorded' | 'missing'
  /** engines.dsh claim read from the SOURCE instance at export time. */
  engineRange: string | null
}

/** Mirrors the Rust `InstanceTemplateFile` (issue #13). */
export interface InstanceTemplateFile {
  formatVersion: number
  exportedAt: string
  name: string
  description: string
  kind: string
  dshVersion: string
  runtimeId: string
  args: string[]
  env: TemplateEnvEntry[]
  plugins: TemplatePluginEntry[]
  excludedPlugins: string[]
}

export interface TemplatePluginPreview {
  pluginId: string
  registryId: string
  version: string
  enabled: boolean
  sourceStatus: string
  engineVerdict: 'compatible' | 'incompatible' | 'unknown'
  engineReason: string
}

/** Mirrors the Rust `InstanceTemplatePreview`. */
export interface InstanceTemplatePreview {
  blocked: string[]
  dshVersionInstalled: boolean
  runtimeInstalled: boolean
  plugins: TemplatePluginPreview[]
  notes: string[]
}

/**
 * Distill a managed instance into a template. `dest` (the save-dialog path)
 * also writes the file with seal-and-reopen verification; `null` extracts
 * without writing. Desktop-only: browser mode has no real instance.
 */
export async function exportInstanceTemplate(
  instanceId: string,
  dest: string | null,
): Promise<InstanceTemplateFile> {
  if (!isDesktop) throw new Error('实例模板仅在桌面应用内可用')
  return invoke<InstanceTemplateFile>('export_instance_template', { instanceId, dest })
}

/** Read a template file. Newer format versions are refused explicitly. */
export async function readInstanceTemplate(path: string): Promise<InstanceTemplateFile> {
  if (!isDesktop) throw new Error('实例模板仅在桌面应用内可用')
  return invoke<InstanceTemplateFile>('read_instance_template', { path })
}

/** The offline create-time preview over an already-read template object. */
export async function previewInstanceTemplate(
  template: InstanceTemplateFile,
): Promise<InstanceTemplatePreview | null> {
  if (!isDesktop) return null
  return invoke<InstanceTemplatePreview>('preview_instance_template', { template })
}

/** Native file-open dialog restricted to template files. */
export async function chooseTemplateFile(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      title: '选择实例模板文件',
      multiple: false,
      directory: false,
      filters: [{ name: 'PHL 实例模板', extensions: ['json'] }],
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}
