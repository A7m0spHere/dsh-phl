# 接入本机 DSH 与会话/整合包迁移

> 2026-09-27 由四篇实施记录（P0 接入 / P1 会话与 `.phlpack` / P2 收尾 + Pack Core /
> P2 验收清单）合并而成。原文面向「实现位置 + 规格偏差 + 边界」三类读者，合并后按
> 用户可感知的功能域组织：**发现与接入 → 会话迁移 → `.phlpack` 导出/安装 →
> Pack Core/CLI 与导出 Skill**。规格条文号（§xx）指《PHL 下一阶段开发规格》，
> 该规格与进度状态由维护者在本地工作区维护，不随本仓库发布。

## 1. 本机 DSH 发现与接入（原 P0）

### 1.1 发现（Discovery）

| 能力 | 实现 |
|---|---|
| Candidate 数据结构 | `discovery/candidate.rs::DshCandidate`（字段覆盖规格列表 + `size_bytes` 预览用） |
| 默认路径扫描 | `inspect::default_homes()`：`$DSH_HOME` + `~/.dsh`（镜像 dsh-home-paths 优先级） |
| PATH / 环境变量扫描 | `inspect::find_dsh_executable()`（PATH 与 `%APPDATA%\npm`）；`CandidateSource::Path` 合成候选 |
| 手动选择 | 命令 `inspect_dsh_home`（选目录）、`inspect_dsh_executable`（选可执行文件，home 按规则推断） |
| 去重 | `candidate::home_key`（canonical + 分隔符/大小写归一）按目录身份去重，手动来源优先 |
| 已被 PHL 管理检测 | `managed_homes()` 扫描 `<root>/instances/*/dsh-home`，`alreadyManaged` + `managedInstance` |
| Discovery UI | `src/pages/AdoptDshPage.tsx` 的 `DiscoverStep`：候选卡片 + 手动入口；被管理项置灰 |

**平台文件结构**：`discovery/{windows,macos,linux}.rs` 拆分承载命令根。
macOS/Linux 关键事实：**Finder/桌面启动不继承 shell PATH**，故显式探测
`/usr/local/bin`、`/opt/homebrew/bin`(mac)、`/usr/bin`(linux)、`~/.npm-global/bin`、
`~/.volta/bin`、`~/.bun/bin`、`~/.local/bin`、nvm `versions/node/*/bin`（一层有界展开）。
Linux 的 flatpak/snap home 重定向刻意不猜，留给手动选择。非宿主平台文件以
`#[cfg(test)] #[path]` 编进测试构建——任何 OS 上 `cargo test` 都对三平台代码做
类型检查与单测。真机验证属 L-12（macOS/Linux 专项，未执行）。

### 1.2 接入（Adoption）

实现：`src-tauri/src/instances/adoption.rs` + manifest 扩展。

- **Copy 模式**（`ManagementMode::ManagedCopy`）：整树复制 DSH_HOME 进实例目录，
  源目录不动。复制保持环境完整（`settings.yaml`/`profiles`/`attachments` 随环境整体
  复制），因此**不剥离 API Key**（与 `.phlpack` 导出不同，后者按规格 §12 必须过
  secret 过滤）。符号链接插件跳过并计入 `symlink_entries`，预览提示「接入后可按需重装」。
- **External 模式**（`ManagementMode::External`）：登记 `externalHome`，不复制；
  `home_of` 让 launch/verify/plugin/记录都解析到真实 home。**写入门禁**：所有写入实例
  home 的命令在 `instances::reject_external_write` 统一拒绝——插件安装/卸载/启用、
  快照还原、克隆、API 同步；快照**创建**放行（只读复制）。删除外部实例对源目录天然安全。
- **Preview**：`preview_adoption` → 版本/插件/对话数/预计复制量/符号链接数/警告。
- **staging/rollback**：`.phl-adopt-<id>` 暂存目录，成功才 `rename` 进位；任一步失败
  删 staging，不留半个实例。
- **插件清单**：实例页从 profile 磁盘扫描 PHL 安装记录、Cordis patch 直挂插件，以及
  `dsh.profile.bundles` 中实际存在并声明 `dsh.bundle.patch` 的 DSH 原生 bundle。
  原生 bundle 标成只读外部管理项；PHL 不用单插件 patch/uninstall 流程改动它，启停或卸载仍由 DSH 管理。
- **按对话选择迁移**：`SessionStrategy::Selected`。复制阶段排除顶层 `sessions` 与
  派生投影缓存，随后 `import_selected_sessions` 把选定会话目录**原 id、原字节布局**
  迁入实例 home；任何一个选定目录缺失即整单失败。命令 `list_adoption_sessions(path)`
  是向导的对话清单入口。**有意的语义差别**：接入向导的语义是**迁移**（「以前的对话
  还在」：全新 home 无碰撞，原样保留 id 才是诚实形态），跨实例**分发**才走 Session
  引擎的重铸 id 路径——Selected 复用引擎的发现遍历而非 re-id 复制。

