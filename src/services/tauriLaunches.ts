import * as desktop from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import { registryBase } from './tauriVersions'
import type { Instance, LaunchPhase } from '@/types'
import { Cancelled, LaunchError, newTransferId } from './repository'
import type { LaunchContext, LaunchProgress, LaunchOutcome, PhlRepository } from './repository'

/**
 * Desktop overrides for the **process / port module**: a launch spawns
 * `<runtime>/node <version>/lib/bin.js web --port <p> --no-open` with
 * `DSH_HOME=<instance>/dsh-home` (see src-tauri/src/launch.rs for why the
 * `web` subcommand is the only sound entry), and readiness is the port
 * accepting connections.
 */

/**
 * `dsh web` boots `$DSH_HOME/profiles/web` — the only profile that serves the
 * WebUI and parses `--port`. Plugins load from that same profile's
 * `node_modules`, so the installer's profile dir and the boot profile must be
 * one and the same.
 */
export const WEB_PROFILE = 'web'

function phlRoot(): string {
  return useSettingsStore.getState().root
}

async function launch(
  instance: Instance,
  ctx: LaunchContext,
  onProgress: (p: LaunchProgress) => void,
  signal: AbortSignal,
): Promise<LaunchOutcome> {
  // Preconditions live here, not in Rust: the states and the compatibility
  // matrix come from the catalog, and the failures read with the same titles
  // as the mock flow did.
  if (!ctx.version || ctx.version.state.kind !== 'installed') {
    throw new LaunchError(
      'DSH 版本未安装',
      `实例固定使用 ${ctx.version?.name ?? instance.versionId}，但它还没有安装到本机。`,
      '前往「版本」页面安装该版本后重试。',
    )
  }
  if (!ctx.runtime || ctx.runtime.state.kind !== 'installed') {
    throw new LaunchError(
      'Runtime 未安装',
      `实例需要 ${ctx.runtime?.name ?? instance.runtimeId}，但它还没有安装。`,
      '前往「运行时」页面安装后重试。',
    )
  }
  // An empty `requiresNode` means the registry never declared `engines.node`
  // — that is "unknown", not "nothing is compatible".
  if (ctx.version.requiresNode.length && !ctx.version.requiresNode.includes(ctx.runtime.major)) {
    throw new LaunchError(
      'Runtime 与 DSH 版本不匹配',
      `${ctx.version.name} 要求 Node ${ctx.version.requiresNode.join(' / ')}，当前实例绑定的是 ${ctx.runtime.name}。`,
      '在实例设置里更换 Runtime，或安装匹配的 Node 版本。',
    )
  }
  if (instance.profile !== WEB_PROFILE) {
    throw new LaunchError(
      '暂不支持从该 profile 启动',
      `PHL 以 DSH 的 web 面启动实例（只有它解析 --port），而实例的 profile 是「${instance.profile}」。`,
      '把实例的 profile 改回 web 后重试。',
    )
  }
  // 固定端口的冲突在进入 Rust 前就给出带实例名的报错；autoPort 的实例交给
  // Rust 向上扫描，这里不拦。
  if (!instance.autoPort && ctx.portsInUse.has(instance.port)) {
    throw new LaunchError(
      '端口被占用',
      `端口 ${instance.port} 正在被实例「${ctx.portsInUse.get(instance.port)}」使用。`,
      '开启自动分配端口，或先停止占用该端口的实例。',
    )
  }

  const transferId = newTransferId(`l:${instance.id}`)
  if (signal.aborted) throw new Cancelled()
  const onAbort = () => void desktop.cancelLaunch(transferId)
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    // The two resolve phases are real checks that already happened above —
    // narrate them briefly so the timeline reads the same as the mock flow.
    onProgress({ phase: 'resolve-version', progress: 0.04 })
    onProgress({ phase: 'resolve-runtime', progress: 0.08 })
    const outcome = await desktop.launchInstance({
      transferId,
      root: phlRoot(),
      instanceId: instance.id,
      versionName: instance.versionId.replace(/^dsh-/, ''),
      runtimeName: instance.runtimeId,
      // The launcher self-heals a version that predates dependency installs;
      // it needs the same registry the catalog uses.
      registryBase: registryBase(),
      profile: instance.profile,
      port: instance.port,
      autoPort: instance.autoPort,
      env: instance.env,
      args: instance.args,
      // Rust re-aligns a stale on-disk binding with this one before spawn
      // (the binding persistence path is fire-and-forget).
      api: instance.api ?? null,
      onProgress: (e) =>
        onProgress({ phase: e.stage as LaunchPhase, progress: e.progress, detail: e.detail ?? undefined }),
    })
    return outcome
  } catch (err) {
    if (signal.aborted) throw new Cancelled()
    if (err instanceof LaunchError) throw err
    throw new LaunchError('启动失败', err instanceof Error ? err.message : String(err))
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

/** The process module's overrides, spread into the desktop repository. */
export const tauriLaunchOverrides: Pick<PhlRepository, 'launch' | 'stop'> = {
  launch,
  stop: async (instance) => {
    await desktop.stopInstance(instance.id)
  },
}
