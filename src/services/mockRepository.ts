import { instanceSeed, templateSeed, PHL_ROOT } from '@/data/instances'
import { pluginSeed } from '@/data/plugins'
import { runtimeSeed } from '@/data/runtimes'
import { versionSeed } from '@/data/versions'
import { slugify } from '@/lib/format'
import type {
  DshVersion,
  Instance,
  InstanceDraft,
  InstanceTemplate,
  LaunchPhase,
  Plugin,
  Runtime,
} from '@/types'
import { LAUNCH_PHASES } from '@/types'
import {
  Cancelled,
  LaunchError,
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

/* ------------------------------------------------------------------ *
 * repository
 * ------------------------------------------------------------------ */

class MockRepository implements PhlRepository {
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
    return structuredClone(pluginSeed)
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
    const root = `${PHL_ROOT}\\instances\\${slug}`

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
      dshHome: `${root}\\dsh-home`,
      workspace: `${root}\\workspace`,
      profile: 'default',
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
    const root = `${PHL_ROOT}\\instances\\${slug}`
    return {
      ...structuredClone(source),
      id: `${slug}-${Math.random().toString(36).slice(2, 6)}`,
      name,
      note: `从 ${source.name} 克隆`,
      port,
      dshHome: `${root}\\dsh-home`,
      workspace: `${root}\\workspace`,
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
    }
    const total = LAUNCH_PHASES.reduce((a, p) => a + weights[p], 0)

    let elapsed = 0
    let port = instance.port

    for (const phase of LAUNCH_PHASES) {
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
        if (ctx.version && !ctx.version.requiresNode.includes(ctx.runtime.major)) {
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
}

export const mockRepository: PhlRepository = new MockRepository()
export type { Plugin }
