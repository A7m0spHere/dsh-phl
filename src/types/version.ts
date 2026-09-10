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
  /** A removal is walking the tree — the row shows it instead of lying idle. */
  | { kind: 'removing' }

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

/**
 * The wizard's "install it here" affordance: only from a resting state with
 * no operation running. `failed` counts because retry-in-place is the
 * wizard's own recovery path. One predicate (audit A) — the four hand-written
 * `=== 'available' || === 'failed'` copies it replaces would each have to be
 * remembered when a new resting state lands.
 */
export const isVersionInstallable = (v: DshVersion) =>
  v.state.kind === 'available' || v.state.kind === 'failed'

/**
 * The single source of truth for "an operation on this version is in flight".
 * Every busy / active / keep predicate derives from this list — the per-set
 * literals that used to live in catalogStore, the refresh merge, the wizard
 * and the pages each drifted the moment a kind was added
 * (`installing-deps` and `removing` both landed in some and not others).
 *
 * `removing` belongs here even though it never enters the transfer queue:
 * the tree walk is disk work that must count as busy for the data-root
 * migration guard and must disable the row's buttons.
 */
export const VERSION_TRANSITIONING_KINDS: readonly VersionInstallState['kind'][] = [
  'queued',
  'downloading',
  'extracting',
  'verifying',
  'installing-deps',
  'removing',
]

export const isVersionBusyKind = (kind: VersionInstallState['kind']) =>
  VERSION_TRANSITIONING_KINDS.includes(kind)

export const isVersionBusy = (v: DshVersion) => isVersionBusyKind(v.state.kind)

/**
 * Whether a refresh from disk may overwrite this row's optimistic state.
 * In-flight kinds lag the disk (the marker is written when the operation
 * completes); `failed` is kept too because the disk has no shape for it —
 * dropping it would flip a failed row back to "available" on the next poll.
 */
export const keepVersionStateOnRefresh = (kind: VersionInstallState['kind']) =>
  isVersionBusyKind(kind) || kind === 'failed'
