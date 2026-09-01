import { useCallback, useMemo } from 'react'
import {
  Copy,
  FolderOpen,
  Pencil,
  Play,
  Square,
  Camera,
  Star,
  Trash2,
  ExternalLink,
} from 'lucide-react'
import type { MenuItem } from '@/components/ui'
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
    if (store.stateOf(instance.id).status === 'running') {
      ui.toast({
        kind: 'warn',
        title: '实例正在运行',
        message: '请先停止实例，再执行删除。',
      })
      return
    }
    const ok = ui.confirmDelete
      ? await ui.confirm({
          title: `删除实例「${instance.name}」`,
          message: '该实例独占的 DSH_HOME、插件、Profile 与 workspace 都会被一并删除，且不可恢复。',
          detail: instance.dshHome,
          tone: 'danger',
          confirmLabel: '永久删除',
          typeToConfirm: instance.name,
        })
      : true
    if (ok) {
      if (ui.route.name === 'instance' && ui.route.id === instance.id) ui.navigate({ name: 'instances' })
      await store.deleteInstance(instance.id)
    }
  }, [instance, store, ui])

  const snapshot = useCallback(async () => {
    if (!instance) return
    const label = await ui.prompt({
      title: '创建快照',
      label: '快照名称',
      defaultValue: `${new Date().toLocaleDateString('zh-CN')} 基线`,
      helper: '记录当前 DSH 版本、Runtime、插件版本与配置，可随时回滚。',
      confirmLabel: '创建',
    })
    if (!label) return
    store.updateInstance(instance.id, {
      snapshots: [
        ...instance.snapshots,
        {
          id: `snap-${Date.now()}`,
          label,
          createdAt: new Date().toISOString(),
          versionId: instance.versionId,
          runtimeId: instance.runtimeId,
          pluginCount: instance.plugins.length,
          size: instance.diskUsage,
        },
      ],
    })
    ui.toast({ kind: 'success', title: `已创建快照「${label}」` })
  }, [instance, store, ui])

  const copyPort = useCallback(() => {
    if (!instance) return
    const url = `http://localhost:${instance.port}`
    void navigator.clipboard?.writeText(url)
    ui.toast({ kind: 'info', title: '已复制访问地址', message: url, duration: 2600 })
  }, [instance, ui])

  const revealFolder = useCallback(() => {
    if (!instance) return
    ui.toast({
      kind: 'info',
      title: '在原型中不可用',
      message: `接入 PHL Core 后将打开 ${instance.dshHome}`,
      duration: 3200,
    })
  }, [instance, ui])

  const openWebUI = useCallback(() => {
    if (!instance) return
    ui.toast({
      kind: 'info',
      title: '在原型中不可用',
      message: `接入 PHL Core 后将打开 localhost:${instance.port}`,
      duration: 3200,
    })
  }, [instance, ui])

  const menuItems = useMemo<MenuItem[]>(() => {
    if (!instance || !id) return []
    const status = store.stateOf(id).status
    const running = status === 'running'
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
      { id: 'clone', label: '克隆实例', icon: <Copy size={13} />, onSelect: clone },
      { id: 'snapshot', label: '创建快照', icon: <Camera size={13} />, onSelect: snapshot },
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

  return { clone, rename, remove, snapshot, copyPort, revealFolder, openWebUI, menuItems }
}
