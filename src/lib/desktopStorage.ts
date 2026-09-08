import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * The storage / data-root half of the desktop bridge (§M2 domain split): disk
 * probes, the data-root summary + move, and the migration journal that makes an
 * interrupted move recoverable. Moved out of `desktop.ts` behind the compat
 * re-export door; browser mocks (interesting free-space numbers, a demo
 * migration) are preserved verbatim.
 */

/** One-level look at `<root>/<kind>` — cheap, no deep walks. */
export interface DirSummary {
  exists: boolean
  entries: number
}

export interface RootDataSummary {
  instances: DirSummary
  versions: DirSummary
  runtimes: DirSummary
  config: DirSummary
  cache: DirSummary
  hasData: boolean
}

export interface MoveProgress {
  /** Which data directory is being moved: instances/versions/runtimes/cache. */
  kind: string
  progress: number
  bytesDone: number
  bytesTotal: number
}

export interface MoveSummary {
  moved: string[]
  bytes: number
  cancelled: boolean
}

/**
 * Free bytes on the drive containing `path`. The browser mock keeps the demo
 * interesting: C: reads roomy, D:/E: tighter. Desktop errors (unmapped drive)
 * resolve to `null` so callers can simply hide the figure.
 */
export async function freeSpace(path: string): Promise<number | null> {
  if (!isDesktop) {
    if (/^[eE]:/.test(path)) return 8.4 * 1024 ** 3
    if (/^[dD]:/.test(path)) return 24.6 * 1024 ** 3
    return 118.3 * 1024 ** 3
  }
  try {
    return await invoke<number>('free_space', { path })
  } catch {
    return null
  }
}

export async function rootDataSummary(root: string): Promise<RootDataSummary> {
  if (!isDesktop) {
    // Browser demo: the mock world always "has" data, so the migration
    // prompt and its flow are demonstrable.
    return {
      instances: { exists: true, entries: 3 },
      versions: { exists: true, entries: 2 },
      runtimes: { exists: true, entries: 1 },
      config: { exists: true, entries: 1 },
      cache: { exists: true, entries: 1 },
      hasData: true,
    }
  }
  return invoke<RootDataSummary>('root_data_summary', { root })
}

/**
 * Moves instances/versions/runtimes/cache from one root to another.
 * Cancellation is a normal outcome (`cancelled: true`) with partial progress
 * preserved and resumable by calling again with the same roots.
 */
export async function moveRootData(
  from: string,
  to: string,
  transferId: string,
  onProgress: (p: MoveProgress) => void,
): Promise<MoveSummary> {
  if (!isDesktop) {
    const kinds = ['instances', 'versions', 'runtimes', 'config', 'cache']
    const sizes = [4.2e9, 0.9e9, 0.6e9, 1024, 0.2e9]
    for (let i = 0; i < kinds.length; i++) {
      for (let step = 1; step <= 8; step++) {
        await new Promise((resolve) => setTimeout(resolve, 120))
        onProgress({
          kind: kinds[i],
          progress: step / 8,
          bytesDone: (sizes[i] * step) / 8,
          bytesTotal: sizes[i],
        })
      }
    }
    return { moved: kinds, bytes: sizes.reduce((a, b) => a + b, 0), cancelled: false }
  }
  const channel = new Channel<MoveProgress>()
  channel.onmessage = onProgress
  return invoke<MoveSummary>('move_root_data', { from, to, transferId, onProgress: channel })
}

/* --------------------------- migration journal --------------------------- */

export interface MigrationJournalEntry {
  kind: string
  /** pending = untouched, moving = interrupted mid-copy, moved = arrived. */
  state: 'pending' | 'moving' | 'moved'
  bytes: number
}

export interface MigrationJournal {
  from: string
  to: string
  entries: MigrationJournalEntry[]
  committed: boolean
  startedAt: string
}

/**
 * The journal of an interrupted or cancelled data-root migration, or `null`.
 * An open journal is the restart-recovery entry: 继续 resumes from the
 * record, 撤销 walks the moved directories back.
 */
export async function migrationStatus(): Promise<MigrationJournal | null> {
  if (!isDesktop) return null
  return invoke<MigrationJournal | null>('storage_migration_status')
}

/** Undo a non-committed migration: everything that arrived moves back. */
export async function migrationUndo(): Promise<MoveSummary> {
  return invoke<MoveSummary>('storage_migration_undo')
}
