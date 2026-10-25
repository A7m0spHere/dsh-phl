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
  /**
   * The session's format generation (the `.vN` in its artifact filename, equal
   * to the header's `version`). Mirrors the Rust `HEADER_FORK_MAX_GENERATION`:
   * only generations up to 1 carry the fork marker in the header line a copy
   * rewrites, so anything newer is listed but not migratable.
   */
  formatVersion: number
}

/**
 * Mirrors the Rust `UnreadableSession` — a session that exists on disk but
 * cannot be read. Surfaced instead of silently skipped (#6): a compatibility
 * refusal must show as "N 条无法读取（原因）", never as "无会话".
 */
export interface RemoteUnreadableSession {
  sessionDir: string
  project: string
  /** `unsupported-version` | `encoding-mismatch` | `io` | `corrupt`. */
  kind: 'unsupported-version' | 'encoding-mismatch' | 'io' | 'corrupt'
  /** Human-readable explanation from the backend codec. */
  reason: string
  /** The generation the artifact filename declared, when known. */
  formatVersion: number | null
}

/** Mirrors the Rust `SessionListing`: the readable half AND the refused half. */
export interface RemoteSessionListing {
  sessions: RemoteSessionInfo[]
  unreadable: RemoteUnreadableSession[]
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

/** List the sessions one instance's DSH_HOME holds. Read-only, browser-safe.
 * The listing carries the unreadable half too, so the UI can distinguish
 * "没有会话" from "有 N 条存在但读不出来". */
export async function listSessions(instanceId: string): Promise<RemoteSessionListing> {
  if (!isDesktop) return { sessions: [], unreadable: [] }
  return invoke<RemoteSessionListing>('list_sessions', { id: instanceId })
}

/**
 * List the conversations a raw source DSH_HOME holds (not an instance yet).
 * Used by the adoption wizard's "选择对话" step. The backend shape-gates the
 * path (must classify as a DSH home); in a browser this throws like the rest
 * of the desktop surface.
 */
export async function listHomeSessions(sourceHome: string): Promise<RemoteSessionListing> {
  if (!isDesktop) throw new Error('会话迁移仅在桌面端可用')
  return invoke<RemoteSessionListing>('list_adoption_sessions', { path: sourceHome })
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
