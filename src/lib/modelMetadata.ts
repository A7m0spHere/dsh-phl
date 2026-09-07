import type { ApiModelRef, ModelMetadataBatch, ModelMetadataField } from '@/types'

export const metadataFields: ModelMetadataField[] = ['name', 'contextWindow', 'maxTokens', 'input', 'reasoningEfforts']
export const hasMissingMetadata = (model: ApiModelRef) => metadataFields.some((field) => model[field] === undefined)

export function editModelField<K extends ModelMetadataField>(model: ApiModelRef, field: K, value: ApiModelRef[K]): ApiModelRef {
  const metadataSources = { ...model.metadataSources }
  if (value === undefined) delete metadataSources[field]
  else metadataSources[field] = 'manual'
  return { ...model, [field]: value, metadataSources }
}

export function metadataSummary(batch: ModelMetadataBatch): string {
  if (batch.catalogStatus === 'mock') return '浏览器预览不查询模型目录；请在桌面端补全。'
  const matched = batch.results.filter((r) => r.matched).length
  const changed = batch.results.filter((r) => r.changed).length
  const ambiguous = batch.results.filter((r) => r.ambiguous).length
  const status = batch.catalogStatus === 'stale' ? ' · 使用旧缓存' : batch.catalogStatus === 'unavailable' ? ' · 目录暂不可用' : ''
  return `已识别 ${matched} 个模型，补全 ${changed} 个，${batch.results.length - matched} 个未找到${ambiguous ? `（含 ${ambiguous} 个匹配歧义）` : ''}${status}`
}

function capacity(value: number): string {
  // Exact powers only: never round 400000 up to 400K (=409600).
  if (value >= 1048576 && value % 1048576 === 0) return `${value / 1048576}M`
  if (value >= 1024 && value % 1024 === 0) return `${value / 1024}K`
  return value.toLocaleString()
}

export function modelCapabilities(model: ApiModelRef): string {
  return [
    model.contextWindow !== undefined && `${capacity(model.contextWindow)} 上下文`,
    model.maxTokens !== undefined && `${capacity(model.maxTokens)} 输出`,
    model.input && model.input.map((v) => v === 'image' ? '图片' : '文本').join(' / '),
    model.reasoningEfforts === false ? '无思考' : model.reasoningEfforts ? `思考 ${Object.keys(model.reasoningEfforts).join(' / ')}` : '思考未声明',
  ].filter(Boolean).join(' · ')
}

export function modelSources(model: ApiModelRef): string {
  const sources = new Set(metadataFields.filter((key) => model[key] !== undefined).map((key) => model.metadataSources?.[key] ?? 'manual'))
  return [...sources].map((source) => source === 'fallback' ? '默认兼容值' : source === 'manual' ? '已有 / 手动配置' : 'models.dev').join(' + ')
}
