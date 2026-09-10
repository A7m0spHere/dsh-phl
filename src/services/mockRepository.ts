import { instanceSeed, templateSeed } from '@/data/instances'
import { pluginSeed } from '@/data/plugins'
import { runtimeSeed } from '@/data/runtimes'
import { versionSeed } from '@/data/versions'
import { slugify } from '@/lib/format'
import { useSettingsStore } from '@/stores/settingsStore'
import type {
  DshVersion,
  InstalledPlugin,
  Instance,
  InstanceDraft,
  InstanceTemplate,
  LaunchPhase,
  Plugin,
  PluginTrust,
  Runtime,
  Snapshot,
} from '@/types'
import { LAUNCH_PHASES } from '@/types'
import {
  Cancelled,
  LaunchError,
  type CopyProgress,
  type CreateProgress,
  type LaunchContext,
  type LaunchOutcome,
  type LaunchProgress,
  type PhlRepository,
  type TransferProgress,
} from './repository'

/* ------------------------------------------------------------------ *
 * simulation helpers
 * ------------------------------------------------------------------ */

const TICK = 60

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new Cancelled())
    const id = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, ms)
    const onAbort = () => {
      clearTimeout(id)
      reject(new Cancelled())
    }
    signal?.addEventListener('abort', onAbort, { once: true })
  })
}

/** Ease-out so a bar decelerates near the end instead of snapping. */
const easeOut = (t: number) => 1 - Math.pow(1 - t, 2.2)

/**
 * Drives `onTick(0..1)` over `duration` ms. Real work is never perfectly
 * linear, so the curve is eased and jittered a little; this is what keeps a
 * progress bar from looking synthetic.
 */
async function ramp(
  duration: number,
  onTick: (p: number) => void,
  signal?: AbortSignal,
  ease: (t: number) => number = easeOut,
): Promise<void> {
  const start = performance.now()
  for (;;) {
    if (signal?.aborted) throw new Cancelled()
    const elapsed = performance.now() - start
    const raw = Math.min(1, elapsed / duration)
    onTick(Math.min(1, ease(raw)))
    if (raw >= 1) return
    await sleep(TICK, signal)
  }
}

const rand = (min: number, max: number) => min + Math.random() * (max - min)

/**
 * The instance tree under PHL's *configured* data root.
 *
 * These paths are no longer decorative: the desktop repository still routes
 * `createInstance` here while the instance module is mock, and the real
 * plugin installer then creates directories under whatever `dshHome` says.
 * Hardcoding the prototype's `C:\Users\dev\…` placeholder meant every plugin
 * install on a real machine wrote to — or failed on — a stranger's path.
 */
function instanceRoot(slug: string): string {
  const root = useSettingsStore.getState().root
  const sep = root.includes('\\') ? '\\' : '/'
  return `${root}${sep}instances${sep}${slug}`
}

const childPath = (root: string, name: string) =>
  `${root}${root.includes('\\') ? '\\' : '/'}${name}`

/* ------------------------------------------------------------------ *
 * repository
 * ------------------------------------------------------------------ */

class MockRepository implements PhlRepository {
  async enrichModelMetadata(args: import('@/types').ModelMetadataRequest): Promise<import('@/types').ModelMetadataBatch> {
    return { results: args.models.map((model) => ({ model, matched: false, changed: false, ambiguous: false })), catalogStatus: 'mock' }
  }
  async listInstances() {
    await sleep(120)
    return structuredClone(instanceSeed)
  }
  async listVersions() {
    await sleep(90)
    return structuredClone(versionSeed)
  }
  async listRuntimes() {
    await sleep(80)
    return structuredClone(runtimeSeed)
  }
  async listPlugins() {
    await sleep(110)
    return { plugins: structuredClone(pluginSeed) }
  }
  async listTemplates() {
    await sleep(40)
    return structuredClone(templateSeed)
  }

  /* ---------------- instance lifecycle ---------------- */

