/**
 * A DSH release that PHL can install locally. Several versions coexist;
 * an Instance pins exactly one of them.
 */
export type VersionChannel = 'stable' | 'rc' | 'nightly'

export type VersionInstallState =
  | { kind: 'available' }
  | { kind: 'queued' }
  | { kind: 'downloading'; progress: number; bytesDone: number; bytesPerSec: number }
  | { kind: 'extracting'; progress: number }
  | { kind: 'verifying' }
  | { kind: 'installed'; installedAt: string }
  | { kind: 'failed'; reason: string }

export interface DshVersion {
  id: string
  /** Display name, e.g. `0.1.0-rc.7` */
  name: string
  channel: VersionChannel
  releasedAt: string
  /** Download size of the package in bytes. */
  size: number
  /** Node major versions this release is known to run on. */
  requiresNode: number[]
  notes: string[]
  /** Latest release on its channel. */
  latest?: boolean
  /** Kept for compatibility testing; PHL warns before removing it. */
  legacy?: boolean
  state: VersionInstallState
}

export const isVersionInstalled = (v: DshVersion) => v.state.kind === 'installed'
export const isVersionBusy = (v: DshVersion) =>
  v.state.kind === 'queued' ||
  v.state.kind === 'downloading' ||
  v.state.kind === 'extracting' ||
  v.state.kind === 'verifying'
