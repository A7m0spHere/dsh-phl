/**
 * The global API provider library and its per-instance bindings.
 *
 * Mirrors the Rust wire types in `src-tauri/src/api_config.rs`. The library
 * lives once at `<root>/config/api.json`; each instance stores only a
 * *binding* (an id reference plus its inheritance mode) — materialization
 * writes the merged result into the instance's `dsh-home/settings.yaml`.
 *
 * Secrets are never part of these shapes: a provider references an
 * environment-variable *name* (`apiKeyEnv`, DSH's own field), so the key
 * itself lives in the user's environment or DSH's `.credentials.yaml`.
 */

export interface ApiModelRef {
  id: string
  name?: string
  contextWindow?: number
  maxTokens?: number
  input?: Array<'text' | 'image'>
  reasoningEfforts?: false | Record<string, string | null>
  /** PHL-only, per-field provenance; never sent to settings.yaml. */
  metadataSources?: Partial<Record<ModelMetadataField, ModelMetadataSource>>
}

export type ModelMetadataField = 'name' | 'contextWindow' | 'maxTokens' | 'input' | 'reasoningEfforts'
export type ModelMetadataSource = 'models.dev' | 'openrouter' | 'fallback' | 'manual'
export interface ModelMetadataRequest {
  models: ApiModelRef[]
  /** DSH provider route name, not PHL's opaque provider id. */
  provider?: string
  /**
   * The provider's configured endpoint. Its host identifies the serving
   * provider far more reliably than the user's display name (disambiguation
   * hint for same-named models), and OpenRouter routes unlock the second
   * metadata tier.
   */
  baseUrl?: string
  /** "更新目录并补全": bypass the freshness window for this call. */
  forceRefresh?: boolean
  /** Settings gate for the OpenRouter second tier; default true. */
  useOpenRouter?: boolean
}
export interface ModelMetadataResolution {
  model: ApiModelRef
  matched: boolean
  changed: boolean
  ambiguous: boolean
}
export interface ModelMetadataBatch {
  results: ModelMetadataResolution[]
  catalogStatus: 'fresh' | 'stale' | 'unavailable' | 'mock'
  /** Whether the OpenRouter tier was consulted for this batch. */
  openrouterConsulted?: boolean
}

/**
 * One entry from a provider's live `GET /models` listing — the shape PHL
 * trusts (only `id` is guaranteed; everything else optional). Mirrors the
 * Rust `RemoteModel`.
 */
export interface RemoteModel {
  id: string
  name?: string
}

/** How DSH reaches the endpoint: the wire protocol in `settings.yaml`. The
 * backend passes the value through unvalidated — this set is the UI's
 * convenience list, not an authoritative enum. `''` = omit the field. */
export type ProviderApi = 'openai-completions' | 'openai-responses' | 'anthropic' | ''

export interface ApiProvider {
  id: string
  /** The key DSH sees (`llm-pi-ai.providers.<name>`); stable, filesystem-safe. */
  name: string
  kind: 'official' | 'aggregator' | 'custom'
  notes?: string
  api?: string
  baseURL?: string
  /** Environment variable name DSH reads the key from — what lands in
   * settings.yaml. Every provider needs one. */
  apiKeyEnv: string
  /**
   * Optional locally stored key. cc-switch model: the user pastes a real key,
   * PHL owns delivering it — at instance launch it is injected into the DSH
   * child's environment as `apiKeyEnv` (system/instance env always win). The
   * secret lives only in the local api.json and the process env; no instance
   * directory file ever contains it.
   */
  apiKey?: string
  models: ApiModelRef[]
  enabled: boolean
}

/** Points at a provider *by name* (DSH's own key space) + a model. */
export interface ApiDefaultModel {
  providerName: string
  model: string
  reasoningEffort?: string
}

export interface ApiConfig {
  version: 1
  updatedAt: string
  defaultProviderId?: string
  defaultModel?: ApiDefaultModel
  providers: ApiProvider[]
}

export type ApiInheritance = 'default' | 'custom' | 'none'

/** The binding stored on an instance (in `instance.json`'s `api` field). */
export interface ApiBinding {
  inheritance: ApiInheritance
  providerIds: string[]
  defaultModel?: ApiDefaultModel
  syncedAt?: string
  syncedHash?: string
}

export const defaultApiBinding = (): ApiBinding => ({
  inheritance: 'default',
  providerIds: [],
})

export const unmanagedApiBinding = (): ApiBinding => ({
  inheritance: 'none',
  providerIds: [],
})

/**
 * One instance's *actual* API state as read from its settings.yaml —
 * mirrors the Rust `InstanceLiveSnapshot`. The file is the truth; the
 * library only describes what 同步 would write, and 采纳 is the inverse
 * operation (instance → library) and stays a human decision.
 */
export interface InstanceLiveSnapshot {
  inheritance: ApiInheritance
  /** Parsed providers + default model as the file currently has them. */
  live: ApiConfig | null
  /**
   * Managed content (bound provider entries, the default model) was
   * modified or deleted inside the instance. Additions the instance made
   * are not changes — they are just unmanaged entries.
   */
  localChanges: boolean
  defaultModelChanged: boolean
  /** Bound providers whose `apiKeyEnv` is set nowhere PHL can observe. */
  missingKeys: string[]
}

/** Per-provider divergence classification for the live view. */
export type ProviderState =
  /** bound, present in the file, fields match the library */
  | 'matched'
  /** bound, but the file's entry differs from the library */
  | 'modified'
  /** bound, but the entry is gone from the file */
  | 'missing'
  /** in the file, not bound, and not in the library — an instance-side addition */
  | 'local-only'
  /** in the file and in the library, but this instance's binding excludes it */
  | 'unbound'

export interface ProviderLiveView {
  provider: ApiProvider
  state: ProviderState
  /** Field labels (端点/密钥变量/模型) that differ, for the 修改 badge tooltip. */
  diff: string[]
}
