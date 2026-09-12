# P2 实施记录 + P0/P1 收尾：选择迁移、平台发现、Pack Core/CLI、Skill

对应《下一阶段开发规格》§30（P2-1/P2-2）与 §3.2/§3.3、§2.1、§2.2、§26、§32 的收尾项。
状态：全部实现并通过自动化验证（见末尾）；§11–§14 真机手测与 macOS/Linux 真机（L-12）未执行。
进度状态入口由维护者在本地工作区维护（不随本仓库发布）；本文只记录实现位置、偏差与边界。

## 1. 接入向导「按对话选择迁移」（规格 §3.2/§3.3，P0 遗留闭环）

- 后端 `instances/adoption.rs`：`SessionStrategy::Selected` 不再是拒绝路径。
  复制阶段与 `None` 相同（排除顶层 `sessions` 与派生投影缓存），随后
  `import_selected_sessions` 把选定会话目录**原 id、原字节布局**迁入实例 home
  （`locate_session_source` 走与 Session 引擎相同的发现遍历；逐目录 verbatim 复制、
  跳过符号链接；staging 回滚语义不变——任何一个选定目录缺失即整单失败）。
- 新命令 `list_adoption_sessions(path)`：向导的对话清单入口（路径寻址、只读，先过
  `classify_home` 形状闸，复用 `sessions::list_in_home` 的 header 读取）。
- `AdoptionRequest` 增 `sessionDirs`（serde 缺省空数组）；`AdoptionPreview` 增
  `selectedSessionCount`；preview 对 Selected 计量 = 环境排除量 + 选定会话树体积，
  且校验选定目录存在（不存在→列出名字报错，防提交期才炸）。
  校验顺序：external+非 All 拒 → 空选择拒（"不迁移"有自己的策略）→
  目录名过 `sanitize_session_dir`（`session-` 前缀白名单，`..` 进不了 FS 层）→ 存在性。
- 前端：`desktopAdoption.ts` 请求/预览类型扩展 + `listHomeSessions` 桥
  （`desktopSessions.ts` 会话域新函数，浏览器模式抛）；`adoptionStore.ts` 增
  `sourceSessions/selectedSessionDirs`（选第一个候选进入 selected 时懒加载；
  换候选清缓存；空选择不发预览请求）；`AdoptDshPage.tsx` 第三个单选项启用 +
  `SessionPicker`（行样式对齐实例详情 `SessionCopyPanel`）。

### 与规格的关系与一个有意的语义差别

规格 §3.3 说"调用 Session Migration Engine 导入选定 Session"。引擎的复制入口
（`sessions::copy`）语义是**分发**：重铸 id、`parentSession` 记血缘（§5.1/§1.3）。
接入向导的语义是**迁移**（场景 A："以前的对话还在"）：目标是全新的 home、无碰撞风险，
原样保留 id 才是"这些对话还在"的诚实形态,若重铸会把用户的原对话变成"某条不存在的父
对话的子代理"。因此 Selected 复用引擎的**发现遍历**而非 re-id 复制；跨实例分发仍走
原引擎。此为规格措辞的实现取舍,已在此记录。

## 2. 其余收尾项

- **计数即时刷新**（P1 已知限制 2）：`SessionCopyPanel` 成功后 `onCopied` →
  实例详情 `sessionCountTick` 重测;面板在重测期间保持挂载,选择不丢。
- **导入 Bundle 降级**（§26）：实例页三主操作 = 新建实例 / 接入本机 DSH / 安装整合包,
  Bundle 移入「更多导入方式」`Menu`。
- **Discovery 平台路径**（§2.1/§2.2）：拆出 `discovery/{windows,macos,linux}.rs`。
  共享代码只剩纯平台逻辑;`inspect::executable_roots()` = PATH ∪ 平台根。
  macOS/Linux 关键事实:**Finder/桌面启动不继承 shell PATH**,故显式探测
  `/usr/local/bin`、`/opt/homebrew/bin`(mac)、`/usr/bin`(linux)、`~/.npm-global/bin`、
  `~/.volta/bin`、`~/.bun/bin`、`~/.local/bin`、nvm `versions/node/*/bin`(一层有界展开,
  `nvm_bin_dirs` 放在 inspect.rs 以便任意宿主单测)。Linux 的 flatpak/snap home 重定向
  刻意不猜,留给手动选择。**真机验证属 L-12**。非宿主平台文件以
  `#[cfg(test)] #[path="macos.rs"] mod macos_check;` 编进测试构建——任何 OS 上 `cargo test`
  都对三平台代码做类型检查与单测,消灭"从没编译过的死代码"。
  偏差保留:未发现按平台分叉的 home 布局差异（`~/.dsh` 全平台一致,spike 已证),
  故平台文件只承载命令根,不做无法验证的 home 猜测。

