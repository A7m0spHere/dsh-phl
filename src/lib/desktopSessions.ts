/**
 * Session migration bridge (domain module, roadmap O-13).
 *
 * Mirrors the Rust `sessions` command surface. Conversation data stays DSH's:
 * PHL lists, inspects, and copies the on-disk session logs — it never models
 * them. In a plain browser the discovery calls return empty and the copy
 * throws, so the wizard shows a desktop-only notice rather than faking a
 * migration (the project rule: degrade, never pretend to have run).
 */
import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/** Mirrors the Rust `SessionInfo` (a list row). */
export interface RemoteSessionInfo {
  /** Encoded session directory name (`session-…`); the stable handle. */
  sessionDir: string
  /** The project group the session lives under (a cwd-derived key). */
  project: string
  id: string
  createdAtMs: number
  cwd: string | null
  /** Lineage: the id this forked from, if any. */
  parent: string | null
  originSubagent: boolean
}

/** Mirrors the Rust `SessionDetail` (inspect). */
export interface RemoteSessionDetail {
  info: RemoteSessionInfo
  eventCount: number
}

/** Mirrors the Rust `CopyOutcome` (one session × one target). */
export interface RemoteSessionCopyOutcome {
  targetId: string
  targetName: string
  newSessionDir: string
  newId: string
}

/** Copy progress — a 0..1 unit counter across the (session × target) grid. */
export interface RemoteSessionProgress {
  done: number
  total: number
  targetId: string
  newSessionDir: string
}

/** List the sessions one instance's DSH_HOME holds. Read-only, browser-safe. */
export async function listSessions(instanceId: string): Promise<RemoteSessionInfo[]> {
  if (!isDesktop) return []
  return invoke<RemoteSessionInfo[]>('list_sessions', { id: instanceId })
}

/**
 * List the conversations a raw source DSH_HOME holds (not an instance yet).
 * Used by the adoption wizard's "选择对话" step. The backend shape-gates the
 * path (must classify as a DSH home); in a browser this throws like the rest
 * of the desktop surface.
 */
export async function listHomeSessions(sourceHome: string): Promise<RemoteSessionInfo[]> {
  if (!isDesktop) throw new Error('会话迁移仅在桌面端可用')
  return invoke<RemoteSessionInfo[]>('list_adoption_sessions', { path: sourceHome })
}

/** Inspect one session (header + stored event count). */
export async function inspectSession(
  instanceId: string,
  sessionDir: string,
): Promise<RemoteSessionDetail> {
  if (!isDesktop) throw new Error('会话迁移仅在桌面端可用')
  return invoke<RemoteSessionDetail>('inspect_session', { id: instanceId, sessionDir })
}

/** Copy one session into one target instance, preserving lineage + cwd. */
export async function copySession(
  sourceId: string,
  sessionDir: string,
  targetId: string,
): Promise<RemoteSessionCopyOutcome> {
  if (!isDesktop) throw new Error('会话迁移仅在桌面端可用')
  return invoke<RemoteSessionCopyOutcome>('copy_session', { sourceId, sessionDir, targetId })
}

/** Copy several sessions into several targets; streams progress. */
export async function copySessions(
  sourceId: string,
  sessionDirs: string[],
  targetIds: string[],
  onProgress?: (p: RemoteSessionProgress) => void,
): Promise<RemoteSessionCopyOutcome[]> {
  if (!isDesktop) throw new Error('会话迁移仅在桌面端可用')
  const channel = new Channel<RemoteSessionProgress>()
  if (onProgress) channel.onmessage = onProgress
  return invoke<RemoteSessionCopyOutcome[]>('copy_sessions', {
    sourceId,
    sessionDirs,
    targetIds,
    onProgress: channel,
  })
}
