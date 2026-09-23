import {
  importInstanceBundle,
  readInstanceBundle,
  type RemoteBundlePreview,
} from '@/lib/desktop'
import { instanceFromRecord, newInstanceId } from './tauriInstances'
import type { Instance } from '@/types'

/**
 * Bundle-import service entry (roadmap O-14: pages must not assemble
 * manifests or convert backend records inline). The file *dialog* stays in
 * `@/lib/desktop` — that is a system interaction — but reading a bundle and
 * turning one into a live `Instance` is business data access, and the
 * desktop/browser split now lives here instead of leaking into
 * `InstancesPage`.
 */

export type BundlePreview = RemoteBundlePreview

/** Read the bundle header for the confirm dialog. Throws in the browser. */
export function readBundle(path: string): Promise<BundlePreview> {
  return readInstanceBundle(path)
}

/**
 * Import a bundle under the identity fields the page collected (fresh id, a
 * free port) and return the instance to admit into the store. The Rust side
 * overwrites the environment fields with the bundle's own values; the
 * credential note is the caller's to surface — the values never travel with
 * the bundle.
 */
export async function importBundleAsInstance(
  path: string,
  preview: BundlePreview,
  port: number,
): Promise<{ instance: Instance; credentials: string[] }> {
  const outcome = await importInstanceBundle(path, {
    id: newInstanceId(preview.name),
    name: preview.name,
    note: null,
    kind: 'sandbox',
    hue: 0,
    versionId: preview.versionId,
    runtimeId: preview.runtimeId,
    port,
    autoPort: true,
    profile: 'web',
    createdAt: new Date().toISOString(),
    lastRunAt: null,
    totalRuntime: 0,
    favorite: false,
    env: {},
    args: [],
    // Same promise as the create wizard: an imported instance boots
    // configured. The Rust import applies it through the shared path.
    api: { inheritance: 'default', providerIds: [] },
  })
  return { instance: instanceFromRecord(outcome.record), credentials: outcome.credentials }
}
