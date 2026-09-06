import { useCallback, useMemo } from 'react'
import {
  Copy,
  FolderOpen,
  Package,
  Pencil,
  Play,
  Square,
  Camera,
  Download,
  Star,
  Trash2,
  ExternalLink,
} from 'lucide-react'
import type { MenuItem } from '@/components/ui'
import {
  chooseSaveFile,
  exportInstanceBundle,
  openExternal,
  previewInstanceExport,
  revealPath,
} from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import { useInstanceStore, useUIStore } from '@/stores'
import type { Instance } from '@/types'

/**
 * Every surface that can act on an instance — card, detail header, quick
 * switcher — goes through here, so a "clone" started from a list behaves
 * exactly like one started from the detail page.
 */
export function useInstanceActions(instance: Instance | undefined) {
  const store = useInstanceStore()
  const ui = useUIStore()
  const id = instance?.id

  const clone = useCallback(async () => {
    if (!instance) return
    const name = await ui.prompt({
      title: '克隆实例',
      label: '新实例名称',
      defaultValue: `${instance.name} Copy`,
      helper: '版本、Runtime、插件与配置会被完整复制，端口自动分配。',
      confirmLabel: '克隆',
      validate: (v) =>
        !v.trim()
          ? '名称不能为空'
          : store.instances.some((i) => i.name === v.trim())
            ? '已存在同名实例'
            : null,
    })
    if (name) await store.cloneInstance(instance.id, name)
  }, [instance, store, ui])

  const rename = useCallback(async () => {
    if (!instance) return
    const name = await ui.prompt({
      title: '重命名实例',
      label: '实例名称',
      defaultValue: instance.name,
      confirmLabel: '保存',
      validate: (v) =>
        !v.trim()
          ? '名称不能为空'
          : store.instances.some((i) => i.name === v.trim() && i.id !== instance.id)
            ? '已存在同名实例'
            : null,
    })
    if (name) store.updateInstance(instance.id, { name })
  }, [instance, store, ui])

  const remove = useCallback(async () => {
    if (!instance) return
    const status = store.stateOf(instance.id).status
    // starting/stopping 也不放行：删除会和正在中止的启动过程赛跑。
    if (status !== 'stopped' && status !== 'error') {
      ui.toast({
        kind: 'warn',
        title: '实例忙',
        message: '请先停止实例（或等启动 / 停止结束），再执行删除。',
      })
      return
    }
    // An external instance's DSH_HOME is the user's own directory, outside
    // PHL's tree — deletion can only ever mean "stop managing it" (spec §3.1).
    const external = instance.managementMode === 'external'
    const ok = ui.confirmDelete
      ? await ui.confirm({
          title: `删除实例「${instance.name}」`,
          message: external
            ? '这是原地接入的实例：只会从 PHL 移除登记，你的 DSH_HOME 与其中所有内容都不会被动到。'
            : '该实例独占的 DSH_HOME、插件、Profile 与 workspace 都会被一并删除，且不可恢复。',
          detail: instance.dshHome,
          tone: external ? 'default' : 'danger',
          confirmLabel: external ? '从 PHL 移除' : '永久删除',
          typeToConfirm: external ? undefined : instance.name,
        })
      : true
    if (ok) {
      if (ui.route.name === 'instance' && ui.route.id === instance.id) ui.navigate({ name: 'instances' })
      await store.deleteInstance(instance.id)
    }
  }, [instance, store, ui])

  /** Snapshot creation and progress are owned by the instance store. */
  const snapshot = useCallback(async () => {
    if (!instance || !id) return
    await store.createSnapshot(id)
  }, [instance, id, store])

  const copyPort = useCallback(() => {
    if (!instance) return
    const url = webUrl()
    void navigator.clipboard?.writeText(url)
    ui.toast({ kind: 'info', title: '已复制访问地址', message: url, duration: 2600 })
  }, [instance, ui])

  const revealFolder = useCallback(() => {
    if (!instance) return
    const root = useSettingsStore.getState().root
    // 正斜杠在 explorer / open / xdg-open 下都有效，不用按平台拼接。
    void revealPath([root, 'instances', instance.id].join('/')).catch((err) => {
      ui.toast({
        kind: 'error',
        title: '无法打开实例目录',
        message: err instanceof Error ? err.message : String(err),
      })
    })
  }, [instance, ui])

  const webUrl = (): string => {
    // The launch-captured URL carries DSH's per-boot auth token; the bare
    // host:port only works if the log line was never found.
    const state = useInstanceStore.getState().stateOf(instance!.id)
    return state.webUrl ?? `http://localhost:${instance!.port}`
  }

  const openWebUI = useCallback(() => {
    if (!instance) return
    void openExternal(webUrl())
  }, [instance])

  const exportBundle = useCallback(async () => {
    if (!instance) return
    try {
      // The preview is the promise: exactly these fields stay out of the
      // file, so sharing it is a decision the user makes with full knowledge.
      const report = await previewInstanceExport(instance.id)
      const omitted: string[] = []
      if (report.credentials.length > 0) {
        omitted.push(`凭据值，导入后需重新配置：${report.credentials.join('、')}`)
      }
      if (report.machineOnly.length > 0) {
        omitted.push(`机器本地变量：${report.machineOnly.join('、')}`)
      }
      const ok = await ui.confirm({
        title: `导出「${instance.name}」`,
        message: 'Bundle 携带实例配置与插件记录；插件文件不打包，导入后重新安装。',
        detail:
          omitted.length > 0
            ? `以下内容不会写入 Bundle：${omitted.join('；')}。`
            : undefined,
        confirmLabel: '导出',
      })
      if (!ok) return
      const dest = await chooseSaveFile('导出 PHL Bundle', `${instance.name}.phl-bundle.json`, [
        { name: 'PHL Bundle', extensions: ['json'] },
      ])
      if (!dest) return
      const written = await exportInstanceBundle(instance.id, dest)
      ui.toast({
        kind: 'success',
        title: '已导出 Bundle',
        message:
          written.credentials.length > 0
            ? `${dest}（凭据值已省略：${written.credentials.join('、')}）`
            : dest,
      })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '导出 Bundle 失败',
        message: err instanceof Error ? err.message : String(err),
      })
    }
  }, [instance, ui])

  const menuItems = useMemo<MenuItem[]>(() => {
    if (!instance || !id) return []
    const status = store.stateOf(id).status
    const running = status === 'running'
    // The backend gates these too (clone of an external home, snapshot
    // restore, plugin writes); the menu explains instead of letting the user
    // click into an error.
    const external = instance.managementMode === 'external'
    return [
      {
        id: 'toggle',
        label: running ? '停止实例' : '启动实例',
        icon: running ? <Square size={13} /> : <Play size={13} />,
        onSelect: () => store.toggle(id),
      },
      {
        id: 'open',
        label: '打开 WebUI',
        icon: <ExternalLink size={13} />,
        disabled: !running,
        onSelect: openWebUI,
      },
      {
        id: 'favorite',
        label: instance.favorite ? '取消置顶' : '置顶',
        icon: <Star size={13} />,
        separated: true,
        onSelect: () => store.toggleFavorite(id),
      },
      { id: 'rename', label: '重命名', icon: <Pencil size={13} />, onSelect: rename },
      {
        id: 'clone',
        label: external ? '克隆实例（原地接入不支持）' : '克隆实例',
        icon: <Copy size={13} />,
        disabled: external,
        onSelect: clone,
      },
      { id: 'export', label: '导出 Bundle', icon: <Package size={13} />, onSelect: exportBundle },
      {
        id: 'export-pack',
        label: '导出整合包',
        icon: <Download size={13} />,
        onSelect: () => ui.push({ name: 'exportPack', id }),
      },
      {
        id: 'snapshot',
        label: external ? '创建快照（原地接入不支持）' : '创建快照',
        icon: <Camera size={13} />,
        disabled: external,
        onSelect: snapshot,
      },
      {
        id: 'folder',
        label: '打开实例目录',
        icon: <FolderOpen size={13} />,
        separated: true,
        onSelect: revealFolder,
      },
      {
        id: 'delete',
        label: '删除实例',
        icon: <Trash2 size={13} />,
        danger: true,
        separated: true,
        onSelect: remove,
      },
    ]
  }, [instance, id, store, rename, clone, snapshot, remove, revealFolder, openWebUI])

  return { clone, rename, remove, snapshot, copyPort, revealFolder, openWebUI, exportBundle, menuItems }
}
