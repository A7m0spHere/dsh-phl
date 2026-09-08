# DSH Session 集成 Spike（P0-3）

调研日期：2026-09-06。研究对象为本机真实安装的 DSH：
`E:/PHL-DSH/versions/0.1.2-alpha.5` 与 `0.1.2-rc.1`（PHL 受管版本目录），
以及默认 home `~/.dsh` 与 PHL 实例 home `E:/PHL-DSH/instances/11-9z3d/dsh-home` 的真实数据。
所有结论均来自安装包内的类型声明、profile 组合文件与磁盘实读，无猜测项；
每个结论标注证据位置。两个被检版本的 Session 持久化格式一致（见 §5）。

## 1. DSH_HOME 解析（discover / 隔离的根基）

`@deepseek-ai/dsh-home-paths`：

- 优先级：显式配置路径 > `$DSH_HOME` 环境变量 > `~/.dsh`。
  空或全空格的 `$DSH_HOME` 视为未设置（不会退化到 cwd）。
- 常量：`DSH_HOME_DIR_NAME = ".dsh"`、`DSH_HOME_ENV = "DSH_HOME"`；
  支持 `~` / `~/` / `~\` 展开。
- 证据：`node_modules/@deepseek-ai/dsh-home-paths/lib/types/index.d.ts`
  （`resolveDshHome`、`dshHomePath`、`expandHomePath`）。

**对 PHL 的意义**：实例隔离靠注入 `DSH_HOME`（launch/mod.rs 既有约定）成立；
自动发现时「一个 DSH 环境」= 一个 DSH_HOME 目录，默认探测点就是 `~/.dsh` 与
`%DSH_HOME%`（若已设置）。

## 2. DSH_HOME 内部布局（本机实测）

```text
<DSH_HOME>/
├── settings.yaml          # 全局设置（UI、默认模型、provider 配置…）
├── profiles/<name>/       # 各 profile：cordis.yml / cordis.patch.yml /
│                          #   node_modules / package.json / pnpm-lock.yaml
├── sessions/              # ★ Session 持久化根（§3）
├── storages/              # KV 存储：session_projcache（投影缓存，可重建）、
│                          #   session_projcache.json、workspace.json
├── attachments/v1/        # 内容寻址附件库（消息体按引用解析）
├── bin/                   # 工具垫片（本机为 pnpm.cmd，供依赖安装）
└── <插件私有目录>          # 例：llm-deepseek/、super-injector/、skills/、
                           #     preset-backups/ —— 由插件按自身约定写入
```

证据：`ls ~/.dsh`、`ls E:/PHL-DSH/instances/11-9z3d/dsh-home`。
注意：实例 home 与默认 home 布局完全同构，复制目录树即迁移环境的思路可行。

`settings.yaml` 中凭据形态：本机只出现 `apiKeyEnv`（环境变量名引用），
未出现字面 `apiKey:`/`token:` 值。不能保证所有用户部署都如此——
`.dsh` 顶层的插件目录（如 `llm-deepseek/`）可能含供应商凭据数据。
**Adoption 复制整个 home 时这些随环境一起走（这正是「接入后可直接用」的前提）；
`.phlpack` 导出时按规格 §12 必须过 secret 过滤，两者边界不同。**

## 3. Session 持久化根如何确定（persistence root）

- 组合点在基础 profile：`@deepseek-ai/dsh-base/cordis.patch.yml`：

  ```yaml
  - id: session-persistence-jsonl
    name: '@deepseek-ai/dsh-session-persistence-jsonl'
    config:
      root: !!js dshHomePath('sessions')
  ```

  即 **persistence root = `<DSH_HOME>/sessions`**，单一根、跨 profile 共享
  （不按 profile 分目录）。storages 同理：`root: !!js dshHomePath('storages')`。
- 证据：`E:/PHL-DSH/versions/0.1.2-rc.1/node_modules/@deepseek-ai/dsh-base/cordis.patch.yml:110-113,151`。

**风险提示**：root 由 profile 组合决定，理论上可被用户 patch 改到任意位置。
PHL 的 sessionCount / 迁移入口应从 `<DSH_HOME>/sessions` 起步，
同时在文档中保留「被自定义 patch 挪走」这一已知限制（不做全 home 搜索，避免误报）。

## 4. 磁盘布局（JSONL backend）

`@deepseek-ai/dsh-session-persistence-jsonl/lib/types/format.d.ts`：

```text
<sessions>/
└── <projectKey(cwd)>/                 # 人类可读项目目录，如 --D-AI~9879~76EE-dsh-phl--
    └── <session 目录>/                 # 本机实测形如 session-<uuid>
        └── session.jsonl.zstd          # 单个会话一个文件
