import type { ApiProvider } from '@/types'

/**
 * Built-in vendor presets for the "new provider" form.
 *
 * This mirrors what cc-switch / Cherry Studio / LiteLLM ship: a curated table
 * of mainstream providers so the common path is "pick a name, paste a key,
 * fetch models" instead of typing a base URL by hand. A preset only prefills
 * the form — every field stays editable, and "自定义" opens the same form
 * empty. Nothing here is persisted as a new concept: on save it produces an
 * ordinary ApiProvider.
 *
 * Provenance & honesty notes:
 * - base URLs are the documented OpenAI-compatible endpoints (Chinese regions
 *   for CN vendors); cross-checked against Cherry Studio's provider-registry,
 *   cc-switch's provider presets and LiteLLM's provider map.
 * - No models are prefilled, deliberately: model sets change faster than any
 *   shipped table, and several vendors (豆包/OpenRouter/硅基流动/Ollama)
 *   key their ids to the account or the local install. The single honest
 *   source is "获取可用模型" — presets only open the door to it.
 * - apiKeyEnv carries the ecosystem-conventional variable name, so a key the
 *   user already exported works without any further config.
 */

export interface VendorPreset {
  id: string
  /** Display name in the chip strip (short, Chinese). */
  label: string
  /** Written into `name` (the settings.yaml key) when picked. */
  providerName: string
  /** Written into `notes` so the Chinese label survives after save. */
  notes: string
  kind: ApiProvider['kind']
  api: string
  baseURL: string
  /** Env-var *name* (never a value). */
  apiKeyEnv: string
  /** Where to create a key; surfaced as a hint link in the form. */
  consoleUrl?: string
}

export const VENDOR_PRESETS: VendorPreset[] = [
  {
    id: 'deepseek',
    label: 'DeepSeek',
    providerName: 'deepseek',
    notes: '深度求索 · platform.deepseek.com',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://api.deepseek.com/v1',
    apiKeyEnv: 'DEEPSEEK_API_KEY',
    consoleUrl: 'https://platform.deepseek.com/api_keys',
  },
  {
    id: 'openai',
    label: 'OpenAI',
    providerName: 'openai',
    notes: 'platform.openai.com',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://api.openai.com/v1',
    apiKeyEnv: 'OPENAI_API_KEY',
    consoleUrl: 'https://platform.openai.com/api-keys',
  },
  {
    id: 'anthropic',
    label: 'Claude',
    providerName: 'anthropic',
    notes: 'Anthropic · console.anthropic.com',
    kind: 'official',
    api: 'anthropic',
    baseURL: 'https://api.anthropic.com/v1',
    apiKeyEnv: 'ANTHROPIC_API_KEY',
    consoleUrl: 'https://console.anthropic.com/settings/keys',
  },
  {
    id: 'moonshot',
    label: 'Kimi',
    providerName: 'moonshot',
    notes: '月之暗面 · platform.moonshot.cn',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://api.moonshot.cn/v1',
    apiKeyEnv: 'MOONSHOT_API_KEY',
    consoleUrl: 'https://platform.moonshot.cn/console/api-keys',
  },
  {
    id: 'zhipu',
    label: '智谱 GLM',
    providerName: 'zhipu',
    notes: '智谱 AI · open.bigmodel.cn',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://open.bigmodel.cn/api/paas/v4',
    apiKeyEnv: 'ZHIPUAI_API_KEY',
    consoleUrl: 'https://open.bigmodel.cn/apikey/platform',
  },
  {
    id: 'dashscope',
    label: '通义千问',
    providerName: 'qwen',
    notes: '阿里云百炼 · bailian.console.aliyun.com',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    apiKeyEnv: 'DASHSCOPE_API_KEY',
    consoleUrl: 'https://bailian.console.aliyun.com/?tab=model#/api-key',
  },
  {
    id: 'volcengine-ark',
    label: '豆包',
    providerName: 'doubao',
    notes: '火山方舟 · console.volcengine.com/ark · 模型 id 以控制台为准',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://ark.cn-beijing.volces.com/api/v3',
    apiKeyEnv: 'ARK_API_KEY',
    consoleUrl: 'https://console.volcengine.com/ark',
  },
  {
    id: 'minimax',
    label: 'MiniMax',
    providerName: 'minimax',
    notes: '稀宇 · platform.minimaxi.com',
    kind: 'official',
    api: 'openai-completions',
    baseURL: 'https://api.minimaxi.com/v1',
    apiKeyEnv: 'MINIMAX_API_KEY',
    consoleUrl: 'https://platform.minimaxi.com/user-center/basic-information/interface-key',
  },
  {
    id: 'openrouter',
    label: 'OpenRouter',
    providerName: 'openrouter',
    notes: '聚合网关 · openrouter.ai · 模型格式 vendor/model',
    kind: 'aggregator',
    api: 'openai-completions',
    baseURL: 'https://openrouter.ai/api/v1',
    apiKeyEnv: 'OPENROUTER_API_KEY',
    consoleUrl: 'https://openrouter.ai/settings/keys',
  },
  {
    id: 'siliconflow',
    label: '硅基流动',
    providerName: 'siliconflow',
    notes: 'SiliconFlow · cloud.siliconflow.cn · 模型格式 org/Model',
    kind: 'aggregator',
    api: 'openai-completions',
    baseURL: 'https://api.siliconflow.cn/v1',
    apiKeyEnv: 'SILICONFLOW_API_KEY',
    consoleUrl: 'https://cloud.siliconflow.cn/account/credential',
  },
  {
    id: 'ollama',
    label: 'Ollama（本地）',
    providerName: 'ollama',
    notes: '本地模型 · localhost:11434 · 不校验密钥，可配任意占位值',
    kind: 'custom',
    api: 'openai-completions',
    baseURL: 'http://localhost:11434/v1',
    // A real (if pointless) env-var name: the library validates that every
    // provider references one, and Ollama ignores whatever it resolves to.
    apiKeyEnv: 'OLLAMA_API_KEY',
  },
]
