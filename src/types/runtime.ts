/**
 * A Node runtime managed by PHL. Runtimes are decoupled from DSH versions:
 * an Instance picks a version *and* a runtime independently.
 */
export type RuntimeInstallState =
  | { kind: 'available' }
  /** Waiting for a transfer slot (the concurrency cap is enforced now). */
  | { kind: 'queued' }
  | { kind: 'downloading'; progress: number; bytesDone: number; bytesPerSec: number }
  | { kind: 'extracting'; progress: number }
  | { kind: 'installed'; installedAt: string }
  | { kind: 'failed'; reason: string }

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
  state: RuntimeInstallState
}
