/** Settings navigation model and storage presets. */
import { type ReactNode } from 'react'
import { Download, HardDrive, Info, Keyboard, Palette, Settings2, SlidersHorizontal, Stethoscope } from 'lucide-react'
import { type SettingsSection } from '@/stores'

export const SECTIONS: { id: SettingsSection; label: string; icon: ReactNode }[] = [
  { id: 'general', label: '通用', icon: <Settings2 size={13} /> },
  { id: 'downloads', label: '下载', icon: <Download size={13} /> },
  { id: 'appearance', label: '外观', icon: <Palette size={13} /> },
  { id: 'shortcuts', label: '快捷键', icon: <Keyboard size={13} /> },
  { id: 'storage', label: '存储', icon: <HardDrive size={13} /> },
  { id: 'diagnostics', label: '诊断', icon: <Stethoscope size={13} /> },
  { id: 'advanced', label: '高级', icon: <SlidersHorizontal size={13} /> },
  { id: 'about', label: '关于', icon: <Info size={13} /> },
]

/**
 * Common places to park a multi-gigabyte data directory.
 *
 * No "user directory" preset: the only correct value for it is whatever
 * `defaultRoot()` resolves on this machine, and the hardcoded placeholder
 * that used to sit here pointed the app at a nonexistent user's AppData once
 * the root started driving real file operations.
 */
export const ROOT_PRESETS = [
  { label: 'D:\\PHL', path: 'D:\\PHL' },
  { label: 'E:\\PHL', path: 'E:\\PHL' },
]

export const MOVE_KIND_LABELS: Record<string, string> = {
  instances: '实例目录',
  versions: 'DSH 版本',
  runtimes: 'Node Runtime',
  cache: '下载缓存',
}
