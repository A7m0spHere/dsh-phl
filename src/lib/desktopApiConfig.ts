import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type {
  ApiBinding,
  ApiConfig,
  InstanceLiveSnapshot,
  RemoteModel,
  ModelMetadataRequest,
  ModelMetadataBatch,
} from '@/types'

/**
 * The API-config half of the desktop bridge (§M2 domain split): the global
 * provider library, instance sync/import, the live settings.yaml snapshot,
 * provider model listing, and catalog enrichment. Re-exported from `desktop.ts`
 * so consumers are unchanged.
 */

/** The global provider library; `null` until the user creates one. */
export async function loadApiConfig(): Promise<ApiConfig | null> {
  if (!isDesktop) return null
  return invoke('load_api_config', {})
}

export async function saveApiConfig(config: ApiConfig): Promise<ApiConfig> {
  if (!isDesktop) throw new Error('API 配置库仅在桌面端可用')
  return invoke('save_api_config', { config })
}

/** Materialize a binding into the instance's dsh-home/settings.yaml. */
export async function syncInstanceApi(
  instanceId: string,
  binding: ApiBinding,
  config: ApiConfig,
): Promise<ApiBinding> {
  if (!isDesktop) throw new Error('API 配置同步仅在桌面端可用')
  return invoke('sync_instance_api', { instanceId, binding, config })
}

/** Read an instance's live settings.yaml back as a library seed. */
export async function importInstanceApi(instanceId: string): Promise<ApiConfig | null> {
  if (!isDesktop) return null
  return invoke('import_instance_api', { instanceId })
}

/**
 * Read one instance's live settings.yaml + how it compares to the library.
 * The returned `live` view is the truth for display; `localChanges` gates
 * overwrite confirmations, and `planAdoption` (client-side) drives 采纳.
 */
export async function instanceLiveSnapshot(
  instanceId: string,
  binding: ApiBinding,
  config: ApiConfig,
): Promise<InstanceLiveSnapshot> {
  if (!isDesktop) {
    return {
      inheritance: binding.inheritance,
      live: null,
      localChanges: false,
      defaultModelChanged: false,
      missingKeys: [],
    }
  }
  return invoke('instance_live_snapshot', { instanceId, binding, config })
}

/**
 * Ask a provider's endpoint for its live model listing. The key is resolved
 * Rust-side (temp input first, then the referenced env var) and never
 * persisted; `apiKey` here is held only in component memory. Errors from the
 * backend may be prefixed `ENV_MISSING:` — the UI keys its temp-key input on
 * that marker.
 */
export async function fetchProviderModels(args: {
  baseURL: string
  api?: string
  apiKeyEnv: string
  apiKey?: string
  /** Lets the backend fall back to this provider's OS-stored credential. */
  providerId?: string
}): Promise<RemoteModel[]> {
  if (!isDesktop) throw new Error('获取模型列表仅在桌面端可用')
  return invoke('fetch_provider_models', {
    baseUrl: args.baseURL,
    api: args.api ?? null,
    apiKeyEnv: args.apiKeyEnv,
    apiKey: args.apiKey ?? null,
    providerId: args.providerId ?? null,
  })
}

/** Optional catalog enrichment; callers persist via the existing API library flow. */
export async function enrichModelMetadata(
  args: ModelMetadataRequest,
): Promise<ModelMetadataBatch> {
  if (!isDesktop)
    return {
      results: args.models.map((model) => ({
        model,
        matched: false,
        changed: false,
        ambiguous: false,
      })),
      catalogStatus: 'mock',
    }
  return invoke('plugin:model-metadata|enrich_model_metadata', {
    models: args.models,
    provider: args.provider ?? null,
  })
}
