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
  | { name: 'adopt' }
  | { name: 'installPack' }
  | { name: 'exportPack'; id: string }
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
    case 'adopt':
    case 'installPack':
    case 'exportPack':
      return 'instances'
    default:
      return r.name
  }
}

export const routeKey = (r: Route): string =>
  r.name === 'instance' || r.name === 'exportPack' ? `${r.name}:${r.id}` : r.name

/* ------------------------------------------------------------------ *
 * URL-driven routing (mode B)
 *
 * The address bar (hash form, which a Tauri custom-protocol origin can
 * always serve) is the source of truth for *where* we are; an in-memory
 * stack mirrors it so the chrome can render back/forward affordances.
 *
 * Two semantics, deliberately separate:
 *  - `navigate` = horizontal move between sibling screens. Replaces the
 *    current history entry, so hopping across tabs never pollutes the
 *    back stack.
 *  - `push`     = vertical drill-in (list → detail, list → wizard). Adds
 *    an entry, so `back` walks the drill path, not a chronological
 *    replay of every tab you glanced at.
 * ------------------------------------------------------------------ */

export const routePath = (r: Route): string => {
  switch (r.name) {
    case 'instance':
      return `/instance/${encodeURIComponent(r.id)}`
    case 'create':
      return r.cloneFrom ? `/create/${encodeURIComponent(r.cloneFrom)}` : '/create'
    case 'adopt':
      return '/adopt'
    case 'installPack':
      return '/install-pack'
    case 'exportPack':
      return `/export-pack/${encodeURIComponent(r.id)}`
    case 'apiConfig':
      return '/api-config'
    default:
      return `/${r.name}`
  }
}

export const parseRoute = (path: string): Route | null => {
  const parts = path.replace(/^\/+|\/+$/g, '').split('/')
  switch (parts[0]) {
    case '':
    case 'instances':
      return { name: 'instances' }
    case 'instance':
      return parts[1] ? { name: 'instance', id: decodeURIComponent(parts[1]) } : null
    case 'create':
      return parts[1] ? { name: 'create', cloneFrom: decodeURIComponent(parts[1]) } : { name: 'create' }
    case 'adopt':
      return { name: 'adopt' }
    case 'install-pack':
      return { name: 'installPack' }
    case 'export-pack':
      return parts[1] ? { name: 'exportPack', id: decodeURIComponent(parts[1]) } : null
    case 'versions':
      return { name: 'versions' }
    case 'plugins':
      return { name: 'plugins' }
    case 'api-config':
      return { name: 'apiConfig' }
    case 'runtimes':
      return { name: 'runtimes' }
    case 'settings':
      return { name: 'settings' }
    default:
      return null
  }
}