```

- `projectKey(cwd)`：分隔符→`-`，非安全码元→`~XXXX`（UTF-16 码元级转义，
  单射可逆；`~` 也被转义，因此目录名无歧义碰撞）。cwd 缺失 → `_no-cwd`。
  分隔符替换与截断**有意有损**，目录名只作导航用途，身份以文件内 header 为准。
- `logPath = <root>/<projectKey>/<sessionDir>/session.jsonl[.zstd]`；
  后缀由物理编码决定：`zstd`（默认）或 `none`（明文 `.jsonl`）。
- **同一 root 内两种编码互斥**（`ensureRootEncoding` 拒绝混用；
  另有被拒绝的 obsolete 扁平布局 `rejectLegacyFlatArtifact`）。
- 本机实测目录名与编码：全部为 `session-<uuid>/session.jsonl.zstd`，
  项目目录命名与 `projectKey` 规则吻合。

## 5. 记录格式与版本

### Header（第一行，独立一个 zstd 帧）

物理 `HeaderLine`（format.d.ts）与逻辑 `SessionHeader`（`@deepseek-ai/dsh-session`）：

```json
{"type":"session","version":0,"id":"session-43ac60c9-…","createdAt":1788219392720,
 "cwd":"D:\\AI项目\\dsh-phl","delegationDepth":0,"agentPreset":"ptc"}
```

（此行是从 `~/.dsh/sessions` 真实文件用 Node `zlib.zstdDecompressSync` 解出的原文。）

- `SessionHeader` 字段：`version, id, createdAt, cwd?, parentSession?, isSeeded,
  origin?: 'subagent', delegationDepth?, agentPreset?`。
- `parentSession` = fork 血缘；`isSeeded` = 含继承前缀；
  **精确继承长度 `inheritedEventCount` 存在 storage metadata 里，
  物理 header 上对应 `seedLength?`**（dsh-session-persistence `SessionStorageMetadata`）。
- 版本机制：`SESSION_FORMAT_VERSION = 0`（dsh-session/types.d.ts:55）。
  **backend 在 load 时拒绝任何其他 version，明确无迁移**
  （`sessionFormatVersionRefusal`）。
- 0.1.2-alpha.5 与 0.1.2-rc.1 的同名常量与布局一致。

### 事件行

- append-only、seq 连续；每个持久化批次一个 zstd 帧拼接在 header 帧后。
- 默认 `packChunks: true`：连续的 delta 事件打包成 `text-chunks` /
  `reasoning-chunks` / `tool-call-chunks` 存储行（有损于文件观感、无损于事件序列）；
  **读取端 layout-blind**（`scanLog` 总能还原事件流），因此外部工具不能假设一行一事件。
- 撕裂尾帧 = 崩溃中断位点，backend 读取时丢弃并可在 load 时做崩溃修复
  （合成 `interrupted` 关闭事件）。**只读复制场景必须自行处理撕裂尾**：
  复制时按 `scanZstdFrames` 只取完整帧。

## 6. `ctx.sessionPersistence` API 面（运行中 DSH 的服务）

`@deepseek-ai/dsh-session-persistence/lib/types/index.d.ts`，方法：

| 方法 | 语义 | PHL 用途 |
|---|---|---|
| `list(signal)` | 每会话只读 header（不解析全日志） | 会话计数、列表 |
| `listSnapshots(signal)` | header + 变更 token | 增量监视 |
| `inspect(id)` | header + 完整逻辑事件，不做修复提交 | 迁移读源 |
| `load(id)` | 读并**提交崩溃修复** | 迁移不要用（有写副作用） |
| `readFrom(id, fromSeq)` | 后缀读 | 投影类需求 |
| `readRaw(id)` | 原始帧文本（verbatim） | 导出原始 artifact |
| `locate(meta)` | 纯函数解析目标路径，不碰磁盘 | 路径预览 |
| `create(meta, inheritedEventCount?)` | 登记 header（可懒物化） | 写入新会话 |
| `append(id, events)` | 落盘一批事件 | 写入新会话 |
| `prepare(id)` / `borrowSession(id)` | 恢复用的不可变借用 | PHL 不需要 |

关键约束（写入路径）：`create` 对已存在日志拒绝（`rejectExistingLog`）；
seeded 会话首批必须覆盖完整继承前缀；append 首事件 seq 必须等于存储的 next-seq。

## 7. Seed / Fork 语义（对话复制的推荐做法）

`@deepseek-ai/dsh-session` `CreateSessionOptions`：

```ts
{ seed: readonly SessionEvent[],           // 继承的事件前缀
  inheritedEventCount: SessionLogOffset,   // 精确继承长度
  meta: { cwd?, parentSession?, isSeeded?: true, createdAt?, agentPreset?, … } }