  async createInstance(
    draft: InstanceDraft,
    template: InstanceTemplate | undefined,
    onProgress: (p: CreateProgress) => void,
    signal: AbortSignal,
  ): Promise<Instance> {
    const slug = slugify(draft.name)
    const root = instanceRoot(slug)

    const steps: [CreateProgress['step'], number, string][] = [
      ['create', 380, '写入 instance.json'],
      ['environment', 900, '解压 DSH 与 Runtime 到实例目录'],
      ['configure', 620, '写入 DSH_HOME 与端口配置'],
      ['plugins', template && template.plugins.length ? 780 : 240, '安装模板插件'],
    ]

    const total = steps.reduce((a, s) => a + s[1], 0)
    let done = 0
    for (const [step, ms, detail] of steps) {
      const base = done
      await ramp(
        ms,
        (p) => onProgress({ step, progress: (base + p * ms) / total, detail }),
        signal,
      )
      done += ms
    }
    onProgress({ step: 'done', progress: 1 })
    await sleep(220, signal)

    return {
      id: `${slug}-${Math.random().toString(36).slice(2, 6)}`,
      name: draft.name.trim(),
      note: draft.note.trim() || undefined,
      kind: draft.kind,
      hue: draft.hue,
      versionId: draft.versionId!,
      runtimeId: draft.runtimeId!,
      port: draft.port,
      autoPort: draft.autoPort,
      dshHome: childPath(root, 'dsh-home'),
      workspace: childPath(root, 'workspace'),
      // 与桌面端一致：`dsh web` 启动的就是 profiles/web。
      profile: 'web',
      createdAt: new Date().toISOString(),
      totalRuntime: 0,
      diskUsage: 24_000_000 + (template?.plugins.length ?? 0) * 8_400_000,
      env: {},
      args: [],
      plugins: (template?.plugins ?? []).map((pluginId) => {
        const plugin = pluginSeed.find((p) => p.id === pluginId)
        return { pluginId, version: plugin?.releases[0].version ?? '1.0.0', enabled: true }
      }),
      snapshots: [],
    }
  }

  async cloneInstance(source: Instance, name: string, port: number): Promise<Instance> {
    await sleep(900)
    const slug = slugify(name)
    const root = instanceRoot(slug)
    return {
      ...structuredClone(source),
      id: `${slug}-${Math.random().toString(36).slice(2, 6)}`,
      name,
      note: `从 ${source.name} 克隆`,
      port,
      dshHome: childPath(root, 'dsh-home'),
      workspace: childPath(root, 'workspace'),
      createdAt: new Date().toISOString(),
      lastRunAt: undefined,
      totalRuntime: 0,
      favorite: false,
      snapshots: [],
    }
  }

  async deleteInstance(_id: string): Promise<void> {
    await sleep(560)
  }

  async saveInstance(_instance: Instance): Promise<void> {
    // Nothing to persist in the browser: the mock instance list only ever
    // lives for the length of the session.
  }

  async measureDiskUsage(instance: Instance): Promise<number> {
    return instance.diskUsage
  }

  /* ---------------- snapshots ---------------- */

  async createSnapshot(
    instance: Instance,
    onProgress: (p: CopyProgress) => void,
    signal: AbortSignal,
  ): Promise<Snapshot> {
    // 模拟本地树拷贝：一段平滑的进度爬坡。
    const size = 58_000_000
    await ramp(900, (p) => onProgress({ progress: p, bytesDone: Math.round(size * p), bytesTotal: size }), signal)
    return {
      id: `snap-${Date.now()}`,
      label: `快照 ${new Date().toISOString().slice(0, 16).replace('T', ' ')}`,
      createdAt: new Date().toISOString(),
      versionId: instance.versionId,
      runtimeId: instance.runtimeId,
      pluginCount: instance.plugins.length,
      size,
    }
  }

  async restoreSnapshot(
    instance: Instance,
    snapshotId: string,
    onProgress: (p: CopyProgress) => void,
    signal: AbortSignal,
  ): Promise<Instance> {
    // Mirrors the desktop path: the copy is the visible leg and is
    // cancellable; the mock "swap" is the returned record.
    void snapshotId
    const size = 58_000_000
    await ramp(900, (p) => onProgress({ progress: p, bytesDone: Math.round(size * p), bytesTotal: size }), signal)
    return instance
  }

