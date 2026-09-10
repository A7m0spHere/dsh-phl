import { describe, expect, it } from 'vitest'
import {
  isVersionBusy,
  isVersionBusyKind,
  keepVersionStateOnRefresh,
  type DshVersion,
  type VersionInstallState,
} from './version'
import {
  isRuntimeBusy,
  isRuntimeBusyKind,
  keepRuntimeStateOnRefresh,
  type Runtime,
  type RuntimeInstallState,
} from './runtime'

/**
 * The drift guard. These records enumerate *every* kind of each union: a new
 * state kind — or a changed busy semantics — makes `tsc` fail right here
 * until the author decides how every predicate treats it. Before the shared
 * lists existed, `installing-deps` and `queued` (runtime) shipped into some
 * consumers and were silently missing from others.
 */
const versionBusy: Record<VersionInstallState['kind'], boolean> = {
  available: false,
  queued: true,
  downloading: true,
  extracting: true,
  verifying: true,
  'installing-deps': true,
  installed: false,
  failed: false,
  removing: true,
}

const runtimeBusy: Record<RuntimeInstallState['kind'], boolean> = {
  available: false,
  queued: true,
  downloading: true,
  verifying: true,
  extracting: true,
  installed: false,
  failed: false,
  removing: true,
}

const version = (state: VersionInstallState): DshVersion => ({ id: 'v', state } as DshVersion)
const runtime = (state: RuntimeInstallState): Runtime => ({ id: 'r', state } as Runtime)

describe('version state predicates', () => {
  it('isVersionBusy matches the explicit kind table', () => {
    for (const [kind, busy] of Object.entries(versionBusy)) {
      expect(isVersionBusyKind(kind as VersionInstallState['kind'])).toBe(busy)
      expect(isVersionBusy(version({ kind } as VersionInstallState))).toBe(busy)
    }
  })

  it('refresh keeps in-flight and failed rows, adopts disk truth otherwise', () => {
    for (const [kind, busy] of Object.entries(versionBusy)) {
      const keeps = keepVersionStateOnRefresh(kind as VersionInstallState['kind'])
      const expected = busy || kind === 'failed'
      expect(keeps).toBe(expected)
    }
    // `installed`/`available` come back from disk authoritative; `failed`
    // must survive the poll because disk has no shape for it.
    expect(keepVersionStateOnRefresh('failed')).toBe(true)
    expect(keepVersionStateOnRefresh('installed')).toBe(false)
  })
})

describe('runtime state predicates', () => {
  it('isRuntimeBusy covers queued (semaphore) and verifying (shasums)', () => {
    for (const [kind, busy] of Object.entries(runtimeBusy)) {
      expect(isRuntimeBusyKind(kind as RuntimeInstallState['kind'])).toBe(busy)
      expect(isRuntimeBusy(runtime({ kind } as RuntimeInstallState))).toBe(busy)
    }
  })

  it('refresh keeps in-flight and failed rows', () => {
    for (const [kind, busy] of Object.entries(runtimeBusy)) {
      expect(keepRuntimeStateOnRefresh(kind as RuntimeInstallState['kind'])).toBe(
        busy || kind === 'failed',
      )
    }
  })
})
