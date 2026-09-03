import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'

/* ------------------------------------------------------------------ *
 * routing
 * ------------------------------------------------------------------ */

export type Tab = 'instances' | 'versions' | 'plugins' | 'apiConfig' | 'runtimes' | 'settings'

export type Route =
  | { name: 'instances' }
  | { name: 'instance'; id: string }
  | { name: 'create'; cloneFrom?: string }
  | { name: 'versions' }
  | { name: 'plugins' }
  | { name: 'apiConfig' }
  | { name: 'runtimes' }
  | { name: 'settings' }

export const routeTab = (r: Route): Tab => {
  switch (r.name) {
    case 'instances':
    case 'instance':
    case 'create':
      return 'instances'
    default:
      return r.name
  }
}

export const routeKey = (r: Route): string =>
  r.name === 'instance' ? `instance:${r.id}` : r.name

/* ------------------------------------------------------------------ *
 * transient UI: toasts + dialogs
 * ------------------------------------------------------------------ */

export type ToastKind = 'info' | 'success' | 'warn' | 'error' | 'progress'

export interface Toast {
  id: string
  kind: ToastKind
  title: string
  message?: string
  /** ms; 0 keeps it until dismissed. */
  duration: number
  action?: { label: string; run: () => void }
  createdAt: number
}

export interface ConfirmSpec {
  title: string
  message: string
  detail?: string
  confirmLabel?: string
  cancelLabel?: string
  tone?: 'default' | 'danger'
  /** When set, the user must type this exact text to enable the action. */
  typeToConfirm?: string
}

export interface PromptSpec {
  title: string
  label: string
  defaultValue?: string
  placeholder?: string
  confirmLabel?: string
  helper?: string
  validate?: (value: string) => string | null
}

type Dialog =
  | { kind: 'confirm'; spec: ConfirmSpec; resolve: (ok: boolean) => void }
  | { kind: 'prompt'; spec: PromptSpec; resolve: (value: string | null) => void }

/* ------------------------------------------------------------------ *
 * appearance
 * ------------------------------------------------------------------ */

export type Theme = 'light' | 'dark' | 'system'
export type Accent = 'azure' | 'indigo' | 'teal' | 'moss' | 'amber' | 'rose' | 'graphite'
export type Density = 'compact' | 'default' | 'comfortable'
export type MotionLevel = 'full' | 'reduced' | 'off'
export type InstanceLayout = 'grid' | 'list'
/** Whole-UI zoom, as a percentage. Chromium honours `zoom` on the root. */
export type UIScale = 85 | 90 | 100 | 110 | 125

export const ACCENTS: { id: Accent; label: string }[] = [
  { id: 'azure', label: '钢蓝' },
  { id: 'indigo', label: '靛蓝' },
  { id: 'teal', label: '青' },
  { id: 'moss', label: '苔绿' },
  { id: 'amber', label: '琥珀' },
  { id: 'rose', label: '绯红' },
  { id: 'graphite', label: '石墨' },
]

interface UIState {
  /* routing */
  route: Route
  history: Route[]
  /** 1 = navigating forward, -1 = going back. Drives page transition direction. */
  direction: number
  navigate: (route: Route) => void
  back: () => void
  canGoBack: () => boolean

  /* appearance (persisted) */
  theme: Theme
  accent: Accent
  density: Density
  scale: UIScale
  motion: MotionLevel
  layout: InstanceLayout
  showLaunchDock: boolean
  confirmDelete: boolean
  isDark: boolean
  setTheme: (t: Theme) => void
  setAccent: (a: Accent) => void
  setDensity: (d: Density) => void
  setScale: (s: UIScale) => void
  setMotion: (m: MotionLevel) => void
  setLayout: (l: InstanceLayout) => void
  setPref: <K extends 'showLaunchDock' | 'confirmDelete'>(key: K, value: UIState[K]) => void
  syncSystemTheme: () => void

  /* toasts */
  toasts: Toast[]
  toast: (t: Omit<Toast, 'id' | 'createdAt' | 'duration'> & { duration?: number }) => string
  dismissToast: (id: string) => void

  /* dialogs */
  dialog: Dialog | null
  confirm: (spec: ConfirmSpec) => Promise<boolean>
  prompt: (spec: PromptSpec) => Promise<string | null>
  closeDialog: (result?: boolean | string | null) => void

  /* quick switcher */
  paletteOpen: boolean
  setPaletteOpen: (open: boolean) => void

