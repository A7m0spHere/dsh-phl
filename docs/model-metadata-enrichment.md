# PHL 原生模型信息补全

实现日期：2026-09-06；2026-09-10 现代化（见下文「新模型适配」）。全局 `config/api.json` 是补全结果的配置源；补全本身不写实例，沿用现有保存、绑定、显式同步和实例导入流程。

## 新模型适配（2026-09-10）

上线后反馈：新发布的模型补不出信息。定位到四个叠加原因并全部修复：

1. **缓存过期即刷新**：models.dev 收录新发布通常以小时计，旧的七天 TTL 让「昨天发的模型」系统性不可识别。TTL 降为 24 小时；打开「模型与 API」页会惰性触发一次到期刷新（空批次，不写任何配置）；「补全缺失模型信息」旁提供「更新目录并补全」显式强制刷新。
2. **规范化匹配层（最低优先级兜底）**：端点 id 常带日期快照与别名后缀（`gpt-5-20250807`、`model-latest`、Bedrock `model:0`、点号/下划线变体）。在既有四级字面匹配全部落空后，新增一层：`_`/`.` 归一为 `-`、剥去数字日期段（6/8/10 位）、`-v<数字>`、`-latest/-preview/-exp/-chat/-beta`，且要求唯一候选——歧义仍返回 Ambiguous，绝不放松标准（`-vision` 这类产品词永不被剥）。与 LiteLLM 的通配键表、Cline 的 live-catalog 优先是同一路线。
3. **按 base URL 主机消歧**：过去消歧提示传的是用户自己的供应商显示名，几乎不可能等于目录 provider id。现在从供应商配置的 baseURL 主机映射目录 id（api.openai.com→openai、api.siliconflow.cn→siliconflow、openrouter.ai→openrouter 等），显示名提示降为最后兜底；同名跨供应商模型由此可唯一命中。
4. **fallback 可被目录修正**：旧规则只补 undefined 字段，导致首次补全落到「默认兼容值」的模型此后永远不再复查。现在带 `fallback` 来源标记的字段视为可覆盖：目录学到该模型后下一次补全会以真实值替换猜测；`manual` 与目录值仍不动。全局「补全缺失」的候选判定同步把 fallback-only 模型重新纳入。

另新增 **OpenRouter 第二目录**（开源常见做法，如 Cline 对 OpenRouter 路由直接拉取 `context_length`）：仅当 id 含 `/`（`vendor/model` 约定）或 baseURL 主机是 openrouter.ai 时请求公开的 `GET https://openrouter.ai/api/v1/models`（无密钥、只读、≤16 MiB、每日缓存于内存）。它只填 models.dev 没给出的 contextWindow 与 input，字段来源记 `openrouter`；不提供也不猜测 maxTokens/reasoning。可在 设置 → 下载 关闭。

## 上游核对与参考分析