  async deleteSnapshot(instance: Instance, snapshotId: string): Promise<void> {
    await sleep(300)
    void snapshotId
    void instance
  }

  async listOrphanInstanceDirs(): Promise<{ name: string; size: number }[]> {
    return []
  }

  async removeOrphanInstanceDir(_name: string): Promise<void> {}

  /* ---------------- launch / stop ---------------- */

  async launch(
    instance: Instance,
    ctx: LaunchContext,
    onProgress: (p: LaunchProgress) => void,
    signal: AbortSignal,
  ): Promise<LaunchOutcome> {
    // Weights double as phase durations; `link-plugins` scales with the real
    // plugin count so a heavy instance visibly takes longer to come up.
    const weights: Record<LaunchPhase, number> = {
      'resolve-version': 280,
      'resolve-runtime': 300,
      'prepare-home': 420,
      'link-plugins': 260 + instance.plugins.length * 46,
      'allocate-port': 240,
      spawn: 520,
      'await-ready': 860,
      // The mock catalog has no npm pipeline behind it — the phase only ever
      // runs on desktop, repairing a version whose deps are missing.
      'install-deps': 0,
    }
    const total = LAUNCH_PHASES.reduce((a, p) => a + weights[p], 0)

    let elapsed = 0
    let port = instance.port

    for (const phase of LAUNCH_PHASES) {
      if (phase === 'install-deps') continue
      // Preconditions are checked at the top of the phase that owns them, so
      // the failure surfaces with the right label attached to it.
      if (phase === 'resolve-version') {
        if (!ctx.version || ctx.version.state.kind !== 'installed') {
          await sleep(340, signal)
          throw new LaunchError(
            'DSH 版本未安装',
            `实例固定使用 ${ctx.version?.name ?? instance.versionId}，但它还没有安装到本机。`,
            '前往「版本」页面安装该版本后重试。',
          )
        }
      }
      if (phase === 'resolve-runtime') {
        if (!ctx.runtime || ctx.runtime.state.kind !== 'installed') {
          await sleep(300, signal)
          throw new LaunchError(
            'Runtime 未安装',
            `实例需要 ${ctx.runtime?.name ?? instance.runtimeId}，但它还没有安装。`,
            '前往「运行时」页面安装后重试。',
          )
        }
        // An empty `requiresNode` means the registry never declared
        // `engines.node` — that is "unknown", not "nothing is compatible".
        // Treating it as a constraint rejected every launch on desktop, with
        // an empty Node list in the message to prove it.
        if (ctx.version?.requiresNode.length && !ctx.version.requiresNode.includes(ctx.runtime.major)) {
          await sleep(300, signal)
          throw new LaunchError(
            'Runtime 与 DSH 版本不匹配',
            `${ctx.version.name} 要求 Node ${ctx.version.requiresNode.join(' / ')}，当前实例绑定的是 ${ctx.runtime.name}。`,
            '在实例设置里更换 Runtime，或安装匹配的 Node 版本。',
          )
        }
      }
      if (phase === 'allocate-port') {
        if (instance.autoPort) {
          port = instance.port
          while (ctx.portsInUse.has(port)) port += 1
        } else if (ctx.portsInUse.has(instance.port)) {
          await sleep(260, signal)
          throw new LaunchError(
            '端口被占用',
            `端口 ${instance.port} 正在被实例「${ctx.portsInUse.get(instance.port)}」使用。`,
            '开启自动分配端口，或先停止占用该端口的实例。',
          )
        }
      }

      const ms = weights[phase]
      const base = elapsed
      const detail =
        phase === 'link-plugins'
          ? `${instance.plugins.filter((p) => p.enabled).length} 个插件`
          : phase === 'allocate-port'
            ? `:${port}`
            : undefined

      await ramp(
        ms,
        (p) => onProgress({ phase, progress: (base + p * ms) / total, detail }),
        signal,
      )
      elapsed += ms
    }

    return { pid: Math.floor(rand(4000, 39000)), port }
  }