  /* first-run guide */
  guideOpen: boolean
  setGuideOpen: (open: boolean) => void
}

const prefersDark = () =>
  typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches

const resolveDark = (theme: Theme) => (theme === 'system' ? prefersDark() : theme === 'dark')

let toastSeq = 0

export const useUIStore = create<UIState>()(
  persist(
    (set, get) => ({
      /* ---------------- routing ---------------- */
      route: { name: 'instances' },
      history: [],
      direction: 1,

      navigate: (route) => {
        const current = get().route
        if (routeKey(current) === routeKey(route)) return
        set({ route, history: [...get().history, current].slice(-24), direction: 1 })
      },

      back: () => {
        const history = get().history
        if (!history.length) return
        set({
          route: history[history.length - 1],
          history: history.slice(0, -1),
          direction: -1,
        })
      },

      canGoBack: () => get().history.length > 0,

      /* ---------------- appearance ---------------- */
      theme: 'system',
      accent: 'azure',
      density: 'default',
      scale: 100,
      motion: 'full',
      layout: 'grid',
      showLaunchDock: true,
      confirmDelete: true,
      isDark: resolveDark('system'),

      setTheme: (theme) => {
        const isDark = resolveDark(theme)
        document.documentElement.classList.toggle('dark', isDark)
        set({ theme, isDark })
      },
      setAccent: (accent) => {
        document.documentElement.setAttribute('data-accent', accent)
        set({ accent })
      },
      setDensity: (density) => {
        document.documentElement.setAttribute('data-density', density)
        set({ density })
      },
      setScale: (scale) => {
        // `zoom` scales layout as well as type, which is what "the whole UI is
        // too big" actually asks for; a font-size change alone would not move
        // the fixed pixel metrics.
        document.documentElement.style.zoom = scale === 100 ? '' : String(scale / 100)
        set({ scale })
      },
      setMotion: (motion) => set({ motion }),
      setLayout: (layout) => set({ layout }),
      setPref: (key, value) => set({ [key]: value } as Partial<UIState>),

      syncSystemTheme: () => {
        const { theme } = get()
        if (theme !== 'system') return
        const isDark = prefersDark()
        document.documentElement.classList.toggle('dark', isDark)
        set({ isDark })
      },

      /* ---------------- toasts ---------------- */
      toasts: [],
      toast: ({ duration, ...rest }) => {
        const id = `t${++toastSeq}`
        const entry: Toast = {
          id,
          createdAt: Date.now(),
          duration: duration ?? (rest.kind === 'error' ? 7000 : 4200),
          ...rest,
        }
        // Cap the stack; a launcher should never bury the app under toasts.
        set({ toasts: [...get().toasts.slice(-3), entry] })
        return id
      },
      dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),

      /* ---------------- dialogs ---------------- */
      dialog: null,
      confirm: (spec) =>
        new Promise<boolean>((resolve) => set({ dialog: { kind: 'confirm', spec, resolve } })),
      prompt: (spec) =>
        new Promise<string | null>((resolve) => set({ dialog: { kind: 'prompt', spec, resolve } })),
      closeDialog: (result) => {
        const dialog = get().dialog
        if (!dialog) return
        set({ dialog: null })
        if (dialog.kind === 'confirm') dialog.resolve(result === true)
        else dialog.resolve(typeof result === 'string' ? result : null)
      },

      /* ---------------- palette ---------------- */
      paletteOpen: false,
      setPaletteOpen: (paletteOpen) => set({ paletteOpen }),

      /* ---------------- guide ---------------- */
      guideOpen: false,
      setGuideOpen: (guideOpen) => set({ guideOpen }),
    }),
    {
      name: 'phl.ui',
      storage: createJSONStorage(() => localStorage),
      partialize: (s) => ({
        theme: s.theme,
        accent: s.accent,
        density: s.density,
        scale: s.scale,
        motion: s.motion,
        layout: s.layout,
        showLaunchDock: s.showLaunchDock,
        confirmDelete: s.confirmDelete,
      }),
      onRehydrateStorage: () => (state) => {
        if (!state) return
        state.setTheme(state.theme)
        state.setAccent(state.accent)
        state.setDensity(state.density)
        state.setScale(state.scale)
      },
    },
  ),
)

/** Convenience selectors. */
export const useRoute = () => useUIStore((s) => s.route)
export const useNavigate = () => useUIStore((s) => s.navigate)
export const useToast = () => useUIStore((s) => s.toast)
export const useIsDark = () => useUIStore((s) => s.isDark)
