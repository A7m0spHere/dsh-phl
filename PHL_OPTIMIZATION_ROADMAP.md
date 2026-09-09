# PHL 优化规划与长期发展清单

初版：2026-09-06，检查对象 HEAD `b4b9b9d`。§2 为该时点快照；§3/§5 已在同日执行轮更新（HEAD 至 `b365ed0` 系列提交），每项状态附证据提交。

## 1. 结论与产品方向

2026-09-09 发布就绪：**代码与供应链检查已就绪，实机验收按「维护者自测通过、问题走 issue」记账。** `cargo audit`（513 个 crate）**0 个漏洞**，7 条警告均为传递依赖的 unmaintained / unsound（`glib` 仅 Linux 目标编译），不引入豁免；安装包在 `acdb74e` 上重建（2,978,500 字节，SHA-256 `5B72B49F…7837`）。CI：`96546fc`、`cb04137` 全绿；`b8a8c91` 的 Rust job 一次 `cargo test --workspace` 失败但同代码在下一提交通过，本地连跑 10 轮全绿，判为 runner 偶发（再次出现需取日志定位）。**已发布 `v0.1.0-alpha.1`（prerelease）**：<https://github.com/A7m0spHere/dsh-phl/releases/tag/v0.1.0-alpha.1>，资产 `PHL_0.1.0-alpha.1_x64-setup.exe`（2,973,765 字节，SHA-256 `3D6FD01D…9AEB`，附 `.sha256`），由标签触发 `.github/workflows/release.yml` 在干净 runner 上重跑门禁后构建。详见 [发布就绪记录](docs/preview-release-readiness-2026-09-09.md)。

2026-09-09 P2 收口：**首轮 review 点名「公开发包前应一并修复」的四项 P2 已处理完毕。** P2-1 凭据与配置的提交顺序改为「先记旧值、失败回滚」，错误文案不再说谎；P2-2 快照明确**只替换 dsh-home**，不再暗示回滚版本绑定（按 review 允许的第二条路径）；P2-3 新增中断还原的自动回滚（备份放回、退役、清暂存）；P2-4 WebUI 窗口地址与请求不一致时先 navigate 再聚焦。门禁全绿：Rust workspace **352 项通过 / 6 忽略**（桌面库 316）、前端 **92 项 / 15 文件**。放行剩余条件仍为真机安装验收、故障注入与 `cargo audit`，以及在本轮修复之上重建安装包。详见 [P2 收口记录](docs/preview-release-p2-round-2026-09-09.md)。

2026-09-09 许可证：**选定 MIT。** 新增 `LICENSE`（Copyright (c) 2026 A7m0spHere）；`package.json` 与 `src-tauri/Cargo.toml` 声明 `MIT`；README 的许可证章节由「尚未选定、勿再分发」改为 MIT 说明；`THIRD_PARTY_NOTICES.md` 顶部注明本项目许可证，第三方清单保持不变。O-15 记录的「许可证维持发布前由维护者决定」到此关闭。

2026-09-09 接手复核与完善：确认 V1–V4 现有实现和回归有效，并补上后端拒绝重复启动未确认进程、迟到取消不得掩盖插件提交/恢复结果、最终固定 DSH_HOME 防止 Windows 大小写覆盖。最终 gate 全绿：前端 92 项，Rust 349 项通过 / 6 忽略。公开预览仍未放行：凭据/快照/WebUI 的其他历史问题以及真实安装验收未因本次四项修复而关闭。具体证据与产物记录见 [接手复核](docs/preview-release-v-reverification-2026-09-09.md)。

2026-09-09 V1–V4 复验关闭：**上一轮点名的四项已覆盖到实现与失败时序回归，但这不代表全项目已无已知缺陷，公开预览仍为 NO-GO。** V1 使用独立插件事务日志，Prepared / RollingBack / Committed 决定恢复，早写的包 marker 不再证明提交；V2 区分 Alive/Unknown/Exited；V3 核对迁移 journal 的 from/to，前端只同步后端已提交的根；V4 保留 child 的持续退出监听并处理前端早到退出事件。该修复提交的原始基线为 Rust 346 通过 / 6 忽略、前端 88 通过。旧审查中的凭据覆盖、快照绑定与 WebUI 代次等问题仍需独立收口；真实桌面与干净 Windows 安装验收、Rust 依赖审计也尚未完成。接手后的增量完善与最终验证见 [V1–V4 复验记录](docs/preview-release-v-reverification-2026-09-09.md)。

2026-09-09 修复审核验收：**未通过，公开预览仍为 NO-GO。** R4 并发旧快照覆盖已关闭，R1 原提交顺序已修复，但 R2 仍把提交第一步写入的插件 marker 当作完整提交、可能删除唯一旧包；R3 仍将进程查询失败视为退出，并缺少 kept-alive 后续监听。补充隔离回归实际复现插件旧包删除及残留 committed journal 导致下一次迁移假成功（2 项失败）。原有 gate 全绿（85 前端、331 Rust / 6 忽略）不覆盖这些失败场景。以下“R1–R4 已修复”是修复轮自报，验收以 [本轮复验报告](docs/preview-release-reacceptance-2026-09-09.md) 为准。

