import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop, currentWindow, type Unlisten } from './desktopCore'
import { parseThrownError } from './errorCodes'
import type { ApiBinding } from '@/types'

/**
 * The launch half of the desktop bridge (§M2 domain split): spawn an instance,
 * re-adopt survivors of a restart, stop, cancel a pending launch, and subscribe
 * to the "process exited" window event. `onInstanceExited` uses the shared
 * `currentWindow`/`Unlisten` from `desktopCore` (no cycle back into
 * `desktop.ts`). Re-exported from `desktop.ts` so consumers are unchanged.
 */

/** Mirrors the Rust `LaunchEvent`; `stage` is a frontend `LaunchPhase`. */
export interface LaunchEventMsg {
  stage: string
  progress: number
  detail?: string | null
}

export interface LaunchInstanceArgs {
  transferId: string
  instanceId: string
  /** Bare DSH version directory name (id minus the `dsh-` prefix). */
  versionName: string
  /** `node-<major>` or `node-system`. */
  runtimeName: string
  /** Registry the launcher pulls missing version deps from, if any. */
  registryBase: string
  profile: string
  port: number
  autoPort: boolean
  env: Record<string, string>
  args: string[]
  /**
   * The store's binding, authoritative over a stale on-disk manifest at
   * launch (binding persistence is fire-and-forget). Rust re-aligns it.
   */
  api?: ApiBinding | null
  onProgress: (event: LaunchEventMsg) => void
}

export interface LaunchResult {
  pid: number
  port: number
  /** Authenticated `dsh web` URL read from the launch log, when available. */
  webUrl?: string
}

export async function launchInstance(args: LaunchInstanceArgs): Promise<LaunchResult> {
  if (!isDesktop) throw new Error('启动仅在桌面端可用')
  const channel = new Channel<LaunchEventMsg>()
  channel.onmessage = args.onProgress
  return invoke('launch_instance', {
    transferId: args.transferId,
    instanceId: args.instanceId,
    versionName: args.versionName,
    runtimeName: args.runtimeName,
    registryBase: args.registryBase,
    profile: args.profile,
    port: args.port,
    autoPort: args.autoPort,
    env: args.env,
    args: args.args,
    api: args.api ?? null,
    onProgress: channel,
  })
}

/** One managed child as the durable registry saw it at spawn time. */
export interface AdoptedProcess {
  instanceId: string
  pid: number
  port: number
  webUrl?: string | null
}

/** A previous session's process PHL declined to manage, with the reason.
 * `keptRunning` = it may still run; PHL will not stop it. */
export interface DroppedProcess {
  instanceId: string
  pid: number
  reason: string
  keptRunning: boolean
}

export interface AdoptReport {
  adopted: AdoptedProcess[]
  dropped: DroppedProcess[]
}

/**
 * Re-adopt DSH children that survived a PHL restart (O-08). Call once at
 * boot, after the instance list loaded. Unverifiable pids are reported but
 * never terminated.
 */
export async function adoptProcesses(): Promise<AdoptReport> {
  if (!isDesktop) return { adopted: [], dropped: [] }
  return invoke<AdoptReport>('adopt_processes')
}

export async function stopInstance(instanceId: string): Promise<void> {
  if (!isDesktop) return
  await invoke('stop_instance', { instanceId })
}

/** Cancels a launch still waiting for readiness; best-effort. */
export async function cancelLaunch(transferId: string): Promise<void> {
  if (!isDesktop) return
  try {
    await invoke('cancel_launch', { transferId })
  } catch {
    // The launch may have already finished.
  }
}

/**
 * A launched DSH process exited — crash, manual stop or clean shutdown. One
 * event drives all three, so the instance state never says "running" for a
 * dead process.
 */
export async function onInstanceExited(
  handler: (event: { instanceId: string; pid: number; code: number | null }) => void,
): Promise<Unlisten> {
  if (!isDesktop) return () => {}
  return currentWindow().listen<{ instanceId: string; pid: number; code: number | null }>(
    'phl://instance-exited',
    (event) => handler(event.payload),
  )
}

/* ------------------------------ embedded WebUI ------------------------------ */

/**
 * Open (or focus) the embedded WebUI window for one instance. Windows are
 * one-per-instance and labelled by id, so a second call on a running instance
 * focuses the existing window instead of spawning another.
 *
 * Closing the window never stops the instance — but the process exiting
 * always closes the window (Rust: `webui` module + the launch watcher).
 *
 * Fallbacks are part of the contract: the browser build has no window API so
 * it opens a new tab; on desktop, if the backend refuses the request (a
 * rejected non-loopback URL, a window-creation failure) we open the system
 * browser instead, so "打开 WebUI" always reaches a live DSH.
 */
/** How "打开 WebUI" reached the user, or why it could not. */
export type WebUiOpenResult =
  | { ok: true; how: 'window' | 'browser' }
  | { ok: false; reason: string }

export async function openDshWebUi(
  instanceId: string,
  url: string,
  title?: string,
): Promise<WebUiOpenResult> {
  if (!isDesktop) {
    window.open(url, '_blank', 'noopener')
    return { ok: true, how: 'browser' }
  }
  let windowError: unknown
  try {
    await invoke('open_or_focus_webui', { instanceId, url, title: title ?? null })
    return { ok: true, how: 'window' }
  } catch (err) {
    windowError = err
  }
  // The fallback is part of the contract, but so is admitting failure: when
  // both halves fail the caller must say something. Swallowing the first error
  // and letting the second reject into a `void` call site made "打开 WebUI" a
  // button that silently did nothing.
  try {
    await invoke('open_external', { url })
    return { ok: true, how: 'browser' }
  } catch (fallbackError) {
    return {
      ok: false,
      reason: `内嵌窗口失败：${parseThrownError(windowError).message}；改用系统浏览器也失败：${parseThrownError(fallbackError).message}`,
    }
  }
}
