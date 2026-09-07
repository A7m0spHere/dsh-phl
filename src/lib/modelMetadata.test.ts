import { expect, it } from 'vitest'
import { editModelField, hasMissingMetadata, metadataSummary, modelCapabilities, modelSources } from './modelMetadata'
import { providerFieldDiff, planAdoption } from './apiDiff'
import type { ApiModelRef, ApiProvider } from '@/types'

it('preserves explicit false, empty input and an empty map as present values', () => {
  const model: ApiModelRef = { id: 'x', name: '', contextWindow: 1, maxTokens: 1, input: [], reasoningEfforts: false }
  expect(hasMissingMetadata(model)).toBe(false)
  expect(hasMissingMetadata({ ...model, reasoningEfforts: {} })).toBe(false)
  expect(hasMissingMetadata({ ...model, maxTokens: undefined })).toBe(true)
})

it('manual edits change provenance for exactly the edited field', () => {
  const model: ApiModelRef = { id: 'x', contextWindow: 200000, maxTokens: 32768, metadataSources: { contextWindow: 'models.dev', maxTokens: 'fallback' } }
  const edited = editModelField(model, 'maxTokens', 4096)
  expect(edited.metadataSources).toEqual({ contextWindow: 'models.dev', maxTokens: 'manual' })
  expect(model.maxTokens).toBe(32768)
  expect(editModelField(edited, 'maxTokens', undefined).metadataSources).toEqual({ contextWindow: 'models.dev' })
})

it('shows mixed sources and never labels a fallback as catalog data', () => {
  const model: ApiModelRef = { id: 'x', name: 'mine', contextWindow: 1048576, maxTokens: 32768,
    input: ['text', 'image'], reasoningEfforts: { high: 'high' },
    metadataSources: { contextWindow: 'models.dev', maxTokens: 'fallback' } }
  expect(modelSources(model)).toContain('默认兼容值')
  expect(modelSources(model)).toContain('models.dev')
  expect(modelSources(model)).toContain('已有 / 手动配置')
  expect(modelCapabilities(model)).toBe('1M 上下文 · 32K 输出 · 文本 / 图片 · 思考 high')
  expect(modelCapabilities({ id: 'x', reasoningEfforts: false })).toBe('无思考')
})

it('summary distinguishes misses, ambiguity and unavailable catalog', () => {
  expect(metadataSummary({ catalogStatus: 'unavailable', results: [{ model: { id: 'x' }, matched: false, changed: true, ambiguous: true }] }))
    .toContain('已识别 0 个模型，补全 1 个，1 个未找到（含 1 个匹配歧义） · 目录暂不可用')
  expect(metadataSummary({ catalogStatus: 'mock', results: [] })).toContain('浏览器预览')
})

const provider = (model: ApiModelRef): ApiProvider => ({ id: 'p', name: 'custom', kind: 'custom', enabled: true, apiKeyEnv: 'KEY', models: [model] })

it('drift and adoption include capabilities, ignore provenance and mapping order', () => {
  const lib = provider({ id: 'x', input: ['text', 'image'], reasoningEfforts: { high: 'high', off: null }, metadataSources: { input: 'models.dev' } })
  const reordered = provider({ id: 'x', input: ['image', 'text'], reasoningEfforts: { off: null, high: 'high' } })
  expect(providerFieldDiff(reordered, lib)).toEqual([])
  const live = provider({ id: 'x', input: ['text'], reasoningEfforts: false })
  expect(providerFieldDiff(live, lib)).toEqual(['模型清单'])
  const config = { version: 1 as const, updatedAt: '', providers: [lib] }
  const plan = planAdoption({ inheritance: 'default', providerIds: [] }, config, { ...config, providers: [live] })
  expect(plan.update[0].patch.models).toEqual(live.models)
  expect(providerFieldDiff(provider({ id: 'x' }), provider({ id: 'x', reasoningEfforts: false }))).toEqual(['模型清单'])
})
