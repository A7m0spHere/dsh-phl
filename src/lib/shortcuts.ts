import { isTyping, useHotkeys, type Hotkey } from './hooks'

export { isTyping }

/**
 * The app's interactive keyboard shortcuts, in one place.
 *
 * Each global entry doubles as the real binding: `App.tsx` registers these
 * through `useGlobalShortcut()`, so the settings reference and what actually
 * fires can never drift apart. Context-scoped groups (palette, dialogs, …)
 * are still handled by their own components and listed here for display and
 * for the "press to highlight" demo on the shortcuts page.
 */
export interface Shortcut {
  id: string
  /** Raw `KeyboardEvent.key`, lower-cased — same convention as `Hotkey`. */
  key: string
  /** Matches ctrlKey OR metaKey, mirroring `useHotkeys`. */
  ctrl?: boolean
  shift?: boolean
  alt?: boolean
  description: string
  /** Context hint, e.g. 「命令面板打开时」. */
  scope?: string
  /** Fire even while a text field has focus — same meaning as `Hotkey.global`. */
  global?: boolean
}

export interface ShortcutGroup {
  id: string
  title: string
  description: string
  items: Shortcut[]
}

export const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    id: 'global',
    title: '全局',
    description: '在任何页面都生效。',
    items: [
      {
        id: 'palette',
        key: 'k',
        ctrl: true,
        global: true,
        description: '打开 / 关闭快速跳转，搜索实例、页面和操作',
      },
      { id: 'settings', key: ',', ctrl: true, description: '打开设置' },
      {
        id: 'go-back',
        key: 'escape',
        description: '返回上一页；没有返回路径时回到上层栏目',
        scope: '快速跳转或对话框打开时让位给它们',
      },
    ],
  },
  {
    id: 'palette',
    title: '快速跳转',
    description: '打开快速跳转面板后，在面板内生效。',
    items: [
      { id: 'palette-close', key: 'escape', description: '关闭面板' },
      { id: 'palette-down', key: 'arrowdown', description: '向下移动选择' },
      { id: 'palette-up', key: 'arrowup', description: '向上移动选择' },
      { id: 'palette-run', key: 'enter', description: '执行选中的命令' },
    ],
  },
  {
    id: 'popups',
    title: '对话框与下拉',
    description: '确认 / 输入对话框、下拉选择与菜单打开时生效。',
    items: [
      { id: 'popup-close', key: 'escape', description: '关闭对话框、下拉或菜单' },
      { id: 'dropdown-open', key: 'arrowdown', description: '在下拉未展开时打开选项列表', scope: '下拉' },
      { id: 'dropdown-open-enter', key: 'enter', description: '在下拉未展开时打开选项列表', scope: '下拉' },
      { id: 'dropdown-move-down', key: 'arrowdown', description: '向下移动高亮项', scope: '下拉打开时' },
      { id: 'dropdown-move-up', key: 'arrowup', description: '向上移动高亮项', scope: '下拉打开时' },
      { id: 'dropdown-commit', key: 'enter', description: '选中高亮项', scope: '下拉打开时' },
      { id: 'popup-submit', key: 'enter', description: '在输入框内直接提交', scope: '对话框' },
    ],
  },
  {
    id: 'discovery',
    title: '插件发现',
    description: '插件页的「发现插件」弹窗打开时生效。',
    items: [
      { id: 'discovery-close', key: 'escape', description: '关闭弹窗' },
      { id: 'discovery-down', key: 'arrowdown', description: '浏览下一个推荐' },
      { id: 'discovery-up', key: 'arrowup', description: '浏览上一个推荐' },
    ],
  },
  {
    id: 'a11y',
    title: '键盘无障碍',
    description: '焦点落在可折叠卡片的标题栏上时生效。',
    items: [
      { id: 'card-space-activate', key: ' ', description: '展开 / 收起该卡片分区' },
      { id: 'card-enter-activate', key: 'enter', description: '展开 / 收起该卡片分区' },
    ],
  },
]

export function isMacPlatform(): boolean {
  return typeof navigator !== 'undefined' && /mac/i.test(navigator.platform)
}

/** Look up one shortcut by id (used where a binding is registered for real). */
export function shortcutById(id: string): Shortcut {
  for (const g of SHORTCUT_GROUPS) {
    const found = g.items.find((s) => s.id === id)
    if (found) return found
  }
  throw new Error(`unknown shortcut: ${id}`)
}

/** Human labels for one shortcut's chips, e.g. ['⌘', 'K'] on macOS. */
export function shortcutChips(s: Shortcut, mac: boolean): string[] {
  const chips: string[] = []
  if (s.ctrl) chips.push(mac ? '⌘' : 'Ctrl')
  if (s.alt) chips.push(mac ? '⌥' : 'Alt')
  if (s.shift) chips.push(mac ? '⇧' : 'Shift')
  chips.push(keyLabel(s.key))
  return chips
}

function keyLabel(key: string): string {
  switch (key) {
    case 'escape':
      return 'Esc'
    case 'arrowdown':
      return '↓'
    case 'arrowup':
      return '↑'
    case 'arrowleft':
      return '←'
    case 'arrowright':
      return '→'
    case 'enter':
      return 'Enter'
    case ' ':
      return 'Space'
    default:
      return key.toUpperCase()
  }
}

/** True when a live key event is exactly this shortcut's combination. */
export function matchesShortcut(s: Shortcut, e: KeyboardEvent): boolean {
  return (
    e.key.toLowerCase() === s.key &&
    !!s.ctrl === (e.ctrlKey || e.metaKey) &&
    !!s.shift === e.shiftKey &&
    !!s.alt === e.altKey
  )
}

/**
 * Register one global shortcut from the catalogue. `run` is kept in a ref, so
 * callers may pass a closure over fresh state without re-binding the listener.
 * Pass `enabled: false` when a surface above the app owns the key already
 * (e.g. Esc while a dialog or the palette is open).
 */
export function useGlobalShortcut(shortcut: Shortcut, run: () => void, enabled = true) {
  const hotkeys: Hotkey[] = enabled
    ? [{ key: shortcut.key, ctrl: shortcut.ctrl, shift: shortcut.shift, alt: shortcut.alt, global: shortcut.global, run }]
    : []
  useHotkeys(hotkeys)
}