  async stop(instance: Instance, signal: AbortSignal): Promise<void> {
    await sleep(420 + instance.plugins.length * 22, signal)
  }

  /* ---------------- downloads ---------------- */

  private async transfer(
    size: number,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
    withVerify: boolean,
  ) {
    const speed = rand(11_000_000, 19_000_000)
    const downloadMs = Math.max(1400, (size / speed) * 1000)

    await ramp(
      downloadMs,
      (p) =>
        onProgress({
          stage: 'downloading',
          progress: p * 0.78,
          bytesDone: Math.round(size * p),
          // jitter the reported rate so it reads like a real connection
          bytesPerSec: speed * rand(0.82, 1.16),
        }),
      signal,
      (t) => t,
    )

    await ramp(
      760,
      (p) =>
        onProgress({ stage: 'extracting', progress: 0.78 + p * 0.18, bytesDone: size, bytesPerSec: 0 }),
      signal,
    )

    if (withVerify) {
      onProgress({ stage: 'verifying', progress: 0.97, bytesDone: size, bytesPerSec: 0 })
      await sleep(420, signal)
    }
  }

  async installVersion(
    version: DshVersion,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ) {
    await this.transfer(version.size, onProgress, signal, true)
  }

  async removeVersion(_id: string) {
    await sleep(480)
  }

  async installRuntime(
    runtime: Runtime,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ) {
    await this.transfer(runtime.size, onProgress, signal, false)
  }

  async removeRuntime(_id: string) {
    await sleep(420)
  }

  /* ---------------- plugin install pipeline ---------------- */

  /**
   * Mirrors the real Rust pipeline stage for stage: resolve the tarball
   * (Preparing), stream it (Downloading), hash it (Verifying), unpack into
   * the profile and register it in `cordis.patch.yml` (Installing).
   */
  async installPlugin(
    plugin: Plugin,
    instance: Instance,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ): Promise<{ version: string; registryId?: string; trust?: PluginTrust }> {
    const release = plugin.releases[0]
    const size = release?.size ?? 1_800_000
    const speed = rand(6_000_000, 11_000_000)
    const downloadMs = Math.max(1200, (size / speed) * 1000)

    onProgress({ stage: 'preparing', progress: 0, bytesDone: 0, bytesPerSec: 0 })
    await ramp(900, (p) => onProgress({ stage: 'preparing', progress: p * 0.05, bytesDone: 0, bytesPerSec: 0 }), signal)

    await ramp(
      downloadMs,
      (p) =>
        onProgress({
          stage: 'downloading',
          progress: 0.05 + p * 0.7,
          bytesDone: Math.round(size * p),
          bytesPerSec: speed * rand(0.82, 1.16),
        }),
      signal,
      (t) => t,
    )

    onProgress({ stage: 'verifying', progress: 0.78, bytesDone: size, bytesPerSec: 0 })
    await sleep(520, signal)

    await ramp(
      820,
      (p) =>
        onProgress({ stage: 'installing', progress: 0.78 + p * 0.22, bytesDone: size, bytesPerSec: 0 }),
      signal,
    )
    // Touch the profile the way the Rust step would — the mock instance tree
    // is virtual, so the delay is the only observable.
    await sleep(180, signal)
    void instance
    // The mock simulates the desktop path's common case: an npm release
    // pinned to an exact version with a verified integrity.
    return { version: release?.version ?? '0.0.0', trust: 'verified' }
  }

  async setPluginEnabled(
    instance: Instance,
    installed: InstalledPlugin,
    enabled: boolean,
  ): Promise<void> {
    await sleep(240)
    void instance
    void installed
    void enabled
  }

  async uninstallPlugin(instance: Instance, installed: InstalledPlugin): Promise<void> {
    await sleep(520)
    void instance
    void installed
  }

  async latestPluginVersion(plugin: Plugin): Promise<string | null> {
    await sleep(160)
    // GitHub-source plugins have no npm dist-tags to consult — the real
    // bridge returns null for them too.
    if (plugin.source.kind !== 'npm') return null
    return plugin.releases[0]?.version ?? null
  }
}

export const mockRepository: PhlRepository = new MockRepository()
export type { Plugin }