```

复制一个历史对话 = 新 id 的 header（`parentSession` 指向源、`isSeeded: true`、
继承 `cwd`）+ `seed` 装入源事件 + `inheritedEventCount` = 源事件数。
恢复路径（`RestoredSessionOptions`、`seedSource: 'persistence'`）证明
「从存储事件重建一个可续聊会话」是 DSH 一等公民语义——规格 §5.1 的目标行为
在 DSH 侧原生成立。

**P1 实现路线建议（按优先）**：

1. **走 DSH 自己的 JS 运行时**：PHL 在目标实例的 profile 环境里执行一段注入脚本
   （或未来 DSH 提供 CLI），用真实 `JsonlSessionPersistence` + `SessionStore`
   完成 list/inspect/create/append。好处：格式与崩溃语义由 DSH 自己保证。
   成本：需要解析 profile 内模块路径、启动 cordis ctx（较重）。
2. **直接操作 JSONL 容器（Rust 侧）**：读源文件 = 扫完整 zstd 帧 + `scanLog` 等价的
   行解析；写目标 = 新 header 行帧 + 原事件行帧（header 的 `id` 必须改写为新 id，
   `seedLength`/`inheritedEventCount` 按源设置）。可行且纯本地，但**承担了格式跟随
   义务**：`SESSION_FORMAT_VERSION` 或打包行结构一旦变化，PHL 必须同步升级，
   并且每次读都校验 header `version` 与目录编码一致性（root 编码混用会被 DSH 拒绝，
   PHL 造文件时必须匹配目标 root 已有编码）。
3. 规格 §5.4 的写保护与路线无关：**源会话所属实例在跑就不要动它的持久化文件**；
   目标 root 若从未物化过会话，两种物理编码都可以由 PHL 择一建立。

## 8. 对 P0-1 / P0-2 的直接影响（本次就要用的结论）

- `sessionCount`（发现阶段，只数不读）：统计 `<DSH_HOME>/sessions/*/*/session.jsonl*`
  的文件数即可，无需解析任何内容；`sessions/` 不存在时按 0 处理并给 warning。
- **不解析事件内容**：P0 的发现与接入只按目录树整体复制/排除 `sessions/` 与
  `storages/`，不碰单条会话，规避撕裂帧与 packed-row 的解析义务。
- 复制排除表（「不迁移历史对话」）：`sessions/`、`storages/session_projcache*`
  （投影缓存是会话数据的派生物，留着会与缺失的会话不一致）。
  `settings.yaml`、`profiles/`、`attachments/`、插件目录随环境复制。
- 原地接入（external）：PHL 只写 manifest 引用该 home，不复制、不动 `sessions/`。
- DSH 可执行文件的发现锚点：版本目录顶层 `package.json` 的
  `@deepseek-ai/dsh`（本机 `lib/bin.js`）；`dsh` 是否上 PATH 因安装方式而异
  （本机 PATH 上就没有），PATH 扫描要有但不做主渠道。

## 9. 已知限制与后续验证项

1. persistence root 可被用户 patch 覆盖 —— 发现/迁移入口以 `<home>/sessions` 为约定，
   检测不到时如实报告而非全盘搜索。
2. PHL 受管实例启动时 profile 固定 `web`；被发现的第三方 DSH 可能用其他 profile，
   复制接入时整树复制规避此差异。
3. zstd 编解码在 Node ≥22.15 / 24 有内建 `zlib` zstd（本机 v24 实测可解）；
   DSH 的 backend 用 Node 私有解码器 + 公共 API 兜底。PHL Rust 侧如需读写帧，
   选 `ruzstd`（解码）+ `zstd`（编码，需 checksum flag）——列入 P1 spike 附属验证。
4. `origin: 'subagent'` 会话是否应出现在用户可选的对话列表里：建议默认隐藏
   （子代理会话不可独立续聊），P1 落地时定夺。
