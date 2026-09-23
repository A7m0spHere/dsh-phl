import { parseThrownError } from '@/lib/errorCodes'
import { fetchProviderModels } from '@/lib/desktop'
import type { RemoteModel } from '@/types'

/**
 * API-configuration domain entry for provider model discovery.
 *
 * The editor used to import the bridge function and parse the backend's
 * `ENV_MISSING:` marker inline, which put the browser/desktop split and the
 * wire error format into a component. The probe request and its failure
 * shape are business behaviour of the API-config domain, so they live here;
 * the component keeps only its own state (dedupe sequence, per-credential
 * cache, selection).
 */

/** The marker the backend prefixes when the configured env key is absent. */
export const ENV_MISSING_PREFIX = 'ENV_MISSING:'

export interface ProbeContext {
  baseURL: string
  api?: string
  apiKeyEnv: string
  /** Form-entered / stored key tried first by the backend. */
  apiKey?: string
  /** Lets the backend fall back to this provider's OS-stored credential. */
  providerId?: string
}

export type ProbeResult =
  | { kind: 'ok'; models: RemoteModel[] }
  /** The configured env variable has no value; the editor asks for a key. */
  | { kind: 'missing-key'; message: string }
  /** Any other backend failure, already unwrapped from the `[code]` wire form. */
  | { kind: 'error'; message: string }

/** Probe `/models` for one provider; never throws, never touches UI state. */
export async function probeProviderModels(ctx: ProbeContext): Promise<ProbeResult> {
  try {
    const models = await fetchProviderModels(ctx)
    return { kind: 'ok', models }
  } catch (err) {
    const msg = parseThrownError(err).message
    if (msg.startsWith(ENV_MISSING_PREFIX)) {
      return { kind: 'missing-key', message: msg.slice(ENV_MISSING_PREFIX.length) }
    }
    return { kind: 'error', message: msg }
  }
}
