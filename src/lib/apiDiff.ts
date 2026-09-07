import type { ApiBinding, ApiConfig, ApiModelRef, ApiProvider, ProviderLiveView } from '@/types'

/**
 * Pure comparison helpers between an instance's live `settings.yaml` view
 * (what the file actually says) and the global library (what 同步 would
 * write). Lives in lib/ rather than the store because the UI composes it in
 * several places (badges, sync confirmation, adoption plan) and the logic is
 * the part worth keeping honest.
 *
 * Field semantics: a live provider parsed from YAML only has fields that
 * were present in the file; DSH omits empty ones, so `undefined` vs `''`
 * counts as equal everywhere below.
 */

const norm = (s?: string) => (s && s.trim() ? s : undefined)

function modelsEqual(a: ApiModelRef[], b: ApiModelRef[]): boolean {
  if (a.length !== b.length) return false
  // Provenance belongs to PHL; compare only fields materialized into DSH.
  const key = (m: ApiModelRef) => JSON.stringify([
    m.id, norm(m.name), m.contextWindow, m.maxTokens,
    m.input === undefined ? undefined : [...m.input].sort(),
    m.reasoningEfforts && Object.entries(m.reasoningEfforts).sort(([a], [b]) => a.localeCompare(b)),
  ])
  const ka = [...a].map(key).sort()
  const kb = [...b].map(key).sort()
  return ka.every((k, i) => k === kb[i])
}

/** Human-readable labels for what differs; drives the 修改 badge tooltip. */
export function providerFieldDiff(live: ApiProvider, lib: ApiProvider): string[] {
  const d: string[] = []
  if (norm(live.api) !== norm(lib.api)) d.push('协议')
  if (norm(live.baseURL) !== norm(lib.baseURL)) d.push('端点')
  if (live.apiKeyEnv !== lib.apiKeyEnv) d.push('密钥变量')
  if (!modelsEqual(live.models, lib.models)) d.push('模型清单')
  return d
}

/** Which library providers a binding currently pulls in. */
export function boundProviders(binding: ApiBinding, config: ApiConfig): ApiProvider[] {
  if (binding.inheritance === 'default') return config.providers.filter((p) => p.enabled)
  if (binding.inheritance !== 'custom') return []
  return binding.providerIds
    .map((id) => config.providers.find((p) => p.id === id))
    .filter((p): p is ApiProvider => !!p)
}

/**
 * The full live view: bound providers classified by whether the file agrees,
 * then file entries outside the binding shown as additions or unbound.
 * A bound provider missing from the file (the user deleted it in DSH) shows
 * as `missing` — it is a change 同步 would undo.
 */
export function classifyInstance(
  binding: ApiBinding,
  config: ApiConfig,
  live: ApiConfig | null,
): ProviderLiveView[] {
  const byName = new Map((live?.providers ?? []).map((p) => [p.name, p]))
  const out: ProviderLiveView[] = []
  const bound = boundProviders(binding, config)
  const boundNames = new Set(bound.map((p) => p.name))
  for (const lib of bound) {
    const actual = byName.get(lib.name)
    if (!actual) {
      out.push({ provider: lib, state: 'missing', diff: [] })
      continue
    }
    const diff = providerFieldDiff(actual, lib)
    out.push({ provider: actual, state: diff.length ? 'modified' : 'matched', diff })
  }
  for (const [name, actual] of byName) {
    if (boundNames.has(name)) continue
    const inLibrary = config.providers.some((p) => p.name === name)
    out.push({ provider: actual, state: inLibrary ? 'unbound' : 'local-only', diff: [] })
  }
  return out
}

/**
 * The adoption plan: what folding an instance's live state back into the
 * library would do. Deliberately pure so the UI can show it in a
 * confirmation before anything is written — 采纳 is a human decision.
 */
export interface AdoptionPlan {
  add: ApiProvider[]
  update: { id: string; name: string; patch: Partial<ApiProvider> }[]
  /** Bound providers the instance deleted; offered as an opt-in removal. */
  removeMissing: { id: string; name: string }[]
  defaultModel?: ApiConfig['defaultModel']
}

export function planAdoption(
  binding: ApiBinding,
  config: ApiConfig,
  live: ApiConfig | null,
): AdoptionPlan {
  const plan: AdoptionPlan = { add: [], update: [], removeMissing: [] }
  if (!live) return plan
  const libByName = new Map(config.providers.map((p) => [p.name, p]))
  const byName = new Map(live.providers.map((p) => [p.name, p]))

  for (const [name, actual] of byName) {
    const lib = libByName.get(name)
    if (!lib) {
      plan.add.push(actual)
      continue
    }
    const diff = providerFieldDiff(actual, lib)
    if (diff.includes('协议') || diff.includes('端点') || diff.includes('密钥变量')) {
      plan.update.push({
        id: lib.id,
        name,
        patch: { api: actual.api, baseURL: actual.baseURL, apiKeyEnv: actual.apiKeyEnv },
      })
    }
    if (diff.includes('模型清单')) {
      const entry = plan.update.find((u) => u.id === lib.id)
      if (entry) entry.patch.models = actual.models
      else plan.update.push({ id: lib.id, name, patch: { models: actual.models } })
    }
  }

  const boundIds = new Set(boundProviders(binding, config).map((p) => p.id))
  for (const lib of config.providers) {
    if (boundIds.has(lib.id) && !byName.has(lib.name)) {
      plan.removeMissing.push({ id: lib.id, name: lib.name })
    }
  }
  if (live.defaultModel) plan.defaultModel = live.defaultModel
  return plan
}

export function adoptionPlanIsNoop(plan: AdoptionPlan): boolean {
  return (
    plan.add.length === 0 &&
    plan.update.length === 0 &&
    plan.removeMissing.length === 0 &&
    !plan.defaultModel
  )
}
