import { WandSparkles, X } from 'lucide-react'
import { Button, Input, Select } from '@/components/ui'
import type { ApiModelRef, ModelMetadataField } from '@/types'
import { editModelField, modelCapabilities, modelSources } from '@/lib/modelMetadata'

const levels = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

export function ModelRow({ model, onChange, onRemove, onEnrich }: {
  model: ApiModelRef
  onChange: (model: ApiModelRef) => void
  onRemove: () => void
  onEnrich: () => void
}) {
  const edit = <K extends ModelMetadataField>(key: K, value: ApiModelRef[K]) => onChange(editModelField(model, key, value))
  const efforts = model.reasoningEfforts
  return (
    <div className="rounded-lg bg-surface-sunken p-2.5 ring-1 ring-inset ring-line">
      <div className="flex items-center gap-2">
        <Input aria-label="模型 ID" value={model.id} placeholder="模型 ID（如 openai/gpt-5）" className="min-w-0 flex-1 font-mono"
          onChange={(e) => onChange({ ...model, id: e.target.value, metadataSources: undefined })} />
        <Button size="sm" variant="ghost" disabled={!model.id.trim()} onClick={onEnrich}>
          <WandSparkles size={12} />补全信息
        </Button>
        <Button size="sm" variant="ghost" aria-label="移除模型" onClick={onRemove}><X size={12} /></Button>
      </div>
      {model.name && <p className="mt-2 break-words text-sm font-medium text-ink">{model.name}</p>}
      <p className="mt-1 break-words text-sm leading-relaxed text-ink-muted">{modelCapabilities(model)}</p>
      {modelSources(model) && <p className="mt-1 text-xs text-ink-faint">来源：{modelSources(model)}</p>}
      <details className="mt-2 text-sm">
        <summary className="cursor-pointer text-accent-ink">编辑详细字段</summary>
        <div className="mt-2 grid grid-cols-2 gap-2 border-t border-line pt-2">
          <label className="col-span-2 space-y-1 text-ink-muted">显示名
            <Input value={model.name ?? ''} onChange={(e) => edit('name', e.target.value || undefined)} placeholder="留空可自动识别" />
          </label>
          <label className="space-y-1 text-ink-muted">上下文窗口
            <Input type="number" min={1} step={1} value={model.contextWindow ?? ''} onChange={(e) => edit('contextWindow', e.target.value ? Number(e.target.value) : undefined)} />
          </label>
          <label className="space-y-1 text-ink-muted">最大输出
            <Input type="number" min={1} step={1} value={model.maxTokens ?? ''} onChange={(e) => edit('maxTokens', e.target.value ? Number(e.target.value) : undefined)} />
          </label>
          <label className="space-y-1 text-ink-muted">输入模态
            <Select aria-label="输入模态" value={model.input === undefined ? 'unset' : [...model.input].sort().join(',')}
              onChange={(e) => edit('input', e.target.value === 'unset' ? undefined : e.target.value === '' ? [] : e.target.value.split(',') as ApiModelRef['input'])}>
              <option value="unset">未设置（继承 DSH）</option><option value="">空列表（继承 DSH）</option>
              <option value="text">文本</option><option value="image,text">文本 / 图片</option><option value="image">图片</option>
            </Select>
          </label>
          <label className="space-y-1 text-ink-muted">思考能力
            <Select aria-label="思考能力" value={efforts === undefined ? 'unset' : efforts === false ? 'disabled' : 'custom'}
              onChange={(e) => edit('reasoningEfforts', e.target.value === 'unset' ? undefined : e.target.value === 'disabled' ? false : { high: 'high' })}>
              <option value="unset">未设置（继承 DSH）</option><option value="disabled">不支持思考</option><option value="custom">手动声明档位</option>
            </Select>
          </label>
          {efforts && <div className="col-span-2 space-y-1.5">
            <p className="text-xs text-ink-faint">勾选已确认支持的档位，并填写接口使用的值；off 留空表示 null。</p>
            {levels.map((level) => <div key={level} className="flex items-center gap-2">
              <label className="flex w-24 items-center gap-2 text-ink-muted"><input type="checkbox" className="accent-accent" checked={level in efforts}
                onChange={(e) => {
                  const next = { ...efforts }
                  if (e.target.checked) next[level] = level === 'off' ? null : level
                  else delete next[level]
                  edit('reasoningEfforts', next)
                }} />{level}</label>
              {level in efforts && <Input aria-label={`${level} 接口值`} className="min-w-0 flex-1 font-mono" value={efforts[level] ?? ''}
                onChange={(e) => edit('reasoningEfforts', { ...efforts, [level]: level === 'off' && !e.target.value ? null : e.target.value })} />}
            </div>)}
            {!Object.keys(efforts).some((key) => key !== 'off') && <p className="text-xs text-warn">至少声明一个思考档位，或选择“不支持思考”。</p>}
          </div>}
        </div>
      </details>
    </div>
  )
}
