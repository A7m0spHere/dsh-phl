/**
 * A DSH release that PHL can install locally. Several versions coexist;
 * an Instance pins exactly one of them.
 */
export type VersionChannel = 'stable' | 'rc' | 'alpha' | 'nightly'

/** Where the package can actually be fetched from (npm registry today). */
export interface VersionSource {
  tarball: string
  /** npm `dist.integrity`, e.g. `sha512-<base64>`; verified after download. */
  integrity?: string
}

export type VersionInstallState =
  | { kind: 'available' }
  | { kind: 'queued' }
  | { kind: 'downloading'; progress: number; bytesDone: number; bytesPerSec: number }
  | { kind: 'extracting'; progress: number }
  | { kind: 'verifying' }
  /** npm is installing the version's own dependencies (the long cold-install stage). */
  | { kind: 'installing-deps'; progress: number }
  | {
      kind: 'installed'
      installedAt: string
      /** `degraded` = some dependencies were pruned during install. */
      installHealth?: 'healthy' | 'degraded'
      skippedDependencies?: string[]
    }
  | { kind: 'failed'; reason: string }

export interface DshVersion {
  id: string
  /** Display name, e.g. `0.1.0-rc.7` */
  name: string
  channel: VersionChannel
  releasedAt: string
  /** Download size of the package in bytes. */
  size: number
  /**
   * Node major versions this release is known to run on, from the package's
   * `engines.node`. **Empty means unknown, not "none"** — a version whose
   * registry entry declares no engines must not be treated as incompatible
   * with every runtime.
   */
  requiresNode: number[]
  notes: string[]
  /** Latest release on its channel. */
  latest?: boolean
  /** Kept for compatibility testing; PHL warns before removing it. */
  legacy?: boolean
  /**
   * Released on GitHub but not yet published to the npm registry — shown so
   * the list tracks GitHub's pace; there is nothing to install yet.
   */
  pendingPublish?: boolean
  /** Present when the version is downloadable from a real registry. */
  source?: VersionSource
  state: VersionInstallState
}

export const isVersionInstalled = (v: DshVersion) => v.state.kind === 'installed'
export const isVersionBusy = (v: DshVersion) =>
  v.state.kind === 'queued' ||
  v.state.kind === 'downloading' ||
  v.state.kind === 'extracting' ||
  v.state.kind === 'verifying'