完整阅读了 [dsh-model-info-fill](https://github.com/11zld22/dsh-model-info-fill/tree/693a259f287ae3c17196194e64b405c2b29be0f0) 的三个源码文件、测试、README、包清单、Cordis patch 和 MIT LICENSE。其实现将 models.dev 的 provider/model 两层结构展开成数组，以不区分大小写的完整 ID、去前缀和尾部 ID 顺序取第一个匹配；填入容量、模态和思考映射；缓存位于 DSH_HOME，七天刷新，失败沿用旧缓存。Host 监听 DSH settings 更新并写回，client 注入设置页按钮。

PHL 独立实现 Rust resolver，没有复制这些源码、UI 或资源，没有安装该 DSH 插件，也没有注册实例内的 settings 监听器。Model metadata enrichment inspired by dsh-model-info-fill.

DSH schema 核对基于 `deepseek-ai/deepseek-harness` 提交 `d347e703908d0406b7a7ef80e3a0e594d86b2215`：

- [config.ts](https://github.com/deepseek-ai/deepseek-harness/blob/d347e703908d0406b7a7ef80e3a0e594d86b2215/packages/llm/llm-pi-ai/src/config.ts) 的 `modelFields`、`reasoningEfforts` 和默认容量。
- [catalog.ts](https://github.com/deepseek-ai/deepseek-harness/blob/d347e703908d0406b7a7ef80e3a0e594d86b2215/packages/llm/llm-pi-ai/src/catalog.ts) 的 `PiAiModelProfile`、`MODALITIES`、`THINKING_LEVELS`、`resolveModelReasoning`。

实际字段为 `input?: ('text' | 'image')[]`、`reasoningEfforts?: false | Record<string, string | null>`。可用档位为 off/minimal/low/medium/high/xhigh/max；仅 off 可映射 null。未设置 reasoningEfforts 时继承 DSH 内置目录；false 禁用思考。空映射、只有 off、非 off 的 null 或空字符串均不是有效声明。PHL 可读取旧文件，补全不改已有字段；保存和下发时校验有效声明。

## 原生链路

`ModelEditor / apiConfigStore → PhlRepository.enrichModelMetadata → desktop.ts → Rust catalog.rs → models.dev / 本地缓存 / fallback → PHL api.json → 显式同步 → settings.yaml`。

IPC 使用 PHL 内嵌 Tauri 模块 `model-metadata`，能力限定为 `model-metadata:allow-enrich-model-metadata`，仅授权 main 窗口。这是随 PHL 编译的原生代码模块，不是 DSH 实例插件；不改变已有 Tauri 命令的权限模型。

三个入口：选择 `/models` 返回的模型后先补全再加入草稿；手动模型行的“补全信息”和编辑器“补全模型信息”；全局页面“补全缺失模型信息”。前两个随供应商表单保存，后者保存全局库。任何请求期间的模型编辑、供应商上下文修改或全局根目录切换会令旧结果作废。全局补全不会自动同步实例。

新建和编辑供应商共享补全等待状态：模型补全期间保存按钮禁用并显示“模型补全中…”，保存处理函数也会拒绝提前提交；结果加入草稿后才恢复保存。失败降级同样恢复保存，取消编辑会清除等待状态。回归命令为 `python scripts/model-enrichment-smoke.py --url http://localhost:5180`（先运行 `npm run dev`，需要 Python Playwright 和 Chrome），覆盖远程模型成功/失败、新建/编辑、单行/批量手动补全和取消重开，共 7 个场景。

## 解析与匹配

解析保留 provider ID。上下文取 `limit.context`，最大输出取 `limit.output`，仅接受正整数；输入只接受 DSH 支持的 text/image，不用 attachment 标志猜图片能力。reasoning 明确 false 才写 false；存在明确 `reasoning_options[type=effort].values` 时映射受支持的档位，off 仅在数据明确包含时加入。只有 reasoning=true、没有档位的信息不够，因此不补 reasoning。

匹配不区分大小写、不改变保存的模型 ID，按以下顺序处理：

1. 完整 ID 精确匹配；多个候选时用提供的 provider route 名称消歧，仍不唯一即停止。
2. 显式 provider/path 限定，从最长路径开始。例如 `openrouter/openai/gpt-5` 先查 openrouter 的 `openai/gpt-5`，再查 openai 的 `gpt-5`。
3. 逐级去掉前缀后精确匹配。已识别的 provider 限定仍生效。
4. 尾部 ID 查找，候选必须唯一，或被 provider 提示唯一消歧。

非唯一候选不会继续降低匹配等级寻找任意结果，也不会因目录顺序不同而改变答案。自定义 provider 名称不做模糊品牌猜测，不能拿 PHL 的 `p-...` UUID 当 catalog provider ID。

## 只补缺失字段与来源

已有 name/contextWindow/maxTokens/input/reasoningEfforts 都优先，包含显式 false、空字符串、空数组和空映射。仅 absent/undefined（Rust Option::None）可补。null 在可选顶层字段反序列化时视为缺失；reasoning 映射内的 off:null 保留。

PHL 的 `metadataSources` 按字段记录 `models.dev`、`fallback` 或 `manual`。无记录的已有值展示“已有 / 手动配置”。手动改值只更新该字段来源。混合补全展示多个来源，避免目录条目缺字段时把默认容量标成目录事实。修改模型 ID 保留显式字段并清除旧来源标记；如需重新识别某项，应先清空对应字段。

来源不写入 settings.yaml，也不参与实例差异比较或同步指纹。input/reasoningEfforts 双向同步，并参与差异检查及实例配置采纳；映射键顺序和输入列表顺序不产生虚假差异。

## 缓存与降级

`PHL_ROOT/cache/models-dev.json` 是 `{ version: 1, fetchedAt: Unix秒, data: models.dev原始JSON }`。24 小时内直接使用（2026-09-10 前为七天，见「新模型适配」），过期时在下一次补全尝试刷新；打开「模型与 API」页会惰性触发一次到期刷新（空批次）。每个 root 独立缓存，进程内互斥复用结果，刷新失败至少等待五分钟后再尝试，避免逐行连续请求。

固定公共地址为 [models.dev/api.json](https://models.dev/api.json)，不发送 API Key、Base URL 或用户模型配置。请求超时 12 秒，响应上限 32 MiB。刷新结果需能解析出有效条目才能以临时文件加 rename 替换缓存；无效响应保留旧缓存。磁盘写入失败仍可用本次内存数据。

无缓存且网络失败、未匹配或匹配歧义时，缺失容量使用 contextWindow=262144、maxTokens=32768，输入使用 text，标明“默认兼容值”。不编造显示名和 reasoning。目录命中但部分字段缺失时，也逐字段使用该策略。带 `fallback` 标记的值是猜测而非事实：目录恢复或更新后的下一次补全会用真实目录值替换它（`manual` 与已命中的目录值不受影响；2026-09-10 起生效，此前规则为“永不覆盖”）。

## 验证与边界

本次验证结果：`npm test` 51 项通过；`npm run typecheck`、`npm run build`、`cargo check` 通过；`cargo test --lib` 191 项通过、5 项忽略；`cargo clippy --all-targets --all-features -- -D warnings` 通过。真实目录探针另行显式运行通过，读取 7,563 条记录并验证 GPT-5 匹配和磁盘缓存写入。项目没有独立 npm lint 脚本。

Playwright 在 headless Chrome 检查实际组件（以测试 IPC 数据隔离真实密钥）：远程列表选择后补全、保留端点名称、手动按钮、已有容量和 false 保护、IPC 失败仍可添加、旧响应丢弃；480px 组件和实际页面 1000px/780px 宽度均无横向溢出，页面无运行时异常。这不等同于实际 Tauri WebView 的整机验收。

单元测试包含 parser、明确/缺失 reasoning、匹配优先级、provider 前缀、尾部歧义、顺序无关性、只补缺失值、来源混合、旧 JSON、YAML 往返、指纹排除来源、缓存新鲜/过期/损坏/刷新失败/磁盘写入失败，以及前端配置竞争和新增能力的采纳。

`cargo test --manifest-path src-tauri/Cargo.toml api_config::catalog::tests::live_catalog_probe --lib -- --ignored --nocapture` 可单独运行真实公开目录探针，使用独立临时目录并清理，不访问用户 API 密钥或实例。

浏览器预览的 resolver 是明确标注的 mock no-op，不请求 models.dev；真实网络和缓存只在桌面端运行。UI 采用摘要加展开编辑，沿用现有 tokens 与 useMotion。测试不替代实际安装包在每个旧 DSH 版本上的验收：不做版本专属 schema 协商、不推测网关自定义别名能力、不主动升级已保存的自动值。