2026-09-09 复查续 · R1–R4 修复轮：本轮审查确认的四项 P1 代码问题已修复。R1：迁移提交改为单一操作——数据全部落位后，由 `move_root_data` 在同一 DataRoot 守卫内先切根指针、随 journal 一并清除（`PhlState::relocate_root`），journal 清理不再先于指针提交；若提交本身失败，已提交的 journal 保留，重启后新增「完成切换」恢复入口（`storage_migration_finish`，从 journal 自身记录取目标路径，不经 UI 重发）。R2：插件安装重试改为先恢复后删除——`recover_interrupted_swap` 在触碰任何目录前按磁盘状态判定中断阶段（旧包搬走未落位→放回；已落位未提交→移开新包放回旧包；提交标记完好→旧备份退役），唯一旧副本在新包提交前不会被删除；卸载同步清理本包名的 `.phl-old-*`/`.phl-tmp-*` 残留。R3：启动取消与 120 秒超时路径改为「确认退出才遗忘登记」（`terminate_and_forget`，kill 成功仍轮询内核确认）；无法确认退出时保留内存与持久登记，错误以 `[state] kept-alive <pid> <port>` 编码上抛，前端把实例呈现为运行中并提供停止重试入口；`stop_instance` 对「kill 失败但进程已消失」按停止成功清理。R4：进程登记改为单锁「变更+快照+落盘」串行提交，临时文件按提交序号命名，写入/替换失败不再静默（带受影响实例清单落日志）。验证：Rust workspace 331 项通过（新增迁移提交、恢复矩阵、终止确认、并发落盘等测试）、前端 85 项通过、typecheck/clippy/fmt/bridge 全绿。故障注入类验收（真实断电/硬退出）仍按放行清单第 3、4 条待真机执行。

2026-09-09 预览发行复查：**公开预览仍为 NO-GO，可继续内部 Alpha 试用。** 当前本地 `6caee5b` 与远端 `ea60b5d` 产品代码一致，最新 CI 成功；本轮 Alpha gate 全绿（84 项前端、317 项 Rust 通过 / 6 忽略）。新确认的 P1 是迁移删日志早于根指针提交、插件中断后重试删除旧包、启动取消/超时终止失败仍丢进程登记，以及并发 registry 持久化写回乱序。不能只补手测就放行，需先修这些代码问题。详见 [本轮完整审查与放行清单](docs/preview-release-review-2026-09-09.md)，旧日期结论仅作历史记录。

2026-09-08 Alpha 发行验收：当前 **NO-GO（暂不公开发行）**。最新发布提交 `6c01c76` 的 Windows CI 有两处短/长路径断言失败；接管进程退出监听、窗口代次隔离、迁移恢复支持边界和安装后桌面验收仍未关闭。浏览器模式冒烟通过。详见 [本轮 Alpha 发行验收](docs/alpha-release-acceptance-2026-09-08.md)；此前 CI 成功记录对应旧提交，不代表当前候选通过。

2026-09-08 深夜续（接任轮）：发布副本 `85fa056` 的 CI 已全绿（run `34246526554`）——短/长路径断言改为比较目录身份，同盘迁移改用 `canonical_with_suffix` 分类链接（runner 的 8.3 短名曾让受管链接被误判为越界）。窗口显示统一走 `reveal`（取消最小化、必要时移回主显示器），修复"进程活着但窗口停在屏幕外"的假死。本轮补上**跨盘撤销**：撤销此前只做 `rename`，跨卷迁移的撤销必然失败，现按前向迁移的 copy+staging 方式回搬并重写受管链接，并新增一条真实双卷测试（单卷机器会跳过并说明）。Alpha 版本标识已定：四处来源（package.json · Cargo.toml · tauri.conf.json · Cargo.lock）统一为 `0.1.0-alpha.1`，新增 `scripts/check-versions.mjs` 作为 gate 第一步，release workflow 复用同一脚本并按 `v<version>` 校验 tag；About 面板的版本改由 Vite 从 package.json 注入，不再硬编码。剩余发行阻塞：干净 Windows 环境的 NSIS 安装/首启/卸载、真实桌面交互的故障注入验收（含跨盘迁移撤销的真机路径）。

2026-09-08 Git 与健康复查：原有 WebUI / Alpha / 链接策略改动已保存为本地检查点 `e5f38c8`，`7176ad7` 衔接远端仅代码发布记录并保留本地文档。远端 `4317475` 的 CI 已成功（run `34176772394`）；它不包含本轮本地改动。最新审查、修复及证据见 [PROJECT_REVIEW.md](PROJECT_REVIEW.md)。当前优先处理：接管进程的持续退出监听、WebUI 窗口与进程代次绑定、迁移中断后的完整自动恢复，以及 Windows 安装包冷启动验收。迁移遇到最终目录已存在但日志仍为 `moving` 时，现先保留数据并报错，尚不能保证自动续传。

2026-09-06 增量：已实现 PHL 原生模型信息补全（models.dev、七天缓存、歧义匹配保护、仅填缺失字段、能力字段 YAML 双向同步），使用规则、上游 schema 依据和验证说明见 [模型信息补全](docs/model-metadata-enrichment.md)。

PHL 已经具备一个可运行的 DSH 桌面环境管理器的主体：版本与 Runtime 安装、实例隔离、插件管理、API 配置、启动停止、快照、Bundle、诊断和局部修复均有 Rust 实现。React、Zustand、Repository 与 Tauri/Rust 的总体分层可以保留。

当前最值得投入的是**交付可靠性、操作失败后的恢复能力，以及环境的精确复现**。旧路线图里一些“未来任务”已经完成，继续照原顺序开发会重复建设；设置、文档与实际行为之间也需要重新对齐。

建议长期围绕一个可验收的产品承诺发展：**用户能创建多个相互隔离的 DSH 环境，知道每个环境用了什么，在升级失败后恢复，并在另一台机器上重建它。**

阶段顺序：Windows 稳定交付 → 故障恢复与任务管理 → 精确版本与环境重建 → 模板和开发者工具 → 经需求验证后的跨平台与团队能力。

本文是本次代码检查形成的执行建议，不代表相关优化已经实现。时间窗口按单人持续开发估算，主要用于排序；涉及磁盘故障、系统凭据和平台适配的任务应在专项验证后重新估算。

## 2. 现状基线

