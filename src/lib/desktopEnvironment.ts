import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * Environment facts & comparison bridge (R1 · M2). The Rust side is strictly
 * read-only: collection touches nothing on disk, never runs DSH dump
 * commands and never reads secret material — the wire types mirror that by
 * carrying availability states instead of empty values.
 */

/** Mirrors the Rust `FactValue`: every fact is known, unknown or unavailable. */
export interface FactValue {
  state: 'known' | 'unknown' | 'unavailable'
  value: string | null
}

export interface EnvironmentFacts {
  schemaVersion: number
  instanceId: string
  instanceName: string
  managementMode: string
  source: string
  observedAt: string
  fingerprint: string
  dsh: { declared: FactValue; installed: FactValue; integrity: FactValue }
  node: {
    binding: FactValue
    actual: FactValue
    platform: FactValue
    arch: FactValue
    system: boolean
  }
  profile: string
  /** The instance's WebUI port (issue #12) — display data. */
  port: FactValue
  /** Env-var KEYS with one-way value digests; values never travel (#12). */
  envKeys: Array<{ key: string; valueDigest: string | null }>
  pluginsAvailable: boolean
  plugins: Array<{
    id: string
    registryId: string
    version: FactValue
    trust: FactValue
    enabled: boolean | null
    order: number
    /** `phl`（安装事务）| `linked`（本地链接）| `bundle`（DSH 管理）(#12). */
    source: string
    /** engines.dsh 判定 vs 本实例 DSH（#10 规则，#12 对比列）。 */
    engine: FactValue
  }>
  api: {
    inheritance: string
    providers: string[]
    hasDefaultModel: boolean
    pendingCredentials: boolean
  }
  workspace: FactValue
  agentsHomeShared: boolean
  conflicts: string[]
}

export interface EnvironmentDiffItem {
  key: string
  category: string
  state: 'same' | 'different' | 'left-only' | 'right-only' | 'unknown'
  left: string | null
  right: string | null
  note: string | null
  /** 该行事实的来源（#12）：manifest / profile / 安装记录 / 实测 / 派生。 */
  source: string | null
}

export interface EnvironmentDiff {
  leftId: string
  rightId: string
  leftName: string
  rightName: string
  leftObservedAt: string
  rightObservedAt: string
  hasUnknown: boolean
  items: EnvironmentDiffItem[]
}

export async function inspectEnvironment(instanceId: string): Promise<EnvironmentFacts | null> {
  if (!isDesktop) return null
  return invoke('inspect_environment', { instanceId })
}

export async function compareEnvironments(
  leftId: string,
  rightId: string,
): Promise<EnvironmentDiff | null> {
  if (!isDesktop) return null
  return invoke('compare_environments', { leftId, rightId })
}
