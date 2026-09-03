import { mockRepository } from './mockRepository'
import { tauriRepository } from './tauriVersions'
import { isDesktop } from '@/lib/desktop'
import type { PhlRepository } from './repository'

/**
 * The single place that decides where PHL's data comes from.
 *
 * In the desktop shell the version / plugin / runtime / instance / process
 * modules are all real (Rust-backed); only templates are still mock. In the
 * browser it is mock all the way so `npm run dev` keeps working for pure UI
 * work.
 */
export const repository: PhlRepository = isDesktop ? tauriRepository : mockRepository

export * from './repository'
