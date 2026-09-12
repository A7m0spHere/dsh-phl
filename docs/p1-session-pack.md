# P1 实施记录：Session 迁移引擎与 `.phlpack`

对应《下一阶段开发规格》§29（P1-1~P1-4）。状态：四个子阶段全部实现并合入，自动化验证通过（见末尾）。
（2026-09-06 后续轮更新）P2 已完成（`phl-pack-core` crate + `phl-pack` CLI + `phl-export` Skill，
见 `docs/p2-pack-core-skill.md`）；本文的已知限制 1（手测登记）与 2（计数刷新）亦已在收尾轮解决。

本文只记录实现位置、与规格的偏差、验证与边界。进度状态入口由维护者在本地工作区维护（不随本仓库发布）。

## P1-1 Session 迁移引擎 — `src-tauri/src/sessions/`

文件：`codec.rs`（纯字节层）、`copy.rs`（复制引擎）、`mod.rs`（list/inspect/命令）、各 `*/tests.rs`。

- **codec**：DSH 会话日志 = 拼接的独立 zstd 校验帧（header 为 frame 0，其后每批一个帧），或明文 `.jsonl`。
  `decode` 把所有帧解成明文 → 取首行为 header；`rewrite_header_line` 只改 `id`/`parentSession`/`seedLength`，
  其余字段（含 `cwd`、`agentPreset`）原样保留（键序不变）；`build_copy` 把新 header + 原事件明文按源编码重封装。
  撕裂尾行（无换行结尾）按 committed-prefix 丢弃，不计入事件数。`SESSION_FORMAT_VERSION=0`，非 0 → `UnsupportedVersion` 硬拒。
- **copy**：`copy_session` 双端 `ensure_not_running` → 目标为 external 拒写 → 读源 → re-id →
  落 `<dstHome>/sessions/<同一 projectKey>/<新 session-id>/session.jsonl[.zstd]`，
  staging `.part` + 重解码校验（新 id/parent/事件数）+ 原子 rename，任一步失败删整个会话目录。
- 新会话 id 形如 `session-<uuidv4>`（`RandomState`+纳秒时钟，无需 uuid crate，见下「依赖」）。
- 多目标 `copy_sessions`：逐 (会话×目标) 独立，目标先全部预校验（自目标/external/运行中）再执行。
- 命令：`list_sessions` / `inspect_session` / `copy_session` / `copy_sessions`（`lib.rs` 注册）。

### 关键决策：为什么用原生 Rust zstd
P0 Spike 给了两条路线：①走 DSH 的 JS 运行时；②Rust 直接读写帧。离线 cargo 无 zstd crate，
但用户批准联网拉取；最终选定**原生 Rust**（更内聚、可单测、无子进程耦合）：
- 解码 `ruzstd` 0.8.2（纯 Rust，兼容 MSRV 1.77.2）；
- 编码 `zstd` 0.14.0（libzstd 静态构建，Windows/MSVC 编译通过）。
- **互解已实测验证**：Rust `zstd` 编码帧 → Node `zlib.zstdDecompressSync`（DSH 实际用的解码器）原样读回；
  ruzstd 读真实 DSH 帧（112 帧 / 199 行）全通过。

## P1-2 `.phlpack` 格式 + 校验器 — `src-tauri/src/pack/{format,mod}.rs`

- 容器：ZIP（复用已在依赖的 `zip` 2.x deflate）；布局 `phlpack.json` / `embedded/plugins/` / `overrides/` / `sessions/` / `assets/`。
- `phlpack.json`（`PhlPackManifest`，camelCase，独立 `formatVersion=1`）：pack 身份 + dsh/runtime 依赖 +
  plugins（registry | embedded 双来源，`required` 默认 true）+ content（sessionsIncluded/secretsExcluded/privacy）+
  integrity（路径→sha256）+ 可选 `environment`（Bundle 兼容段，见 P1-3）。
- 校验 `read_pack`（纯读中央目录 + header 帧，不解包）拒：缺 manifest、非 UTF-8/JSON、formatVersion>本 build、
  `../`/绝对/盘符路径穿越、软链条目、重复条目、超条目数/解压总字节/单条字节、manifest 声明的 embedded 路径缺 package.json、
  integrity 引用的文件缺失或哈希不符（防篡改/损坏，规格 §23）。错误 `PackError` → `[state]` 稳定码。
- `normalize_entry` 对条目名做与 `safe_join` 同纪律的组件级越界拒绝（含 Windows 反斜杠归一）。

## P1-3 导出 — `src-tauri/src/pack/export.rs` + `write.rs`

- `preview_instance_pack_export` → `PackExportPlan`：扫描受管实例（version/runtime/profile/DSH_HOME）、
  逐插件分类（`registry_available`=有 install 目录且 trust 为 verified/pinned → 可重下；否则 `embed_recommended`），
  读插件 `package.json`/LICENSE → `license_unknown` 时给 §10 提示（不判合法），列 `credential_names`（凭据名，值不随包走）。
- `export_instance_pack(id, dest, {embedRegistryIds, includeSessions, sessionsPrivacyAck})`：
  `PackBuilder`（`write.rs`）写 embedded 插件目录树 + 可选 sessions 树，逐文件记 sha256，最后写 `phlpack.json`；
  **sessions 需 `sessions_privacy_ack` 否则 `[state]` 拒**（§11 二次确认）；凭据经 `partition_env` 剥离；
  写完立即用 `read_pack` 自校验（导出的包必能被自己的安装器接受）。
