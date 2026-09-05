/** The agent default-model picker for a binding. */
import { ApiDefaultModel, ApiProvider } from '@/types'
import { Select } from '@/components/ui'

export function DefaultModelPicker({
  providers,
  value,
  onChange,
}: {
  providers: ApiProvider[]
  value?: ApiDefaultModel
  onChange: (dm: ApiDefaultModel | undefined) => void
}) {
  const provider = providers.find((p) => p.name === value?.providerName)
  return (
    <div className="flex items-center gap-2">
      <Select
        value={value?.providerName ?? ''}
        onChange={(e) => {
          const p = providers.find((pp) => pp.name === e.target.value)
          onChange(
            p
              ? { providerName: p.name, model: p.models[0]?.id ?? '', reasoningEffort: undefined }
              : undefined,
          )
        }}
        className="w-[160px]"
      >
        <option value="">未选择</option>
        {providers.map((p) => (
          <option key={p.id} value={p.name}>
            {p.name}
          </option>
        ))}
      </Select>
      {provider && (
        <Select
          value={value?.model ?? ''}
          onChange={(e) => value && onChange({ ...value, model: e.target.value })}
          className="w-[200px]"
        >
          {provider.models.length === 0 && <option value="">（无模型清单）</option>}
          {provider.models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id}
            </option>
          ))}
        </Select>
      )}
    </div>
  )
}
