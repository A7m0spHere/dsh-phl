import { mockRepository } from './mockRepository'
import type { PhlRepository } from './repository'

/**
 * The single place that decides where PHL's data comes from.
 * Swapping in a Tauri-backed implementation is a one-line change here.
 */
export const repository: PhlRepository = mockRepository

export * from './repository'
