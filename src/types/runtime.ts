/**
 * A Node runtime managed by PHL. Runtimes are decoupled from DSH versions:
 * an Instance picks a version *and* a runtime independently.
 */
export type RuntimeInstallState =
  | { kind: 'available' }
  /** Waiting for a transfer slot (the concurrency cap is enforced now). */
  | { kind: 'queued' }
  | { kind: 'downloading'; progress: number; bytesDone: number; bytesPerSec: number }
  /** SHASUMS fetch + checksum compare — real work, no ratio to show. */
  | { kind: 'verifying' }
  | { kind: 'extracting'; progress: number }
  | { kind: 'installed'; installedAt: string }
  | { kind: 'failed'; reason: string }
  /** A removal is walking the tree — the row shows it instead of lying idle. */
  | { kind: 'removing' }

/**
 * The single source of truth for "an operation on this runtime is in
 * flight" — the same rule as `VERSION_TRANSITIONING_KINDS`, in its own list
 * so adding a runtime-only kind cannot silently widen the version one.
 * (`queued` and `verifying` were missing from parts of the frontend while
 * installs waited on the transfer-slot semaphore.)
 */
export const RUNTIME_TRANSITIONING_KINDS: readonly RuntimeInstallState['kind'][] = [
  'queued',
  'downloading',
  'verifying',
  'extracting',
  'removing',
]

export const isRuntimeBusyKind = (kind: RuntimeInstallState['kind']) =>
  RUNTIME_TRANSITIONING_KINDS.includes(kind)

export const isRuntimeBusy = (r: Runtime) => isRuntimeBusyKind(r.state.kind)

/**
 * A legacy install/binding id: `node-<digits>` only. Precise ids
 * (`node-22.12.0`) and `node-system` are not legacy. Mirrors the Rust
 * `legacy_major_id` so UI affordances and backend rules agree.
 */
export const isLegacyRuntimeId = (id: string) => /^node-\d+$/.test(id)

/** See `isVersionInstallable` — the wizard's mirrored rule for runtimes. */
export const isRuntimeInstallable = (r: Runtime) =>
  r.state.kind === 'available' || r.state.kind === 'failed'

/** See `keepVersionStateOnRefresh`. */
export const keepRuntimeStateOnRefresh = (kind: RuntimeInstallState['kind']) =>
  isRuntimeBusyKind(kind) || kind === 'failed'

export interface Runtime {
  id: string
  /** e.g. `Node 22` */
  name: string
  major: number
  /** Full semver of the bundled build, e.g. `22.11.0` */
  version: string
  /** LTS codename, when it has one. */
  codename?: string
  lts: boolean
  size: number
  /** Discovered on PATH rather than installed by PHL. */
  system?: boolean
  /** Canonical executable path for the discovered system Node. */
  path?: string
  /**
   * True for legacy install directories named `node-<major>`: they predate
   * precise bindings. Such rows still launch, and an instance bound to one
   * can convert to a precise runtime (see `previewRuntimeConversion`).
   */
  legacy?: boolean
  /** Recorded by precise installs; legacy rows carry none. */
  platform?: string | null
  arch?: string | null
  state: RuntimeInstallState
}
