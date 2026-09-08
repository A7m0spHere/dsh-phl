import { invoke } from '@tauri-apps/api/core'

/**
 * The seam between PHL's UI and the desktop shell.
 *
 * The app runs in a frameless Tauri window with its own title bar, so window
 * controls are the app's responsibility. Everything here degrades to a no-op
 * in a plain browser, which keeps `npm run dev` usable for pure UI work
 * without booting Rust.
 */

import { isDesktop, currentWindow, noop, type Unlisten } from './desktopCore'

// Re-exported so existing `from '@/lib/desktop'` imports keep working while
// the domain split (roadmap O-13) lands one interaction at a time.
export { isDesktop }
export type { Unlisten } from './desktopCore'

export const desktop = {
  isDesktop,

  /** Reveal the window once the first paint is done (it starts hidden). */
  async ready(): Promise<void> {
    if (!isDesktop) return
    try {
      await invoke('app_ready')
    } catch {
      // If the command is unavailable the Rust-side timeout still shows it.
    }
  },

  async minimize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().minimize()
  },

  async toggleMaximize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().toggleMaximize()
  },

  /**
   * Asks the window to close. Rust intercepts the request and hands it back
   * to the frontend through `onCloseRequested`, so the confirmation dialog is
   * the same whether the user clicks our button or the OS asks.
   */
  async requestClose(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().close()
  },

  /** Actually quit, after the user has confirmed. */
  async exit(): Promise<void> {
    if (!isDesktop) return noop()
    await invoke('exit_app')
  },

  async isMaximized(): Promise<boolean> {
    if (!isDesktop) return false
    try {
      return await currentWindow().isMaximized()
    } catch {
      return false
    }
  },

  /** Fires whenever the window is resized, which covers maximize/restore. */
  async onResized(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().onResized(handler)
  },

  async onCloseRequested(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().listen('phl://close-requested', () => handler())
  },
}

// §M2: the native folder picker moved to `desktopShell.ts` (with reveal/open).
export { chooseDirectory } from './desktopShell'

/* -------------------------------- storage ------------------------------- */

// §M2: storage + migration journal moved to `desktopStorage.ts`; re-exported
// verbatim so consumers (including the browser mocks) are unchanged.
export {
  freeSpace,
  rootDataSummary,
  moveRootData,
  migrationStatus,
  migrationUndo,
} from './desktopStorage'
export type {
  DirSummary,
  RootDataSummary,
  MoveProgress,
  MoveSummary,
  MigrationJournalEntry,
  MigrationJournal,
} from './desktopStorage'

/* ------------------------------- versions ------------------------------- */

// §M2: DSH-version commands, the data-root handshake, and the shared
// `cancelTransfer` moved to `desktopVersions.ts`; re-exported verbatim.
export {
  defaultRoot,
  initPhlRoot,
  setPhlRoot,
  listDshVersions,
  listInstalledVersions,
  removeVersionDir,
  downloadDshVersion,
  cancelTransfer,
} from './desktopVersions'
export type { RemoteVersionMeta, InstalledVersionInfo, DownloadArgs } from './desktopVersions'

/* ------------------------------- plugins ------------------------------- */

// §M2: plugin IPC moved to `desktopPlugins.ts`; re-exported verbatim so every
// `from '@/lib/desktop'` consumer (command names, arg shapes, the progress
// `Channel`, browser degradation) is unchanged.
export {
  listDshPlugins,
  installPlugin,
  pluginLatestVersion,
  setPluginEnabled,
  uninstallPlugin,
} from './desktopPlugins'
export type {
  RemotePluginSource,
  RemotePluginMeta,
  PluginProgressStage,
  RemotePluginCatalog,
  InstallPluginArgs,
} from './desktopPlugins'

/* ------------------------------ runtimes ------------------------------ */

// §M2: the Node-runtime domain lives in `desktopRuntimes.ts` (one cohesive
// module, like the Tasks / Pack / Sessions / Adoption split before it).
// Re-exported here verbatim so every `from '@/lib/desktop'` import — the command
// names, argument shapes, the progress `Channel`, and the browser degradation —
// is unchanged.
export {
  NODE_DIST_OFFICIAL,
  NODE_DIST_MIRROR,
  listNodeRuntimeCatalog,
  listInstalledRuntimes,
  systemNodeVersion,
  downloadNodeRuntime,
  removeRuntimeDir,
  runtimesDiskUsage,
} from './desktopRuntimes'
export type { RemoteRuntimeMeta, InstalledRuntimeInfo, DownloadRuntimeArgs } from './desktopRuntimes'

/* ------------------------------- launch ------------------------------- */
// §M2: launch IPC moved to `desktopLaunch.ts`; re-exported verbatim.
export {
  launchInstance,
  adoptProcesses,
  stopInstance,
  cancelLaunch,
  onInstanceExited,
  openDshWebUi,
} from './desktopLaunch'
export type {
  LaunchEventMsg,
  LaunchInstanceArgs,
  LaunchResult,
  AdoptedProcess,
  DroppedProcess,
  AdoptReport,
} from './desktopLaunch'

