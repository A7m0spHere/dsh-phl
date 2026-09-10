import type { StoreApi } from 'zustand'
import type { PluginCatalogOrigin } from '@/services'
import type { DshVersion, InstanceTemplate, Plugin, Runtime } from '@/types'

/** Live transfer state for one (instance, plugin) install. */
export interface PluginTransferState {
  stage:
    | 'queued'
    | 'preparing'
    | 'downloading'
    | 'verifying'
    | 'installing'
    /** pnpm is resolving the plugin's dependency closure (no ratio to show). */
    | 'deps'
    /** The transaction commit: marker + Cordis registration + enable flag. */
    | 'committing'
  progress: number
  bytesDone: number
  bytesPerSec: number
}

/** Per-plugin update-check state (§R5). */
export type LatestVersionState =
  | { status: 'checking' }
  | { status: 'resolved'; latest: string }
  | { status: 'unresolvable' }
  | { status: 'error'; message: string }

export interface CatalogState {
  versions: DshVersion[]
  runtimes: Runtime[]
  plugins: Plugin[]
  templates: InstanceTemplate[]
  loaded: boolean
  loading: boolean
  /** True when neither a live source nor the on-disk cache produced a catalog. */
  pluginsOffline: boolean
  pluginsError?: string
  pluginsOrigin: PluginCatalogOrigin | null
  versionsLoaded: boolean
  versionsSyncing: boolean
  versionsSyncedAt: number | null
  versionsSyncAttemptedAt: number

  load: () => Promise<void>
  refreshVersions: (opts?: { silent?: boolean }) => Promise<void>
  installVersion: (id: string) => Promise<void>
  cancelVersion: (id: string) => void
  removeVersion: (id: string) => Promise<void>

  installRuntime: (id: string) => Promise<void>
  cancelRuntime: (id: string) => void
  removeRuntime: (id: string) => Promise<void>

  pluginTransfers: Record<string, PluginTransferState>
  latestVersions: Record<string, LatestVersionState>
  refreshLatestVersions: (instanceId: string) => Promise<void>
  recheckLatestVersions: (instanceId: string) => Promise<void>
  installPlugin: (instanceId: string, pluginId: string) => Promise<void>
  cancelPlugin: (instanceId: string, pluginId: string) => void
  setPluginEnabled: (instanceId: string, pluginId: string, enabled: boolean) => Promise<void>
  uninstallPlugin: (instanceId: string, pluginId: string) => Promise<void>

  versionById: (id: string) => DshVersion | undefined
  runtimeById: (id: string) => Runtime | undefined
  pluginById: (id: string) => Plugin | undefined
  activeTransfers: () => number
}

export type CatalogSet = StoreApi<CatalogState>['setState']
export type CatalogGet = StoreApi<CatalogState>['getState']
export type TransferControllers = Map<string, AbortController>

export type VersionActions = Pick<
  CatalogState,
  'refreshVersions' | 'installVersion' | 'cancelVersion' | 'removeVersion'
>
export type RuntimeActions = Pick<CatalogState, 'installRuntime' | 'cancelRuntime' | 'removeRuntime'>
export type PluginActions = Pick<
  CatalogState,
  | 'refreshLatestVersions'
  | 'recheckLatestVersions'
  | 'installPlugin'
  | 'cancelPlugin'
  | 'setPluginEnabled'
  | 'uninstallPlugin'
>
