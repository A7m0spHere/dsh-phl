# P0 实施记录：本机 DSH 发现与接入

对应《PHL 下一阶段开发规格》第 2、3、4 部分的 P0 项（§28 开发顺序）。
状态：P0-1、P0-2、P0-3 已实现并合入本工作树，自动化验证通过（见「验证」）。
（2026-09-06 后续轮更新）P1、P2 均已完成；本记录的「按对话选择迁移」遗留项已在
P0/P1 收尾轮闭环（`SessionStrategy::Selected` 实装 + 向导选择列表），
真机手测矩阵已登记进手测清单 §11——实现与登记位置见 `docs/p2-pack-core-skill.md`。

本文只记录「实现位置 + 与规格的偏差 + 已知边界」；规格本身以 `*.md` 为准，
进度状态入口由维护者在本地工作区维护（`PHL_OPTIMIZATION_ROADMAP.md`，不随本仓库发布）。

## P0-3 Session Spike（先行）

- 产出：`docs/research/dsh-session-integration.md`。
- 方法：对本机真实安装（`versions/0.1.2-alpha.5`、`0.1.2-rc.1`）的类型声明、
  `dsh-base/cordis.patch.yml`、以及真实 home（`~/.dsh`、一个实例 home）实读；
  并用 Node 内建 zstd 实测解出一个会话 header 验证格式链路。
- 关键结论（支撑 P0-1/P0-2，避免猜测）：
  - DSH_HOME 解析优先级：显式配置 > `$DSH_HOME`（空=未设） > `~/.dsh`。
  - persistence root = `<DSH_HOME>/sessions`（由 base profile `!!js dshHomePath('sessions')` 固定）。
  - `SESSION_FORMAT_VERSION = 0`，backend 拒绝其它版本、无迁移；两被检版本一致。
  - 物理：`<root>/<projectKey(cwd)>/session-<id>/session.jsonl[.zstd]`，拼接 zstd 帧、
    header 单独一帧；默认 packed-chunk 行；撕裂尾帧需只取完整帧。
  - seed/fork：`parentSession` + `isSeeded` + `inheritedEventCount`（物理 `seedLength`）；
    复制 = 新 id header + seed 源事件 + 继承计数（DSH 一等公民语义）。

## P0-1 DSH Discovery

实现位置：`src-tauri/src/discovery/`。

| 规格 §28 子项 | 实现 |
|---|---|
| Candidate 数据结构 | `candidate.rs::DshCandidate`（字段覆盖规格列表 + `size_bytes` 预览用） |
| 默认路径扫描 | `inspect::default_homes()`：`$DSH_HOME` + `~/.dsh`（镜像 dsh-home-paths 优先级） |
| PATH / 环境变量扫描 | `inspect::find_dsh_executable()`（PATH 与 `%APPDATA%\npm`）；`CandidateSource::Path` 合成候选 |
| 手动选择 | 命令 `inspect_dsh_home`（选目录）、`inspect_dsh_executable`（选可执行文件，home 按规则推断） |
| 去重 | `candidate::home_key`（canonical + 分隔符/大小写归一）按目录身份去重，手动来源优先 |
| 已被 PHL 管理检测 | `managed_homes()` 扫描 `<root>/instances/*/dsh-home`，`alreadyManaged` + `managedInstance`；并检测落在 instances 下但无在册实例的目录 |
| Discovery UI | `src/pages/AdoptDshPage.tsx` 的 `DiscoverStep`：候选卡片（路径/版本/插件数/对话数/体积/来源/警告）、重新扫描、两个手动入口；被管理项置灰并说明 |

- **与规格偏差（结构）**：规格建议 `windows.rs/macos.rs/linux.rs` 三个平台文件。
  实现统一在 `inspect.rs`（`dirs` + 环境变量本身跨平台），Windows 专属点（`dsh.cmd`、
  verbatim 路径）用 `cfg!(windows)` 分支处理；不预先写无法本机验证的 macos/linux
  死代码（对齐 ROADMAP：macOS/Linux 属 L-12 专项）。后续若做 L-12，再从 `inspect.rs`
  抽平台文件成本低。
- 命令注册：`discover_dsh` / `inspect_dsh_home` / `inspect_dsh_executable`（`lib.rs`）。
- 前端桥接：`src/lib/desktopAdoption.ts`（O-13 域拆分第二单元），`desktop.ts` 再导出；
  浏览器模式 `discoverDsh` 返回 `[]`、向导显示「桌面端功能」，不伪造结果。

## P0-2 Adoption

实现位置：`src-tauri/src/instances/adoption.rs` + manifest 扩展。

| 规格 §28 子项 | 实现 |
|---|---|
| Copy 模式 | `ManagementMode::ManagedCopy`：整树复制 DSH_HOME 进实例目录，源目录不动（测试 `copy_mode_..._leaves_the_source_intact`） |
| External 模式 | `ManagementMode::External`：登记 `externalHome`，不复制；`home_of` 让 launch/verify/plugin/记录都解析到真实 home |
| Preview | `preview_adoption` → `AdoptionPreview`（版本/插件/对话数/预计复制量/符号链接数/警告） |
| 插件检测 | 复用 `discovery::inspect::declared_plugins`（bundles + patch 合并、排除 core 作用域、需 node_modules 存在） |
| 配置检测 | 检测 `settings.yaml` 存在性；不读取其内容（凭据边界） |
| Session count | `inspect::count_sessions`（纯文件名 walk，不解析帧，spike §8）；另加 `instance_session_count` 供实例详情「数据」区 |
| 用户选择 Session 策略 | 向导单选「全部迁移 / 不迁移」；`Selected` 最初在命令层拒绝为「下一版本」，收尾轮已实装（见 `docs/p2-pack-core-skill.md` §1） |
| staging | `.phl-adopt-<id>` 暂存目录，成功才 `rename` 进位 |
| rollback | 任一步失败 / 取消 → 删除 staging，不留半个实例（对齐 clone/snapshot 既有模式） |
| manifest | schema v1→v2（见下） |