/* ------------------------------ task centre ------------------------------ */

// Moved to `desktopTasks.ts` (roadmap O-13, first domain unit out of this
// bridge): task types + listTasks re-exported unchanged.
export { listTasks } from './desktopTasks'
export type { TaskInfo, TaskList } from './desktopTasks'

/* --------------------------- discovery + adoption ------------------------- */

// Second O-13 domain unit out of this bridge. `desktopAdoption` imports only
// types from here (erased at build), so the re-export is not a runtime cycle.
export {
  discoverDsh,
  inspectDshHome,
  inspectDshExecutable,
  previewAdoption,
  adoptInstance,
  instanceSessionCount,
  chooseDshHome,
  chooseDshExecutable,
} from './desktopAdoption'
export type {
  RemoteDshCandidate,
  RemoteManagedInstance,
  DiscoverySource,
  DiscoveryConfidence,
  AdoptionMode,
  AdoptionSessionStrategy,
  AdoptionRequest,
  RemoteAdoptionPreview,
  RemoteAdoptionOutcome,
} from './desktopAdoption'

/* ------------------------- session migration --------------------------- */

// Third/fourth O-13 domain units: the P1 Session engine and the `.phlpack`
// export/install pipeline. Type-only imports on their side, no runtime cycle.
export {
  listSessions,
  listHomeSessions,
  inspectSession,
  copySession,
  copySessions,
} from './desktopSessions'
export type {
  RemoteSessionInfo,
  RemoteSessionDetail,
  RemoteSessionCopyOutcome,
  RemoteSessionProgress,
} from './desktopSessions'
export {
  previewInstancePackExport,
  exportInstancePack,
  previewPack,
  installPack,
  choosePackSavePath,
  choosePackOpenPath,
} from './desktopPack'
export type {
  RemotePackExportPlan,
  RemoteExportPluginPlan,
  PackExportOptions,
  RemotePackExportReport,
  RemotePackPreview,
  RemotePackDependency,
  PackDependencyStatus,
  PackInstallRequest,
  RemotePackInstallOutcome,
} from './desktopPack'

/* ---------------------------- diagnostics ----------------------------- */

// §M2: diagnostics moved to `desktopDiagnostics.ts`, re-exported verbatim so all
// existing imports from '@/lib/desktop' are unchanged.
export { runDiagnostics, clearDownloadCache } from './desktopDiagnostics'
export type { DiagnosticItem, DiagnosticReport } from './desktopDiagnostics'

/* ------------------------------- bundles ------------------------------ */

// §M2: bundle dialogs + export/read/import commands moved to `desktopBundles.ts`
// (which imports the instance record/manifest types from `desktopInstances`).
export {
  chooseSaveFile,
  chooseBundleFile,
  previewInstanceExport,
  exportInstanceBundle,
  readInstanceBundle,
  importInstanceBundle,
} from './desktopBundles'
export type {
  RemoteBundlePreview,
  RemoteBundleExportReport,
  RemoteBundleImportOutcome,
} from './desktopBundles'

/* ------------------------------ verify + repair ----------------------- */

// §M2: environment-health commands moved to `desktopVerify.ts`; re-exported.
export { verifyInstance, repairInstance } from './desktopVerify'
export type {
  RemoteVerifyCheck,
  RemoteVerifyResult,
  RepairOutcome,
} from './desktopVerify'

/* ------------------------ instances + snapshots ----------------------- */

// §M2: instance-lifecycle + snapshot commands moved to `desktopInstances.ts`.
// `RemoteBundleImportOutcome` now lives with the bundle domain that consumes it.
export {
  listInstanceRecords,
  createInstanceDir,
  saveInstanceManifest,
  deleteInstanceDir,
  cloneInstanceDir,
  instanceDiskUsage,
  scanOrphanInstances,
  deleteOrphanInstance,
  createInstanceSnapshot,
  restoreInstanceSnapshot,
  deleteInstanceSnapshot,
} from './desktopInstances'
export type {
  RemoteAdoptedFrom,
  RemoteInstanceRecord,
  RemoteSnapshotInfo,
  RemoteInstanceManifest,
  CloneInstanceArgs,
} from './desktopInstances'

/* ---------------------------- api config ------------------------------ */

// §M2: API-config IPC moved to `desktopApiConfig.ts`; re-exported verbatim.
export {
  loadApiConfig,
  saveApiConfig,
  syncInstanceApi,
  importInstanceApi,
  instanceLiveSnapshot,
  fetchProviderModels,
  enrichModelMetadata,
} from './desktopApiConfig'

/* --------------------------- shell: reveal + external ------------------- */

// §M2: generic shell helpers (folder picker lives with the dialogs above, the
// reveal + open helpers here) moved to `desktopShell.ts`.
export { revealPath, openExternal } from './desktopShell'