- environment 段即 `InstanceBundle`（`phl_bundle:2`，含 credentials 名单）——规格 §15/§24 的 Bundle 兼容段，
  安装端按 Bundle 语义应用。

## P1-4 安装器 — `src-tauri/src/pack/install.rs`

- `preview_pack(path)` → `PackPreview`：`read_pack` 校验 → `resolve_dependencies` 逐依赖给
  `installed | downloadable | embedded | missing`，required missing → `blocked`（§18-20）；DSH/Runtime 未装标 `downloadable` 附提示（不硬阻，实例仍可建后补）。
- `install_pack(path, {manifest, allowMissing})`（`guarded`，锁 `Resource::Instance`）：**commit 时重开+重校验**（堵 TOCTOU）
  → staging `.phl-pack-<id>/dsh-home` → `unpack_pack` 把 embedded 插件解到 profile `node_modules`、sessions 解到 home/sessions、overrides 解到 home（逐文件 `confine` 防越界）→
  按 environment 段 `partition_imported_env` 再剥凭据、写 manifest（`management_mode=PackInstalled`、`source=Phlpack`、`adoptedFrom` 记包来源）→
  原子 rename 落位；任一步失败删 staging（不留半个实例）。返回 `deferredPlugins`（远程插件）交回既有插件安装管线。
- 远程插件**不在事务内自动联网安装**（插件安装是独立可续传、带进度与回滚的管线，见 §11/§7）；安装后由 UI 逐个触发，保持「PHL 是配置源 + 两个事务不纠缠」。

## 前端

- 桥接：`desktopSessions.ts`（list/inspect/copy/copySessions，多目标带进度 Channel）、`desktopPack.ts`
  （preview/export、preview/install、`.phlpack` 保存/打开对话框）；`desktop.ts` 按 O-13 模式再导出。均 `if(!isDesktop)` 降级（列表返回空、写操作抛「桌面端可用」）。
- UI：实例详情「数据」卡内联 `SessionCopyPanel`（选会话→选目标其它受管且非 external/未运行实例→复制，复制前确认）；
  `packStore` + `PackInstallPage`（选包→预览依赖→安装→入库 reload→回实例列表）；`PackExportPage`（预览→勾 embedded→
  会话勾选+隐私二次确认→保存路径→导出）；实例页「安装整合包」入口 + 实例菜单「导出整合包」；schema v2 的
  `pack-installed`/`source:phlpack` 在类型里可选透传。

## 与规格的偏差

1. **远程插件安装延后**：安装器把 registry 插件列为 `deferredPlugins` 交 UI，而非在 pack 事务内联网装（复用既有独立插件管线，避免两事务互相回滚纠缠）。§21 的「安装远程插件」步骤因此在提交后由 UI 触发，而非 install_pack 内部。
2. **DSH/Runtime 未装不硬阻**：preview 标 downloadable+提示，允许先建实例再由版本/Runtime 页补齐（与 bundle import 行为一致）；真正 `blocked` 仅 required 且无任何来源的插件。
3. **会话复制重封装为单帧**：copy 把整段明文重新编成一个 zstd 帧（读取 layout-blind），未保留源的多帧分组——DSH 读取与续写均正常（已在互解测试覆盖）。
4. **overrides 为前向槽**：PHL 自产包暂不写 overrides；安装器仍解包应用 `overrides/` 以兼容第三方包。
5. **子代理会话**：list 标记 `originSubagent`，UI 展示但不特殊处理；规格 §9 的「默认隐藏子代理会话」留待真机反馈再定。
6. **新 id 随机源**：用 `RandomState`+纳秒（非 uuid crate，因其 MSRV 高于本 build 钉的 1.77.2）；本地实例级唯一性足够，测试覆盖多份复制不同 id。

## 验证

- `cargo test --lib`：**241 通过 / 0 失败 / 5 忽略**。新增：sessions codec/copy/命令 21 项（含互解真实帧、re-id/lineage/cwd、撕裂尾、版本兼容、多目标、运行/external 门禁）；
  pack format/validator 12 项（防穿越/软链/重复/大小/版本/integrity 篡改）、export 3 项（round-trip、凭据剥离、隐私确认）、install 3 项（解包落位、confine、resolver 分类）。
- `cargo clippy --all-targets --all-features -D warnings` 通过；`cargo fmt --check` 干净。
- `npm run typecheck` 0 错；`npm test` **55 通过**（新增 packStore 4 项：pick/preview/失败/install 入库）；`vite build` 成功。
- DSH 字段与 models.dev 数据形状均已对本机真实安装 / 线上接口核实（非假设）。

## 已知限制 / 下一步

1. ~~真机手测未登记~~ → 已登记进维护者本地的手测清单 §12/§13（不随本仓库发布；Windows 真机执行待做，含真实 DSH 启动后能否看到迁移/安装的会话）。
2. ~~复制会话后计数需重开刷新~~ → 收尾轮已做即时失效（`SessionCopyPanel.onCopied` → 详情重测）。
3. `install_pack` 的解包阶段未接可中断取消（pack 体量小，原子回滚已保证无半成品）。
4. 远程插件自动装、`.phlpack` 文件关联/MIME、MCP/签名 PKI 等均按规格 §31 不在本阶段。
5. ~~P2 未开始~~ → 已完成，见 `docs/p2-pack-core-skill.md`。