| 能力 | 本地代码现状 | 下一步的重点 |
|---|---|---|
| 桌面与 UI | Tauri 2、React 18、TypeScript；已有页面懒加载、自绘标题栏、快捷键、主题与动效设置 | 验证真实桌面流程、整理无效设置、测量性能 |
| 实例管理 | 创建、克隆、保存、删除、目录隔离；清单已有 schemaVersion 和旧格式迁移保护 | 操作互斥、崩溃恢复、多版本升级兼容 |
| 版本安装 | 下载、完整性校验、依赖安装、安装健康检查、暂存与替换回滚 | 依赖精确记录、来源追踪、离线重装 |
| Runtime | 真实 Node 下载、校验与安装；目录按 `node-<major>` 标识 | 允许同一大版本下多个补丁版本共存 |
| 插件 | 注册表、安装、开关、卸载；已有 commit 解析、来源信任状态和安装摘要 | 完整安装事务、依赖兼容、按锁定来源重建 |
| API 配置 | 全局库、实例绑定、同步、差异检查、模型探测；Windows 系统凭据存储 | 配置与凭据提交一致性、环境变量与导出边界 |
| 进程 | 真实启动、端口分配、日志、退出事件与进程树停止 | PHL 重启后识别保留进程，明确托管语义 |
| 快照与 Bundle | 快照复制 `dsh-home`；Bundle 导出清单与插件记录 | 快照不包含 workspace/logs；Bundle 尚不能自动完整重建环境 |
| 诊断与修复 | 已有环境验证、目录骨架重建、过期事务残留清理 | 汇总修复计划、统一任务执行、修复后复验 |
| Source Build | 版本页已有引导入口，可生成交给 DSH agent 的构建任务；相关 helper 在未跟踪文件中 | 尚不是 PHL 内部受管的源码构建管线 |
| 交付 | 有 CI 和 Windows NSIS Release 工作流 | 修正 Rust 命令目录、验证干净构建、明确发布渠道 |

### 本次实际验证

