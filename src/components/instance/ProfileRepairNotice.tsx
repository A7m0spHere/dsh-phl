/**
 * The repair affordance for a historical manifest saved with a non-web
 * profile (#4): such an instance can never launch (`dsh web` is the only
 * --port entry) and the backend now refuses to persist it, so the detail page
 * names the problem where the user will look for it.
 *
 * The repair is metadata-only — bind `web` back through the optimistic queue
 * and wait for the persisted result before announcing success (#5's rule).
 * No profile directory is moved or deleted; files under the old name stay
 * exactly where they were, and the note says so.
 */
import { useState } from 'react'
import { Badge, Button, Notice, Tooltip } from '@/components/ui'
import { useInstanceStore } from '@/stores/instanceStore'
import { useUIStore } from '@/stores/uiStore'
import type { Instance } from '@/types'

export function ProfileRepairNotice({ instance }: { instance: Instance }) {
  const update = useInstanceStore((s) => s.updateInstance)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)
  const [repairing, setRepairing] = useState(false)

  const repair = async () => {
    if (repairing) return
    const ok = await confirm({
      title: '把 Profile 改回 web',
      message: `该实例当前 Profile 是「${instance.profile}」，启动会被拒绝。改回 web 后，启动与插件读写都指向 profiles/web。`,
      detail: '仅修改实例登记，不移动或删除任何文件；原 profile 目录会原样保留。',
      confirmLabel: '改回 web',
    })
    if (!ok) return
    setRepairing(true)
    const saved = await update(instance.id, { profile: 'web' })
    setRepairing(false)
    if (!saved) return
    toast({
      kind: 'success',
      title: 'Profile 已改回 web',
      message: '原 profile 目录未做任何移动或删除，可自行处理。',
    })
  }

  return (
    <div className="mt-2">
      <Notice tone="danger" title="Profile 不是 web，实例当前无法启动">
        <p>
          PHL 只通过 `dsh web` 启动实例，而它固定读取 web profile。 该实例的 Profile
          被登记为「{instance.profile}」，启动会被拒绝，保存其他修改也会失败。
        </p>
        <div className="mt-2.5 flex gap-2">
          <Button size="sm" variant="primary" disabled={repairing} onClick={() => void repair()}>
            {repairing ? '修复中…' : '改回 web'}
          </Button>
        </div>
        <p className="mt-1.5 text-xs text-ink-faint">
          仅修改登记：原 {instance.profile} profile 目录的插件与配置不会被移动或删除。
        </p>
      </Notice>
    </div>
  )
}

/** The inline marker for the Profile row itself — same fact, smaller footprint. */
export function ProfileRowValue({ profile }: { profile: string }) {
  if (profile === 'web') return <>{profile}</>
  return (
    <span className="flex items-center gap-2">
      {profile}
      <Tooltip content="该 Profile 无法通过 `dsh web` 启动，保存修改也会被后端拒绝">
        <Badge tone="danger">无法启动</Badge>
      </Tooltip>
    </span>
  )
}