## 3. P2-1：`phl-pack-core` crate + `phl-pack` CLI（规格 §15/§30）

- **workspace 根设在 `src-tauri/Cargo.toml`**（members：`.` + `crates/phl-pack-core` +
  `crates/phl-pack-cli`）:仓库根设 workspace 会把 target/bundle 路径整体搬家,
  破坏 `build-app.cmd` 与 tauri 打包的既有路径假设;crate 布局为
  `src-tauri/crates/phl-pack-core`(与规格示意同构,路径不同,已在此记录)。
- **core 内容**（宿主无关:无 tauri/无实例存储/无凭据策略）:
  `format.rs`(schema+`validate_manifest`,原样迁移)、`lib.rs`(容器校验 `read_pack`、
  integrity、`PackError`——`into_string` 的 `[state]` 编码移到宿主边界 `pack_error()`,
  core 只给 `detail()`/`Display`)、`write.rs`(`PackBuilder` 公有化 + 新增
  `build_pack_from_dir`:从 §6.1 布局目录打包、自动重算 integrity、成品回读自校验)、
  `unpack.rs`(解包 + dest-root confinement:词法归一后仍越界即拒,Gate 在核心层,
  宿主与 CLI 共用;`install.rs` 的本地 `confine`/手写解包环删除,改为
  `unpack_pack` 薄映射:embedded→profile/node_modules、sessions→home/sessions、
  overrides→home)。
- 宿主 `src-tauri/src/pack/{export,install}.rs` 保留(实例扫描/凭据剥离/staging/
  回滚/resolver 是产品胶合,不属格式),经 `pack/mod.rs` 的兼容 re-export 读同一命名空间
  (`crate::pack::format::…`、`crate::pack::write::PackBuilder`、`read_pack_from_path`)。
  规格 §15 的 secret filtering 一项有意留在宿主:凭据**名单**属 PHL credential store
  边界(AGENTS.md 安全约定),core 只承载格式与 privacy 元数据;不做为搬代码而复制策略。
- **CLI**(`crates/phl-pack-cli`,bin 名 `phl-pack`,零新增依赖、手写参数):
  `inspect <pack> [--json]` / `validate <pack>` / `unpack <pack> <dir>` /
  `build <布局目录> <out.phlpack>`。规则:`build` 拒绝覆盖已存在输出(覆盖确认归
  Skill/人,规格 §16),`unpack` 先跑完整校验再写字节,一切动作走 core——GUI 与
  Skill 永不漂移出第二套格式(Spec §14.2 的架构在代码层的落实)。
  测试:CLI 3 项(build→validate→inspect→unpack 闭环、覆盖/缺 manifest 拒绝、坏包不落盘),
  core 23 项(迁入的 12 项容器测试 + builder 5 + unpack confine 4 + build_from_dir 2),
  宿主 pack 胶合测试全部保留原断言。

## 4. P2-2：`phl-export` DSH Skill 文档

`docs/skills/phl-export/SKILL.md`:面向"没有 PHL 的 DSH 用户"。侦察(§2.2 的 DSH_HOME
解析规则、版本/插件/会话计数与发现模块同源)→ 插件分类(registry vs embedded、
许可读取、"不判合法")→ 敏感逐项确认(本地插件/会话双确认/未知文件/覆盖输出,
对齐规格 §9/§11/§16)→ secrets 边界(不原样带 `settings.yaml`,凭据值零进入)→
组装 §6.1 布局目录 + 手写 `phlpack.json`(唯一由 Skill 产出的文件)→ `phl-pack build` +
`validate`。禁止事项汇总:不发明格式、不手算哈希、不读会话正文、不猜凭据识别。
Skill 本身是文档交付物(DSH 侧加载),其行为验收在手测清单 §14。