| 检查 | 结果 |
|---|---|
| `npm test` | 7 个测试文件、33 项测试通过 |
| `npm run typecheck` | 通过 |
| `npm run build` | 通过；主 JS 477.34 kB，gzip 150.30 kB；其他页面另行分包 |
| `cargo test --manifest-path src-tauri/Cargo.toml --offline` | 112 项通过、4 项忽略，无失败 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --offline --all-targets --all-features -- -D warnings` | 通过 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | 未通过；已有未提交的 `src-tauri/src/plugins/cordis.rs` 在第 37、327 行附近有格式差异 |
| 在仓库根目录执行 `cargo metadata --offline --no-deps --format-version 1` | 失败：根目录及父目录没有 Cargo.toml，验证了工作流默认执行位置的问题 |

这些结果只证明本地当前工作树的检查情况。未执行真实 DSH 下载、用户实例启停、跨盘迁移、安装包安装或远端 CI；未启用 4 项被忽略测试，也未重新进行依赖漏洞审计。主 JS 体积不是总安装包体积，也不能据此推断首屏耗时。

检查开始时已有改动：`src-tauri/src/plugins/cordis.rs`、`src/data/versions.ts`，以及未跟踪的 `src/lib/githubBuildTask.ts`、`src/lib/githubBuildTask.test.ts`。本次规划不修改这些文件，不把它们视作已发布能力。

## 3. 优先优化清单

优先级定义：P0 为下一次对外发布前应关闭的问题；P1 为随后稳定性迭代；P2 为基于测量和用户反馈推进的增强。下面的代码判断与测试结果分别标注，不能把静态分析当作已完成的故障注入实验。

### P0：先建立可信的交付基线

- [ ] **O-01 修复 CI 与 Release 的 Rust 执行位置和检查顺序。** —— 实现已合入（b3fda5d：Rust 步骤限定 src-tauri、发布检查顺序与前端产物就绪）。2026-09-07 本机 `npm run app:build` 已成功生成 NSIS；干净检出 CI 与受控 tag 演练仍未执行，保持待验证。
  - 证据：`.github/workflows/ci.yml:62` 起直接执行 Cargo，未指定工作目录；`release.yml:51` 同样在根目录执行 `cargo test`。本地根目录命令已复现找不到清单。
  - 行动：仅为 Rust 步骤指定 `src-tauri` 工作目录，或统一传 `--manifest-path src-tauri/Cargo.toml`；Node 步骤继续在仓库根目录运行。Release 的 Rust 检查前先准备前端产物；在全新检出中确认图标等构建输入齐全。CI 中 tag 条件与实际触发器对齐；手动构建与正式发布分开定义。
  - 验收：一次干净检出的 CI、一次手动安装包构建成功；受控 tag 演练验证版本一致性、产物及哈希、预发布状态。不能仅以本机有缓存时构建成功作为验收。
  - 同步处理：将现有 rustfmt 差异归入原改动整理；校验 package.json、Cargo.toml、tauri.conf.json 与发布 tag 的版本一致。

- [x] **O-02 收紧 Bundle 的敏感字段导出，并修正文档承诺。** —— 实现已合入（40733f9：env_policy 显式凭据名 + 名称启发式，导入端报告待补凭据）。README/SECURITY 边界描述已在 O-15 复核修正。
  - 证据：`instances/manifest.rs:41` 的 `env` 保存字符串值；`instances/bundle.rs:103` 将完整 manifest 放入 Bundle。导入过滤器排除代码注入变量，但保留普通 API Key 名称的变量，见 `instances/mod.rs:686` 的现有测试。因此，用户若把密钥填入实例 env，导出会携带该值；本次没有读取用户配置，也没有确认发生过泄漏。
  - 行动：区分普通环境变量与凭据引用；Bundle 默认输出可分享字段及待补凭据名，导出预览明确省略项。显式敏感字段优先，名称启发式只能作补充；不能只依赖 `KEY` 字符串黑名单。旧实例提供可预览的迁移。
  - 验收：使用虚构密钥的测试实例，导出文件不包含该值；导入后准确提示需重新配置的凭据，普通变量保留。README/SECURITY 对 manifest、Bundle、快照与日志的说明符合实际边界。

- [x] **O-03 让设置界面只承诺真实可用的行为。** —— startup / 最小化到托盘 / 自动检查更新 / 日志级别已禁用并如实说明；隔离 node_modules 与开发者模式假开关移除；端口起点接入 suggestPort；「同时下载数」在 O-10 成为真实传输上限后重新启用。
  - 证据：`startup`、`minimizeToTray`、`checkUpdates`、`concurrency`、`logLevel`、`isolateNodeModules` 在当前 `src` 与 Rust 源码检索中只有定义、默认值和设置控件，未发现对应执行消费逻辑。`closeStopsInstances` 和版本目录定时刷新已有消费者，不应一并判为未实现。
  - 行动：逐项建“设置 → 执行入口 → 可观察效果”对照。未接入的先隐藏或禁用并准确说明；下载并发和日志等级在对应后端能力完成后开放。实例依赖隔离属于核心产品约束，不宜保留可随意关闭的假开关。
  - 验收：每个可用设置均有可验证效果；托盘操作实际出现托盘图标；并发数有实际执行上限；更新检查区分 PHL 应用更新与 DSH 版本目录刷新。

### P1：让失败、取消和重启都有明确结果

- [x] **O-04 将插件文件、安装记录与 Cordis 注册作为一个完整提交。** —— 822527d：备份与 cordis 快照保留到提交成功；任一提交步骤失败回滚旧包与旧启用状态；备份不可恢复时明示待恢复路径。故障注入测试覆盖记录写入失败、注册失败与备份丢失（plugins/install.rs tests）。2026-09-09 R2 修正：此前的“Drop/守卫在重启恢复路径覆盖”不成立——没有 Drop 恢复；旧备份在重试开头被无条件删除，硬中断后的第二次失败即可丢掉唯一旧包。现由 `recover_interrupted_swap` 在安装开始时按磁盘状态做跨进程恢复（放回未落位的旧包 / 移开未提交的新包并放回旧包 / 提交标记完好才退役备份），恢复矩阵与跨卷失败注入均有单测；真机硬退出验收仍见手测 §10。
  - 证据：`plugins/install.rs:158` 清理旧包备份之后，才在第 193、197、205 行写安装记录、注册 Cordis、更新启用状态。静态流程显示后续任一步失败时，已有备份已被清理，不能完成整体回滚。
  - 行动：保留旧包与配置备份，直到安装记录和配置提交、验证全部成功；明确取消点；失败恢复旧包及旧启用状态。
  - 验收：对记录写入失败、Cordis 写入失败、取消和进程中断分别注入故障；操作结果只能是旧插件完整可用，或新插件完整可用，并能识别待恢复状态。

- [x] **O-05 在 Rust 层建立资源操作互斥和任务登记。** —— f621479：resources.rs 冲突规则 + 原子获取 + RAII 释放；接入插件/实例/快照/Bundle/Runtime/版本/迁移/修复全部写路径；任务登记与 list_tasks；cancel_transfer 同步登记。并发与失败释放由单测覆盖。
  - 证据：`versions/mod.rs:168` 的 Transfers 按操作 ID 保存取消标记；它不是实例级资源锁。`instanceStore.ts` 的 `pluginInstallActive` 注释明确说明插件与快照冲突主要依赖前端拦截。
  - 行动：为实例、版本、Runtime 与根目录定义冲突规则；后端原子获取操作权，前端显示等待或冲突原因。任务 ID 与资源 ID 分开；记录阶段、取消请求和最终结果。
  - 验收：并发安装同一插件、安装时快照、迁移时创建、使用中替换 Runtime 等测试不会同时改写冲突资源；所有失败路径释放操作权。
  - 依赖：优先支撑 O-04、O-06，后续任务中心复用。

- [x] **O-06 将目录迁移升级为可恢复流程。** —— 1f433cb + dc43b1d：迁移日志持久化（源/目标/逐目录状态/字节/提交位）、重启后启动横幅与去处理入口、继续/撤销、提交前不切根指针、跨盘暂存+字节校验+原子放入。恢复矩阵的单测齐全；「完整迁移前后数量一致」等真机对照在手测 §10 登记。2026-09-09 R1 修正：原实现的“提交”不是单操作——成功路径先删 journal，再由前端另发请求切根指针，两次提交之间崩溃即出现“数据已搬完、journal 已没了、指针仍指旧根”。现在 `move_root_data` 在同一 DataRoot 守卫内以 `PhlState::relocate_root` 一次完成“切指针 + 清 journal”；指针提交失败则 committed journal 保留，重启后「完成切换」（`storage_migration_finish`）从 journal 自身记录收尾，恢复入口不再依赖 UI 重发路径。
  - 证据：`storage.rs:183` 起按子目录搬迁；取消时已搬走目录留在新根。`SettingsPage.tsx:236` 的取消分支不切根，当前主要靠 Toast 告知“重新迁移续传”。现有保护和取消测试已存在，但未覆盖完整的重启恢复体验。
  - 行动：持久化迁移记录，包括源、目标、已完成项、待完成项和提交状态；重启后优先进入恢复入口。在所有必要目录就绪前不切换根指针；跨盘采用暂存、验证与最终提交；取消必须可选择继续或按记录恢复。
  - 验收：每个阶段退出重启后都能恢复；完整迁移前后实例、配置、快照和日志数量一致；磁盘满、目标锁定、源删除失败有可继续处理的结果。恢复验证使用专用测试目录。

- [x] **O-07 修复 API 配置与系统凭据的提交顺序。** —— 4b263af：保存固定一次根解析，准备（仅新增凭据）→ 提交（tmp+rename）→ 提交成功后才清理被删供应商凭据；凭据命名空间为按 provider id 的全机共享，已写入文档与注释。假凭据存储的顺序测试覆盖提交失败/成功清理/存储失败三分支。
  - 证据：`api_config/library.rs:94` 先写凭据，第 102 行删除已移除供应商的凭据，之后才写临时配置并 rename；配置写盘失败时可能留下旧配置却已更改凭据。`credentials.rs:20` 仅以 provider ID 命名，多个数据根使用相同 ID 时是否共享凭据需要明确。
  - 行动：保存开始时固定根目录；设计可恢复的“准备凭据 → 提交配置 → 清理旧引用”流程。明确凭据是全局共享还是配置库独立；若独立，采用稳定配置库 ID，避免目录迁移导致引用失效。
  - 验收：配置提交失败不丢失旧配置所需凭据；切换两个根目录不意外覆盖；旧明文迁移部分失败后可重试且不丢值。测试使用临时命名空间和虚构密钥。

- [x] **O-08 完成保留进程与 PHL 重启后的管理闭环。** —— 47c83c6：进程身份登记（pid/port/spawn 时间/内核 exe 路径）；adopt_processes 重启对账，可信才接管（web URL 自最近日志恢复），身份不可确认只报告绝不终止；纯函数决策表有单测。真机双场景入手测 §10。2026-09-09 R3/R4 修正：① 取消/超时启动的清理原会忽略 `kill_tree` 失败仍删登记，进程活着却脱离管理、重启无法按原记录接管、错误还谎称“已终止”；现 `terminate_and_forget` 以“内核确认退出”为遗忘前提，未确认时保留全部登记与端口占用，把 `[state] kept-alive` 交回前端映射为可停止的运行态，`stop_instance` 的 `terminate_or_gone` 容忍“kill 报错但进程已消失”。② `Registry` 原用两把独立锁，A 旧快照可能在 B 新快照写完后覆盖磁盘、活跃登记被抹且临时文件名固定会撞车；现改为单锁「变更+快照+落盘」串行提交，临时名按提交序号区分，落盘/替换失败带受影响实例清单记日志。并发乱序与残留有确定性单测。
  - 证据：`App.tsx` 允许关闭时保留实例；`launch/mod.rs:80` 附近明确说明 Processes 只在内存中保存，PHL 重启后原子进程不受管理。
  - 行动：先定义停止退出、保留运行、最小化托盘三种行为。记录进程身份与实例关系；重启后验证 PID、创建时间、程序路径和实例标识，再决定接管或显示外部运行状态。识别不可靠时不自动终止。
  - 验收：保留运行后重启，正确展示并打开已有 WebUI；不会重复启动，也不会因 PID 复用误停其他程序；启动中退出和 PHL 异常退出有明确行为。

- [x] **O-09 建立桌面关键链路回归，并统一错误信息。** —— 94516b5：稳定错误码（busy/port-conflict/net-retryable/net-fatal/permission/disk-full/not-found/state）+ 前端解析与下一步提示；任务生命周期日志携带任务 id；诊断报告可复制导出（不含密钥/环境值）；手测清单 §10 覆盖冲突、迁移中断恢复、接管、凭据回滚、设置真实性与结果登记表（真机执行记录待填）。
  - 证据：现有单元测试和 `scripts/browser-smoke.py` 已有价值，但浏览器测试走 Mock；手测清单没有本轮桌面执行记录。
  - 行动：先补最小 Windows 桌面回归：安装 → 创建 → API 绑定 → 启动 → 插件变更 → 快照恢复 → 停止 → 重启确认。将常见后端错误整理为稳定错误码、阶段、可重试性和可执行操作；日志关联同一任务 ID。
  - 验收：在干净用户环境/专用数据根留下版本、系统、步骤与结果；网络中断、权限不足、磁盘不足、端口冲突可以归因；诊断导出提供脱敏预览。

### P2：按数据优化体验和维护成本

- [x] **O-10 统一长任务展示，落实并发设置。** —— f029a30：标题栏任务中心读取后端任务登记（阶段/取消请求/结果/错误提示/失败重试入口）；版本、Runtime、插件安装共享由 concurrency 强制的传输槽位队列，超额显示排队中；轮询隐藏时暂停、运行中加速。 使用 O-05 的任务模型呈现排队、运行、取消中、失败、完成；保留简短失败历史和重试入口。验收：跨页面可追踪；取消反馈及时；实际并发不超过设置值。
- [x] **O-11 改善下载恢复与取消。** —— e60a798：等待与响应头均可被取消打断（1 s 轮询窗口）；错误分类（网络类退避重试 ≤3、4xx/磁盘即时失败）；断点续传绑定 URL+ETag/Last-Modified，206 确认同一实体才追加，任何不匹配从零重启，收尾对整文件重算摘要。本地模拟服务器测试覆盖续传/异体拒绝/停滞取消/致命 404。 `versions/download.rs` 当前从头创建缓存文件；等待网络响应或下一个流分块时，取消依赖后续检查。加入可中断等待、分类重试；断点续传须绑定来源和 ETag/Last-Modified 并重新校验。验收：本地模拟慢响应、中断和内容变化，取消能结束等待，续传不会拼接不同内容。
- [x] **O-12 控制日志与磁盘扫描缓存。** —— 95d2c80：日志尾读 256 KiB 有界（截断行丢弃）、web URL 头部有界读取、每实例仅保留最近 10 个启动日志；instance_disk_usage 15 s memo + 变更点失效（创建/删除/克隆/插件装卸/快照恢复）。扫描可取消项未做（单命令粒度已足够小），记为残留限制。 `launch/process.rs:11` 与第 191 行附近读取完整日志；改为有界读取，设置滚动和保留期限；目录占用缓存并允许显式刷新。验收：大日志尾部读取内存受上限约束；扫描可取消，缓存有更新时间和失效规则。
- [ ] **O-13 按职责拆分大页面与桌面桥接。** —— 持续完成中：`desktop.ts` 已降至兼容导出壳（282 行、17 个领域 bridge 文件）；插件页已拆出 registry/installed/detail/update-state；2026-09-07 又将 `catalogStore.ts` 从 700+ 行拆为 91 行组合壳 + version/runtime/plugin actions + 通知策略，并将 `api_config/mod.rs` 从 1200+ 行拆为 147 行编排壳 + types/validation/launch_keys/tests。SettingsPage（约 931 行）和 InstanceDetailPage（约 878 行）仍需按完整交互继续拆分，因此不勾完成。验收：每次只迁移一个完整交互，已有行为与错误路径保持；不以文件行数作为质量指标。
- [ ] **O-14 建立性能和可访问性基线。** —— 部分完成：PERF_BASELINE.md 已更新构建/体积/测试，并加入 `.phlpack` 64 MiB + 1000 文件的前后基准（build/validate/unpack 峰值工作集下降 95%+）及浏览器 Mock 交互预检。桌面冷启动、50/200 实例、500 插件、Tauri Profiler、125%/150% 缩放与常驻内存仍待真机采集。
- [x] **O-15 收敛项目文档。** —— AGENTS.md 成为约定权威（CLAUDE.md 改为兼容指针）；README 的「环境修复为未来能力」与「LICENSE 占位（文件不存在）」两处失真已修正，许可证维持「发布前由维护者决定」的如实表述；SECURITY 修正 commit 固定已实现的描述、补充应用状态文件边界；各历史文档的分工写入 AGENTS 文档节。 README 已将局部修复列为未来能力，SECURITY 仍称 commit 固定未实现且凭据只有 env 引用；这些均落后于代码。保留旧计划作历史，建立当前状态入口；将适用项目约定维护到 canonical AGENTS.md，CLAUDE.md 作为兼容入口。核实 README 提到但本次文件清单未见的 LICENSE 占位，发布前由维护者决定授权方式。验收：实现状态、代码位置、已知限制和验证记录可相互追溯。

## 4. 长期发展清单

以下是产品建设任务，状态均为待办。带条件的任务在前置条件满足后再排期，不将所有方向同时推进。

| ID | 发展项 | 用户获得的结果 / 完成标准 | 前置条件与时机 |
|---|---|---|---|
| L-01 | Runtime 精确版本共存 | 同一 Node 大版本下可保留两个不同补丁版本；实例固定具体版本；旧 `node-<major>` 引用可迁移和回退 | O-05；环境重建前先做 |
| L-02 | 环境锁定清单 | 记录 DSH、Runtime、插件、依赖解析结果、来源与摘要；显式记录因上游包缺失而采用的降级，能识别漂移 | L-01；复用已有安装 marker，避免重复事实来源 |
| L-03 | Bundle 一键重建 | 导入先显示计划、下载量和缺失凭据，再安装精确资源与插件；缺源明确失败；完成后 verify + 启动检查 | O-02、O-04、O-05、L-02 |
| L-04 | 兼容性检查与升级预演 | 展示 DSH × Node × 插件的兼容证据；区分已验证、不兼容、未知；在克隆实例试升级后再应用原实例 | O-09、L-02；优先消费 engines 与已知元数据 |
| L-05 | 可回滚升级 | 更新前固定旧环境和快照；失败恢复版本引用、插件与配置，并验证可启动 | O-04、O-05、L-01、L-02 |
| L-06 | 模板与启动预设 | 少量版本化模板描述环境、必填配置和插件；新用户从模板创建后能跑通明确场景 | L-03；模板变化可追踪，不内置密钥 |
| L-07 | PHL 自身更新 | 稳定/预览渠道、可信更新校验、更新前状态检查、配置兼容与恢复方案；损坏或校验失败产物不执行 | O-01、O-09、O-15；与 DSH 更新分开 |
| L-08 | 受管 Source Build | 固定 tag/commit，在专用目录构建，发现工具版本，保留构建日志；验证后原子登记产物，失败不影响旧版本 | O-05、L-02；现有 agent 任务只是过渡入口 |
| L-09 | 插件开发环境 | 本地源码关联、开发/稳定实例分离、重载反馈、插件加载日志；开发改动不污染稳定实例 | L-04、O-09；先验证真实插件开发需求 |
| L-10 | 离线资源包与缓存管理 | 在许可允许的资源范围内导出可验证的离线包；无网络的干净测试环境可重建；删除缓存不破坏正在使用的资源 | L-02、L-03、O-05；容量收益测量后排期 |
| L-11 | 快照配额与用户数据备份 | 用户明确区分环境快照和 workspace 备份；按数量/容量保留；备份必须演练恢复 | O-06、O-12；优先完整性，之后再评估去重 |
| L-12 | macOS / Linux | 平台凭据、进程树、路径、权限、打包与安装均有本机验证；文档明确支持矩阵 | Windows 稳定版门槛通过；每个平台单独验收 |
| L-13 | 本地 CLI / 批量操作 | CLI 与桌面复用同一核心，实现创建、检查、启动、导出；批量操作逐实例报告且可取消 | O-05、L-02；存在重复操作需求后启动 |
| L-14 | 团队模板与环境交接 | 分享模板和环境锁定信息，凭据留在各自设备；成员可预览变化并在本地重建 | L-03、L-06；有真实多人使用案例后启动 |
| L-15 | 远程实例管理 | 明确远程主机、认证、连通性、日志和操作权限；本地与远程故障可区分 | L-13、L-14；先做需求验证和独立设计 |

Source Build 的近期补充：现有生成任务含固定构建流程和工具版本假设，目标路径直接指向共享 versions 目录。正式产品化前应改为逐 tag 读取构建契约、固定 commit、暂存构建、执行 CLI 检查再登记；不能把生成提示词成功等同于构建成功。

环境锁定也不能只记录插件显示版本。当前 Bundle 插件记录只有 pluginId、version、registryId，而安装 marker 已有 source、ref、integrity、actualIntegrity、trust；应从已有真实安装事实构建下一版格式。DSH 依赖安装目前使用 `--no-package-lock`，需要另行保留实际依赖解析结果，才能支持精确重建。

## 5. 建议排期与阶段退出条件

| 阶段 | 参考窗口 | 重点范围 | 进入下一阶段的条件 |
|---|---|---|---|
| M0：发布基线 | 第 1–2 周 | O-01、O-02、O-03 的最小闭环；O-15 文档纠偏；O-09 桌面冒烟 | P0 全关闭；干净环境能构建安装；基本链路通过；公开说明与行为一致 |
| M1：稳定性 | 随后 3–6 周 | O-04 至 O-09，先操作权与事务，再迁移/进程/凭据恢复 | 关键故障注入通过；重启后可解释并处理未完成操作；不丢旧环境 |
| M2：环境复现 | 随后 2–3 个月 | L-01、L-02、L-03，再做 L-04/L-05 的最小闭环；按测量穿插 O-10 至 O-14 | 第二台干净 Windows 机器能重建同一环境；升级失败可恢复并启动 |
| M3：产品扩展 | 约 3–6 个月，按反馈调整 | 少量模板、应用更新；在 Source Build 与插件开发中择优推进 | 新用户可独立完成首个实例；扩展能力有真实使用记录与维护人 |
| M4：平台与协作 | 6 个月以后，按需求启动 | L-10 至 L-15 中证据最充分的方向 | 每个方向有目标用户、验收环境、维护投入与退出标准 |

O-04 至 O-08 属于不同故障域，不宜混进一个“大重构”提交。先共享最小操作互斥设施，再按插件、迁移、凭据、进程逐个交付；每项保留独立验证记录。

2026-09-06 执行轮的结果：三张任务卡之外，O-02～O-12、O-15 已实现合入并通过自动化验证（Rust 158 通过 / 4 忽略、前端 37 通过、typecheck/build/clippy/fmt 全绿）；O-01 待干净 CI 与 tag 演练，O-13、O-14 为持续迭代项。下一轮重点：执行 CI 发布演练、手测清单 §10 真机登记、继续 O-13 拆分与 O-14 真机基线采集。

2026-09-06 「本机 DSH 接入」P0 轮：按《下一阶段开发规格》§28 完成 P0-3 Session Spike（`docs/research/dsh-session-integration.md`）、P0-1 Discovery（`src-tauri/src/discovery/`）、P0-2 Adoption（`instances/adoption.rs` + manifest schema v2）与前端接入向导（`AdoptDshPage.tsx` / `adoptionStore.ts` / `desktopAdoption.ts`），实现与偏差记录见 `docs/p0-local-dsh-adoption.md`。自动化验证：Rust 192 通过 / 5 忽略、前端 51 通过、typecheck/build/clippy/fmt 全绿。真机手测矩阵（规格 §32 Discovery/Adoption 段）与 P1（Session 迁移引擎、`.phlpack`）尚未开始。

2026-09-06 「Session 迁移 + PHL Pack」P1 轮：按《下一阶段开发规格》§29 完成 P1-1 Session 迁移引擎（`src-tauri/src/sessions/`：逐帧 zstd codec + re-id 复制，保留 lineage/cwd，运行/外部写门禁、多目标 fan-out）、P1-2 `.phlpack` 格式与校验器（`src-tauri/src/pack/{format,mod}.rs`：ZIP 容器、v1 manifest、防穿越/软链/大小/重复/版本、SHA-256 完整性）、P1-3 导出（`pack/export.rs`：受管实例扫描、本地/远程插件分类与许可提示、凭据剥离、会话隐私二次确认）、P1-4 安装器（`pack/install.rs`：preview+resolver、staging 原子落位+回滚、embedded 插件/会话/overrides 解包、远程插件延后交回安装管线）。前端 `desktopSessions.ts` / `desktopPack.ts` 桥接 + 实例详情「对话迁移」内联面板 + 安装/导出整合包向导 + 实例页「安装整合包」入口。DSH schema 与 models.dev 数据形状均已对真实安装核实。新增 `ruzstd`/`zstd`（离线 crate，Node 互解已验证）。详见 `docs/p1-session-pack.md`。

2026-09-06 「P0/P1 收尾 + P2」轮：关闭上一轮登记的全部遗留项并完成 P2。①接入向导「按对话选择迁移」闭环（规格 §3.2/§3.3）：`SessionStrategy::Selected` 从「下一版本」拒绝路径改为真实实现——复制排除会话库、`list_adoption_sessions` 列源对话、选定目录**原 id 原样**迁入（接入是迁移非分发，与 §5.1 的重 id 复制语义分开），向导补可选单选+对话勾选列表；②复制会话后源/目标计数即时刷新；③实例页「导入 Bundle」按规格 §26 移入「更多导入方式」下拉；④Discovery 平台路径按规格 §2.1 拆出 `discovery/{windows,macos,linux}.rs`（macOS/Linux 补 GUI 启动 PATH 不可见的 Homebrew/npm-global/nvm/Volta/Bun/~/.local bin 根；非宿主平台文件以 `#[cfg(test)] #[path=…]` 编入本机测试构建保持编译与单测覆盖），macOS/Linux **真机**验证仍属 L-12；⑤P2-1：`.phlpack` 格式抽出 `src-tauri/crates/phl-pack-core`（workspace 根设在 src-tauri 以免挪动打包路径；schema/校验/integrity/`PackBuilder`/`build_pack_from_dir`/dest-root 受限解包），新增 `crates/phl-pack-cli`（`phl-pack inspect/validate/unpack/build`），桌面端 export/install 变薄壳复用同一 core；⑥P2-2：`docs/skills/phl-export/SKILL.md`（侦察→分类→敏感询问→组布局→CLI，铁律「不发明格式」）；⑦规格 §32/§33 手测矩阵登记进 `dsh-phl-manual-test-checklist.md` §11–§14。验证：Rust（workspace）255 通过 / 5 忽略（host 229 + core 23 + cli 3）、前端 59 通过、typecheck/build/clippy/fmt 全绿。**尚未完成**：§11–§14 真机手测执行、L-12 macOS/Linux 真机验证、规格 §31 明确排除的市场/云同步/文件关联等。详见 `docs/p2-pack-core-skill.md`。