- **外部模式的危险操作门禁（规格 §3.4）**：所有写入实例 home 的命令在
  `instances::reject_external_write` 处统一拒绝——插件安装/卸载/启用
  （`writable_profile_dir`）、快照还原（`restore_snapshot_inner`）、克隆
  （`run_clone`）、API 同步（`sync_instance_api`）。快照**创建**放行（只读复制 home，
  安全）；`repair` 的 `recreate-skeleton` 对外部实例跳过 home 重建。删除实例对
  外部天然安全（外部 home 在实例目录之外，`remove_dir_all` 触不到），前端措辞改为
  「从 PHL 移除」。
- **与规格偏差**：
  1. 复制到 PHL 时，`settings.yaml`/`profiles`/`attachments` 随环境整体复制——
     这是「接入后可直接用」的前提；因此**复制模式不剥离 API Key**（与 `.phlpack`
     导出不同，后者是 P1、按规格 §12 必须过 secret 过滤）。已在 spike §2 记录边界差异。
  2. 配置/插件勾选：规格 §3.2 把它们列为可勾选项。当前**复制保持环境完整**，UI 以
     「随环境一起接入」信息清单呈现（含不迁移凭据、不复制 workspace 文件两条负向说明），
     而非可关闭开关——按目录复制无法只拷「一半的插件/配置」。
  3. 符号链接插件：复制时**跳过并计入 `symlink_entries`**，预览提示「接入后可按需重装」；
     不跟随链接到外部目标（安全）。`link:` 型本地插件因此不会随复制进来。
  4. 采纳后的实例 `api` 绑定为 `None`（自管），不套用全局库，避免覆写用户自带配置。

### Manifest schema v2

- 新增字段：`managementMode`（`managed-copy`|`external`|`pack-installed`）、
  `source`（`created`|`adopted`|`phlpack`）、`externalHome`（仅 external）、
  `adoptedFrom`（`{dshHome, detectedVersion, adoptedAt, mode}`）。
- 迁移：全部字段带「等于 v1 世界」的 serde 默认（v1 实例本就都是 PHL 自有副本 + 创建），
  故 v1 文件无损读入 v2，`write_manifest` 盖 v2 戳；对**高于**本 build 的版本仍拒绝猜测。
- 前端 `Instance` / `RemoteInstanceRecord` 同步扩展并**回传**这些字段，
  因为 `save_instance` 往返整份 manifest——不回传会被 serde 默认悄悄重置成
  managed-copy，使外部实例丢失 home 指向。测试 `manifest_v2_round_trips...` 守住此边界。

## 前端

- 路由：新增 `adopt`（`uiStore` Route/routePath/parseRoute/isAncestor + `Router.tsx`），
  实例页头部新增「接入本机 DSH」入口（`InstancesPage`）。
- 状态：`src/stores/adoptionStore.ts`（扫描态 + 向导 draft + 预览 + 提交），提交走
  `instanceStore.admitInstance` + `load()`，与 Bundle 导入同一道门。
- 向导：`AdoptDshPage.tsx`（发现→接入方式→预览→进度，进度条走 `Channel` 字节回调）。
- 实例详情：新增「数据」区（会话计数 + 来源 + 外部说明）、头部「原地接入/接入的副本」
  徽标；菜单对外部实例禁用「克隆/创建快照」。

## 验证

- `cargo test --lib`：192 通过 / 0 失败 / 5 忽略（忽略项为既有真机项）。
  含 discovery 10 项、adoption 8 项（copy/external/rollback 边界、alreadyManaged 拒绝、
  pack/selected 拒绝、session 排除精确性、v1→v2 往返、外部克隆拒绝）。
- `cargo clippy --all-targets -D warnings`：通过。`rustfmt`：本次新增/改动文件均干净
  （未对并发他人的文件跑全局 `cargo fmt`）。
- `npm run typecheck` 通过；`npm test` 51 通过（含 adoptionStore 5 项：external 强制
  all 策略、已被管理不前进、失败不入库、成功 admit+reload）；`npm run build` 通过，
  AdoptDshPage 独立分包（~13.9 kB）。

## 已知限制 / 下一步

1. persistence root 若被用户 patch 改址：发现/会话计数只认 `<home>/sessions`，检测不到
   如实报 0 + 警告，不做全盘搜索（spike §3、§9）。
2. 复制模式对超大 home 目前无「剩余空间预检」（磁盘满会在复制中失败并回滚）——P1 安装器
   的空间检查可复用。（仍未接，登记在收尾轮已知限制）
3. ~~「按对话选择迁移」留待 P1~~ → 已实现：Selected 复制排除会话库后按选定目录原样迁入
   （原 id，迁移非分发语义），见 `docs/p2-pack-core-skill.md` §1。
4. ~~真机手测尚未登记~~ → 已登记进维护者本地的手测清单 §11（不随本仓库发布；执行仍待 Windows 真机）。