## 4b. Review 修复:核心层凭据**文件**过滤(§12/§15)

端到端跑 CLI 时暴露一个真实缺口:凭据剥离此前只覆盖实例 env 的**键值**
(`partition_env`),而 `add_tree` 逐字节复制插件目录/会话树时**只跳软链**。一个
本地插件目录里若带 `.env`,它会原样进包,而 manifest 仍写 `secretsExcluded: true`
—— 撒谎的隐私声明,违反 §12「`.env` 中明显凭据…不得进入 Pack」。规格 §15 本就
把 secret filtering 列为 Pack Core 职责,故此修复落在 core:

- `PackBuilder::add_tree` 现返回 `TreeAdd { added, withheld_secrets }`,遍历中
  跳过命中 `is_secret_entry_name` 的文件并登记;`build_pack_from_dir`(CLI/Skill
  路径)经 `collect_tree_files` 应用同一规则,扣下名单经出参透出、CLI 打印。
- `is_secret_entry_name` 是**文件名级、零误伤**的白名单族:`.env`(排除
  `.example/.sample/.template/.dist`)、SSH 私钥 `id_*`、`credentials.json`、
  `token.json`、`.npmrc/.netrc/.pypirc`。**刻意不做** `contains("token")` 之类
  子串匹配(会静默丢 `tokenizer.js`,正是本项目拒绝的「环境悄悄缺一半」);
  不读文件内容(§33:不假装能识别正文里的秘密)。
- 宿主 `export.rs`:预览期 `collect_secret_files` 扫插件目录,把「哪个插件带
  疑似凭据文件」写进 `PackExportPlan.secret_files` + warning;导出后
  `PackExportReport.secret_files_withheld` 如实列出扣下了什么。前端
  `PackExportPage` 在扣下发生时弹 warn toast(与成功 toast 并列),不静默。
- 新增/更新测试:core `secret_names_are_exact_families_not_substrings`、
  `build_from_dir_withholds_secret_shaped_files_and_reports_them`(含
  `.env.example` 必须照常进包的反例);e2e 用 CLI 实证 `.env` 不再落包。
  这是"纵深防御兜底",不替代 Skill 第 4 步的自觉清理(藏于非典型文件名的 key
  仍识别不了,已在 SKILL.md 标明)。

## 5. 验证

- `cargo test --workspace --all-targets --all-features`:**268 通过 / 0 失败 / 5 忽略**
  (dsh-phl lib 233、phl-pack-core 32、phl-pack-cli 3)。Core 新增压缩/哈希/解包大文件中途取消测试。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过;`cargo fmt --all --check` 干净。
- `npm run typecheck` 0 错;`npm test` **73 通过**;`npm run build` 成功；
  `npm run bridge:check` 验证 17 个 bridge 文件 / 71 个 invoke 无缺失 handler。
- R6 前后基准与 R7 协作式取消详见 `PERF_BASELINE.md`、手测清单 §15。
- 手测矩阵:§32/§33 已按场景登记进维护者本地的手测清单 §11–§14（不随本仓库发布），
  **执行**(Windows 真机)未开始;macOS/Linux 发现路径真机属 L-12。

## 6. 已知限制 / 下一步

1. 规格 §31 的排除项(市场/云同步/账号/签名/文件关联)未动,符合计划。
2. Selected 迁移不处理"只选了子代理对话而其父未选"的血缘断链:目录原样迁入后 DSH
   侧 `parentSession` 可能指向本 home 不存在的外部 id——分发面板同样允许此形态,
   真机反馈若要求再收紧。
3. `unpack_entries_to` 的 confine 是词法归一 + starts_with,不解引用符号链接:
   staging/实例根由 PHL 自建,无符号链接注入面;第三方包条目已被
   `read_pack` 软链闸挡住,双保险成立。
4. `phl-pack` 无发布通道(未随安装包分发);Skill 使用前需自取二进制,发布接线
   (O-01 CI)后续再排。
5. workspace 化后 `cargo test` 需在 `src-tauri/` 下跑 `--workspace`(或逐包);
   根目录无 Cargo.toml 是有意决定(见 §3)。