2026-09-07 review 收口轮：完成 M3 catalog store action/通知策略拆分、M5 API config types/validation/launch_keys/tests 拆分；R7 将唯一 transfer id 和协作式取消贯穿 Pack 扫描、压缩、完整性哈希与解包的 64 KiB 循环，导出/安装 UI 增加取消入口，取消解包会删除当前半成品；R6 新增可复现前后基准。另加入 bridge 静态门禁（17 文件 / 71 个 invoke 全部有 Rust handler）并修复浏览器回归发现的嵌套按钮与 Toast ref 警告。验证：Rust workspace 268 通过 / 5 忽略；前端 73 通过；typecheck/build/clippy/fmt/bridge check 全绿；NSIS 构建成功。仍需真机项见手测清单 §15。

## 6. 如何判断优化有效

指标先在开发/验收环境采集；不把在线遥测作为默认前提。性能数值是建议目标，需在首轮测量后确认，当前没有实测达标结论。

| 指标 | 测量方式 | 建议验收方向 |
|---|---|---|
| 干净安装成功 | 固定 Windows 测试环境，从安装包创建并启动实例 | 发布候选的全部关键用例通过，记录失败原因 |
| 环境可恢复 | 关键写入点、磁盘满、文件锁、取消、重启注入 | 旧环境可用或进入明确恢复状态，不静默丢数据 |
| 精确重建 | 两台干净环境对比锁定清单与启动结果 | 必需资源身份与摘要一致；差异可说明 |
| 长任务反馈 | 从用户点击到状态反馈；从取消到任务结束 | 即时显示已接收；普通本地操作反馈建议不超过 200 ms；网络取消建议 2 秒内结束等待，提交阶段说明不可立即取消的原因 |
| 启动与交互 | 固定设备、多次测量冷启动和典型交互 | 先记录 p50/p95；同条件回归超过 10% 时分析原因；PHL 启动与 DSH 就绪分开统计 |
| 大数据规模 | 50/200 实例、500 插件、大日志、大 node_modules | UI 可操作，后台扫描可取消，内存和日志有可解释上限 |
| 维护成本 | 新能力变更涉及层次、重复实现、用户故障定位时间 | 共用管线减少重复；错误能关联到阶段与日志 |

