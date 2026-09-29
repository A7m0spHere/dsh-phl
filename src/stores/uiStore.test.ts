import { beforeEach, expect, it } from 'vitest'
import { useUIStore } from './uiStore'

beforeEach(() => {
  useUIStore.setState({ dialog: null })
})

it('a displaced confirm settles false instead of hanging forever', async () => {
  // The 2026-09-29 review: a second confirm() (e.g. the window close
  // guard) used to overwrite the first dialog and drop its resolver —
  // the awaiting code never resumed. The displaced dialog must take its
  // no-branch.
  const first = useUIStore.getState().confirm({ title: '删除实例', message: '...' })
  const second = useUIStore.getState().confirm({ title: '退出 PHL', message: '...' })

  expect(await first).toBe(false)
  const dialog = useUIStore.getState().dialog
  expect(dialog?.spec.title).toBe('退出 PHL')

  useUIStore.getState().closeDialog(true)
  expect(await second).toBe(true)
})

it('a displaced prompt settles null the same way', async () => {
  const first = useUIStore.getState().prompt({ title: '命名', label: '名称' })
  const second = useUIStore.getState().prompt({ title: '重命名', label: '名称' })
  expect(await first).toBeNull()
  useUIStore.getState().closeDialog('kept')
  expect(await second).toBe('kept')
})

it('confirming the current dialog still resolves true only for its own promise', async () => {
  const p = useUIStore.getState().confirm({ title: 't', message: 'm' })
  useUIStore.getState().closeDialog(true)
  expect(await p).toBe(true)
  expect(useUIStore.getState().dialog).toBeNull()
})