const initialRoute: Route = (() => {
  const hash = typeof window !== 'undefined' ? window.location.hash.replace(/^#/, '') : ''
  return parseRoute(hash) ?? { name: 'instances' }
})()

const hashFor = (r: Route) => `#${routePath(r)}`

// Mirror of the browser's own back/forward lists so the title bar can show
// the chevrons and (later) list destinations. We only ever append/truncate
// in ways that match what we ask `history` to do; a reload resets them,
// which is acceptable — the deep link itself survives a reload.
const nav = {
  back: [] as Route[],
  forward: [] as Route[],
  /**
   * Signed traversal delta for the history.go() we just requested. popstate
   * does not report how far it jumped, so multi-step "walk back to an
   * ancestor" moves would otherwise desync the mirror stacks.
   */
  pending: null as number | null,
}

/** -1 = back, 0 = lateral (no slide), 1 = forward. Drives page transitions. */
let navDirection = 0

const applyTraversal = (d: number) => {
  if (d < 0) {
    const steps = Math.min(-d, nav.back.length)
    const popped = nav.back.splice(nav.back.length - steps, steps)
    nav.forward = [...popped.reverse(), ...nav.forward].slice(0, 24)
  } else {
    const steps = Math.min(d, nav.forward.length)
    const shifted = nav.forward.splice(0, steps)
    nav.back = [...nav.back, ...shifted].slice(-24)
  }
}

const syncHistory = (set: (partial: Partial<UIState>) => void) =>
  set({
    navBack: [...nav.back],
    navForward: [...nav.forward],
    direction: navDirection,
    navSeq: ++navSeq,
  })

let navSeq = 0

/** A route that sits above `to` in the same tab's drill tree. */
const isAncestor = (ancestor: Route, to: Route): boolean => {
  if (routeTab(ancestor) !== routeTab(to)) return false
  if (ancestor.name !== 'instances') return false
  return (
    to.name === 'instance' ||
    to.name === 'create' ||
    to.name === 'adopt' ||
    to.name === 'installPack' ||
    to.name === 'exportPack'
  )
}

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
  /** Screens behind the current one (back stack), newest last. */
  navBack: Route[]
  /** Screens ahead of the current one (forward stack), next first. */
  navForward: Route[]
  /** Bumped on every history change so components can key off it. */
  navSeq: number
  /** 1 = navigating forward, -1 = going back, 0 = lateral (no slide). */
  direction: number
  /** Horizontal move between siblings: replaces history, never pushes. */
  navigate: (route: Route) => void
  /** Vertical drill-in (list → detail / wizard): adds a history entry. */
  push: (route: Route) => void
  /** Go one level up in the drill tree, reusing history when possible. */
  up: () => void
  back: () => void
  forward: () => void
  canGoBack: () => boolean
  canGoForward: () => boolean

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
      route: initialRoute,
      navBack: [],
      navForward: [],
      navSeq: 0,
      direction: 0,

      navigate: (route) => {
        const current = get().route
        if (routeKey(current) === routeKey(route)) return
        // Explicit "up": if a matching screen is already behind us, walk
        // back to it (keeping the forward stack alive) instead of burying
        // a duplicate under the current slot.
        for (let i = nav.back.length - 1; i >= 0; i--) {
          if (routeKey(nav.back[i]) === routeKey(route)) {
            navDirection = -1
            nav.pending = i - nav.back.length
            window.history.go(nav.pending)
            return
          }
        }
        if (isAncestor(current, route)) navDirection = -1
        else if (route.name === 'instance' || route.name === 'create') navDirection = 1
        else navDirection = 0
        nav.forward = []
        window.history.replaceState(null, '', hashFor(route))
        set({ route, navForward: [] })
        syncHistory(set)
      },

      push: (route) => {
        const current = get().route
        if (routeKey(current) === routeKey(route)) return
        navDirection = 1
        nav.back = [...nav.back, current].slice(-24)
        nav.forward = []
        window.history.pushState(null, '', hashFor(route))
        set({ route, navBack: [...nav.back], navForward: [] })
        syncHistory(set)
      },

      up: () => {
        const { route } = get()
        if (route.name === 'instance' || route.name === 'create') {
          get().navigate({ name: 'instances' })
        }
      },

      back: () => {
        if (!nav.back.length) return
        navDirection = -1
        nav.pending = -1
        window.history.back()
      },

      forward: () => {
        if (!nav.forward.length) return
        navDirection = 1
        nav.pending = 1
        window.history.forward()
      },

      canGoBack: () => nav.back.length > 0,
      canGoForward: () => nav.forward.length > 0,

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

/**
 * Wire real history traversal (Alt+←/→, mouse side buttons, in-app back)
 * back into the store. WebView2 handles those accelerators natively, so the
 * web layer only ever has to react to popstate. Returns a disposer.
 */
let popStateBound = false

export function bindHistorySync(): () => void {
  let boundHere = false
  if (!popStateBound) {
    popStateBound = true
    boundHere = true
    window.addEventListener('popstate', onPopState)
  }
  function onPopState() {
    const path =
      (window.history.state?.path as string | undefined) ??
      window.location.hash.replace(/^#/, '') ??
      '/'
    const next = parseRoute(path) ?? ({ name: 'instances' } as Route)
    const prev = useUIStore.getState().route
    const key = routeKey(next)
    if (key === routeKey(prev)) {
      nav.pending = null
      navDirection = 0
      return
    }

    let dir = 1
    const d = nav.pending
    nav.pending = null
    if (d !== null) {
      applyTraversal(d)
      dir = d < 0 ? -1 : 1
    } else if (navDirection === 0) {
      // A traversal we did not initiate (browser chrome gesture, reload).
      // Infer from the stacks by matching the destination's last occurrence.
      const bi = nav.back.map(routeKey).lastIndexOf(key)
      if (bi >= 0) {
        applyTraversal(bi - nav.back.length)
        dir = -1
      } else {
        const fi = nav.forward.map(routeKey).indexOf(key)
        if (fi >= 0) applyTraversal(fi + 1)
        else {
          nav.back = [...nav.back, prev].slice(-24)
          nav.forward = []
        }
        dir = 1
      }
    } else {
      applyTraversal(navDirection)
      dir = navDirection
    }
    navDirection = 0

    useUIStore.setState({
      route: next,
      navBack: [...nav.back],
      navForward: [...nav.forward],
      direction: dir,
      navSeq: ++navSeq,
    })
  }
  return () => {
    if (boundHere) {
      popStateBound = false
      window.removeEventListener('popstate', onPopState)
    }
  }
}