## 7. 暂不投入与维护方式

当前保留 Tauri/Rust/React 架构。没有测量证据时，不为减小某个源文件、追求框架更新或更换状态库安排整项目重写；也不先做云账号、远程集群、复杂权限体系或大型公开模板市场。硬链接/共享 node_modules 去重需先证明不会破坏实例隔离，并量化磁盘收益。

每个迭代结束更新本文任务状态：待办 → 进行中 → 待验证 → 完成。只有实现合入且对应验收通过才勾选；同时记录提交、验证环境、结果与残留限制。新发现的问题优先按用户影响排序，不按功能数量排序。

与现有材料的关系：`dsh-phl-project-master-plan.md`、`dsh-phl-development-roadmap.md` 保留产品历史与愿景；`PROJECT_REVIEW.md` 是 2026-09-05 的审查快照，测试数与依赖审计结论不能当作今天的结果；`dsh-phl-manual-test-checklist.md` 可继续承载实际桌面验收，并补充 API 配置、修复、迁移恢复、进程接管和精确重建。

## 8. 主要代码依据索引

以下路径相对于仓库根目录；行号对应本次检查工作树，后续编辑后可能变化。

| 关注点 | 入口 |
|---|---|
| CI / 发布目录与触发 | `.github/workflows/ci.yml:62`、`.github/workflows/release.yml:48` |
| 数据源与领域接口 | `src/services/index.ts`、`src/services/repository.ts`、`src/lib/desktop.ts` |
| 设置接入情况 | `src/stores/settingsStore.ts:36`、`src/pages/SettingsPage.tsx:365`、`src/App.tsx` |
| Bundle 敏感值与锁定信息 | `src-tauri/src/instances/manifest.rs:41`、`src-tauri/src/instances/bundle.rs:37`、`:103` |
| 导入环境变量过滤 | `src-tauri/src/instances/snapshot.rs:43`、`src-tauri/src/instances/mod.rs:686` |
| 插件事务提交边界 | `src-tauri/src/plugins/install.rs:158`、`:193`、`:197` |
| 来源固定与降级 | `src-tauri/src/plugins/resolve.rs:116`、`src-tauri/src/plugins/security.rs` |
| 后端取消与前端互斥 | `src-tauri/src/versions/mod.rs:168`、`src/stores/instanceStore.ts` 的 `pluginInstallActive` |
| 目录迁移 | `src-tauri/src/storage.rs:183`、`src/pages/SettingsPage.tsx:236` |
| 凭据提交和命名 | `src-tauri/src/api_config/library.rs:94`、`:102`、`:120`、`src-tauri/src/credentials.rs:20` |
| 进程保留与日志读取 | `src-tauri/src/launch/mod.rs:80`、`src-tauri/src/launch/process.rs:11`、`:191` |
| Runtime 标识 | `src-tauri/src/runtimes/catalog.rs` 的 `group_catalog`、`src-tauri/src/runtimes/install.rs:39` |
| 下载与依赖物化 | `src-tauri/src/versions/download.rs`、`src-tauri/src/versions/dependencies.rs:408` |
| 诊断与已实现的局部修复 | `src-tauri/src/verify.rs`、`src-tauri/src/repair.rs:1` |
| Source Build 过渡能力 | `src/pages/VersionsPage.tsx:154`、`src/lib/githubBuildTask.ts` |