### 1.3 Manifest schema v2

新增字段：`managementMode`（`managed-copy`|`external`|`pack-installed`）、
`source`（`created`|`adopted`|`phlpack`）、`externalHome`（仅 external）、
`adoptedFrom`（`{dshHome, detectedVersion, adoptedAt, mode}`）。全部字段带「等于 v1
世界」的 serde 默认，v1 文件无损读入；高于本 build 的版本仍拒绝猜测。前端
`Instance`/`RemoteInstanceRecord` 必须回传这些字段（`save_instance` 往返整份
manifest，不回传会被 serde 默认悄悄重置成 managed-copy，使外部实例丢失 home 指向；
测试 `manifest_v2_round_trips...` 守住此边界）。

## 2. 会话迁移引擎（原 P1-1）

实现：`src-tauri/src/sessions/`——`codec.rs`（纯字节层）、`copy.rs`（复制引擎）、
`mod.rs`（list/inspect/命令）。

- **格式**（来自 P0-3 Spike，对本机真实安装实读验证）：DSH 会话日志 = 拼接的独立
  zstd 校验帧（header 为 frame 0，其后每批一个帧），或明文 `.jsonl`。物理布局
  `<DSH_HOME>/sessions/<projectKey(cwd)>/session-<id>/session.jsonl[.zstd]`。
  `SESSION_FORMAT_VERSION = 0`，非 0 → `UnsupportedVersion` 硬拒（DSH 无迁移）。
  DSH_HOME 解析优先级：显式配置 > `$DSH_HOME` > `~/.dsh`。
- **codec**：`decode` 把所有帧解成明文取首行 header；`rewrite_header_line` 只改
  `id`/`parentSession`/`seedLength`，其余字段（含 `cwd`、`agentPreset`）原样保留
  （键序不变）；`build_copy` 把新 header + 原事件按源编码重封装。撕裂尾行按
  committed-prefix 丢弃。完整调研记录见
  [docs/research/dsh-session-integration.md](./research/dsh-session-integration.md)。
- **copy**：双端 `ensure_not_running` → 目标为 external 拒写 → re-id → staging
  `.part` + 重解码校验 + 原子 rename，失败删整个会话目录。复制重封装为单帧
  （读取 layout-blind），DSH 读取与续写均正常（互解测试覆盖）。
- **zstd 选型**：解码 `ruzstd`（纯 Rust）、编码 `zstd`（libzstd 静态构建）。
  互解已实测：Rust 编码帧 → Node `zlib.zstdDecompressSync` 原样读回；ruzstd
  读真实 DSH 帧全通过。
- 命令：`list_sessions` / `inspect_session` / `copy_session` / `copy_sessions`（多目标，
  目标先全部预校验再执行）。

## 3. `.phlpack` 整合包（原 P1-2~P1-4）

### 3.1 格式与校验

容器 ZIP，布局 `phlpack.json` / `embedded/plugins/` / `overrides/` / `sessions/` /
`assets/`。`phlpack.json`（`formatVersion=1`，camelCase）：pack 身份 + 依赖 + plugins
（registry | embedded，`required` 默认 true）+ content（sessionsIncluded/secretsExcluded/
privacy）+ integrity（路径→sha256）+ 可选 `environment`（Bundle 兼容段 `phl_bundle:2`）。

校验 `read_pack`（纯读中央目录 + header 帧，不解包）拒绝：缺 manifest、非 UTF-8/JSON、
formatVersion 越界、`../`/绝对/盘符路径穿越、软链条目、重复条目、超限、integrity
哈希不符。错误 `PackError` → `[state]` 稳定码。

### 3.2 导出

`preview_instance_pack_export` → 逐插件分类（registry 可重下 vs 建议嵌入）、许可
检查、`credential_names`（凭据**名**，值不随包走）。`export_instance_pack`：
写 embedded 插件树 + 可选 sessions 树，逐文件记 sha256；**sessions 需
`sessions_privacy_ack` 否则拒**（§11 二次确认）；凭据经 `partition_env` 剥离；
写完立即 `read_pack` 自校验。

### 3.3 安装

`preview_pack` → 依赖逐项 `installed | downloadable | embedded | missing`，
required missing → `blocked`；DSH/Runtime 未装标 `downloadable` 附提示（不硬阻，
可先建实例再补）。`install_pack`（锁 `Resource::Instance`）：**commit 时重开+重校验**
（堵 TOCTOU）→ staging → 解包（embedded→profile `node_modules`、sessions→home/sessions、
overrides→home，逐文件 confine）→ 凭据再剥 → 原子落位；失败删 staging。
**远程插件不在事务内自动联网安装**：返回 `deferredPlugins` 交回既有独立插件管线
（带进度与回滚），避免两个事务互相回滚纠缠。

## 4. Pack Core、CLI 与导出 Skill（原 P2）

### 4.1 `phl-pack-core` crate + `phl-pack` CLI

- workspace 根设在 `src-tauri/Cargo.toml`（members：`.` + `crates/phl-pack-core` +
  `crates/phl-pack-cli`）。core 宿主无关（无 tauri/无实例存储/无凭据策略）：
  `format.rs`（schema 校验）、`lib.rs`（`read_pack`/integrity/`PackError`）、
  `write.rs`（`PackBuilder` + `build_pack_from_dir`）、`unpack.rs`（解包 + dest-root
  confinement）。宿主 `pack/{export,install}.rs` 保留产品胶合（实例扫描/凭据剥离/
  staging/回滚），凭据策略**有意留在宿主**：凭据名单属 PHL credential store 边界。
- CLI（bin `phl-pack`，零新增依赖）：`inspect [--json]` / `validate` / `unpack` /
  `build`。规则：`build` 拒绝覆盖已存在输出（覆盖确认归 Skill/人），`unpack` 先完整
  校验再写字节，一切动作走 core——GUI 与 Skill 永不漂移出第二套格式。

### 4.2 凭据**文件**层过滤（Review 抓到的缺口）

规格 §12 要求「`.env` 中明显凭据不得进包」，但剥离此前只覆盖实例 env **键值**，
插件目录里的 `.env`/SSH key 会随 `add_tree` 原样进包，而 manifest 仍撒谎标
`secretsExcluded: true`。修复落在 core：

- `PackBuilder::add_tree` 返回 `TreeAdd { added, withheld_secrets }`，跳过命中
  `is_secret_entry_name` 的文件并登记；预览与导出报告都点名扣下了什么
  （`PackExportPlan.secret_files` / `PackExportReport.secret_files_withheld`）。
- `is_secret_entry_name` 是**文件名级、零误伤**的白名单族：`.env`（排除
  `.example/.sample/.template/.dist`）、SSH 私钥 `id_*`、`credentials.json`、
  `token.json`、`.npmrc/.netrc/.pypirc`。**刻意不做**子串匹配（会静默丢
  `tokenizer.js`）、不读文件内容（不假装能识别正文里的秘密）。
- 最快验收（命令行）：布局目录 `embedded/plugins/mine/` 放 `.env` 与 `.env.example`
  → `phl-pack build` 输出点名扣下 `.env` 且 `.env.example` 仍在 → `unpack` 后
  `grep -r sk-` 搜不到。

### 4.3 `phl-export` DSH Skill

[docs/skills/phl-export/SKILL.md](./skills/phl-export/SKILL.md)：面向「没有 PHL 的
DSH 用户」——侦察（DSH_HOME 解析、版本/插件/会话计数与发现模块同源）→ 插件分类 →
敏感逐项确认（本地插件/会话双确认/覆盖输出）→ secrets 边界（凭据值零进入）→
组装布局目录 + 手写 `phlpack.json` → `phl-pack build` + `validate`。铁律：不发明
格式、不手算哈希、不读会话正文、不猜凭据识别。

## 5. 已知边界（合并自各篇的遗留清单）

1. persistence root 若被用户 patch 改址：发现/会话计数只认 `<home>/sessions`，检测不到
   如实报 0 + 警告，不做全盘搜索。
2. 复制模式对超大 home 无「剩余空间预检」（磁盘满会在复制中失败并回滚）。
3. Selected 迁移不处理「只选了子代理对话而其父未选」的血缘断链：迁过去
   `parentSession` 可能指向本 home 不存在的外部 id——当前允许，真机反馈若要求再收紧。
4. `unpack` 的 confine 是词法归一 + starts_with，不解引用符号链接：staging 根由 PHL
   自建无注入面，第三方包条目已被 `read_pack` 软链闸挡住，双保险成立。
5. 非典型文件名的硬编码密钥（如 `config.js` 里写死 token）三层都不识别，靠
   「含会话必须提示隐私」兜底（§33 明确不假装能扫正文）。
6. `phl-pack` CLI 无发布通道（未随安装包分发）；Skill 使用前需自取二进制。
7. macOS/Linux 发现路径的真机验证属 L-12，未执行。
8. R6 性能基准：`scripts/pack-benchmark.ps1`——64 MiB + 1000 文件、3 次中位数，
   build/validate/unpack 峰值工作集从 ~130 MiB 降至 ~5–6 MiB（详见
   `PERF_BASELINE.md`）。

## 变更记录

- 2026-09-06~12 四篇实施记录陆续成文（P0 / P1 / P2 / P2 验收清单）。
- 2026-09-27 合并为本文件；实现位置、规格偏差与边界内容未变，按功能域重排。
