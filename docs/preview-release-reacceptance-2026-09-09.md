# R1–R4 修复复验 · 2026-09-09

## 判定

**修复验收未通过，公开预览版仍为 NO-GO。** R4 的并发旧快照覆盖可关闭；R1 的原提交顺序问题已修复，但日志残留路径仍有缺口；R2、R3 不能关闭。本轮发现 2 项 P1、2 项 P2，其中插件旧备份删除和迁移假成功已用隔离回归实际复现。

审查对象是本地 `6caee5b` 之上的未提交修复，不能沿用原提交的 CI 或安装包作为此次修复的发行证据。产品 diff（`git diff --binary -- src src-tauri`）SHA-256：`410b5e943a4e941ace8185a7172168f5b5198f89d20ee6ca7dd3ef6228a24349`。

## 已执行验证

| 项目 | 结果 |
|---|---|
| `npm run test:alpha` | 原有门禁全部通过 |
| 前端测试 | 14 文件、85 项通过 |
| Rust workspace | 331 项通过、6 项忽略（桌面库 295、CLI 3、Pack Core 33） |
| Node desktop runner 测试 | 1 项通过，仍只是 runner 隔离与清理测试 |
| typecheck / bridge / fmt / clippy | 通过；17 bridge 文件、73 invoke、76 注册，无缺失/重复 |
| `npm run build` | 通过；主 JS 510.22 kB / gzip 160.83 kB，有既有分包警告 |
| 补充隔离回归 | 2 项均失败，分别复现下文 V1 和 V3 |
| `git diff --check` | 通过；LF/CRLF 提示不属于空白检查失败 |
| 远端 CI | 最新仍为旧 `ea60b5d` 的成功 run `34249375678`，不包含本轮未提交修复 |
| NSIS / 安装后验收 | 本轮未重打包或安装；现有 alpha 安装包仍是 08:38 的上一轮产物，不能证明新代码已验收 |

补充回归调用真实 Rust 私有函数，以临时目录构造中断后的磁盘状态，无网络下载、真实用户数据、真实凭据或 DSH 实例操作。两条临时测试通过脚本加入原有 test module 后执行 `cargo test --manifest-path src-tauri/Cargo.toml --offline --lib reaccept_ -- --nocapture`，运行完成后产品文件按测试前原始字节恢复并校验。未将故意失败的诊断测试留在产品测试集中，未修改修复实现。

## 未关闭问题

### V1 · [P1 / R2] 安装标记尚不是完整提交，恢复仍会删除唯一旧包

位置：`src-tauri/src/plugins/install.rs:497–502`；前置顺序在 `:300–316`。

`commit_install` 的第一步写入 `phl-plugin.json`，随后才安装依赖、注册 Cordis、更新启用状态。但 `recover_interrupted_swap` 只要该文件是可解析 JSON 就删除 backup。硬退出发生在标记写完、依赖安装尚未完成时，重试仍把不完整的新包当成已提交，删除旧包；本次重试再失败/取消，旧版本便无法恢复。

**实测**：构造 backup 中含 `old.js`、dest 中含合法 marker 和声明必需依赖的 package.json、但没有安装依赖的状态；调用 `recover_interrupted_swap` 后，backup 与 dest 都不再含旧包。测试 `reaccept_marker_before_dependencies_must_keep_old_backup` 失败：

```text
an incomplete dependency install must not destroy the only old package
```

现有 `committed_landing_retires_the_consumed_backup` 测试仅写 `{"version":"2.0.0"}` 就期望删备份，验证了当前实现，却没有验证真实的完整提交条件。

修复/验收要求：使用只有所有安装步骤成功后才持久化的事务提交状态；Cordis 原始状态也需要可跨进程恢复，不能只存在内存 `patch_before` 中。覆盖标记写入、依赖安装、Cordis 注册与启用更新之间的中断，并验证失败/取消后旧包和旧注册状态都可恢复。仅把 marker 移到最后、却不处理 Cordis 已修改时的恢复，仍不完整。

### V2 · [P1 / R3] 进程查询失败被当作已经退出

位置：`src-tauri/src/launch/mod.rs:240–247`、`:306–314`；Windows 探测依据在 `src-tauri/src/launch/registry.rs:334–342`。

新增退出确认使用 `!probe_process(pid).alive`。但 Windows `OpenProcess` 失败或 `GetExitCodeProcess` 查询失败都返回默认 `alive=false`。当终止失败且存活进程无法查询时，200ms 后依然会清除内存和持久登记，并返回停止/取消成功。原 R3 的“无法确认退出仍遗忘登记”因此没有完全消除。

这是静态代码确认，未对用户进程实施权限故障注入。应区分“存活 / 已退出 / 未知”，未知不得清登记；本次启动仍持有 child handle，可优先通过该 handle 确认退出。需要针对 kill 失败且 probe 返回未知的分支注入测试，不能只测试普通进程可成功终止的情况。

### V3 · [P2 / R1] 已提交日志残留导致下一次迁移假成功

位置：`src-tauri/src/storage.rs:599–606`，以及 `src/pages/SettingsPage.tsx:331`。

新 `relocate_root` 明确允许“根指针已写成功，但 journal 删除失败”的结果。此时遗留 A→B 的 committed journal；用户随后发起 B→C，`move_root_inner` 遇到任意 committed journal 就返回旧迁移的成功摘要，没有核对这次的 from/to，也没有搬运数据。后端采用旧 journal 的 B，前端随后又调用 `switchRootTo(C)`，它实际上仍执行 `setRootVerified` 写根指针，并非注释所说的仅同步界面。这会把界面及后端指向没有迁入数据的 C，并提示成功；数据本身仍在 B。

**实测**：在临时目录执行 A→B 的 inner，保留它生成的 committed journal，再调用同一 inner 执行 B→C。返回成功，但 `C/config/sentinel` 不存在。测试 `reaccept_stale_committed_journal_must_not_fake_new_migration` 失败：

```text
B -> C must move the data or reject the stale A -> B journal, never report success without moving
```

本实验验证后端假成功；前端进一步写错根的链路由代码确认，未用桌面交互复现。应拒绝不匹配的 journal，或先安全清理已经完成且根指针一致的旧记录；迁移返回权威目标根，前端只同步该结果，不能再按请求参数另写根。

原 R1 的“先删日志、后提交指针”确已修复，成功与指针提交失败的测试也通过；本项是仍需补齐的残留恢复分支，不能继续照旧描述成原顺序完全没改。

### V4 · [P2 / R3] kept-alive 进程没有持续退出监听

位置：`src-tauri/src/launch/mod.rs:725`、`:762–776`、`:812`。

取消/超时的保留进程分支直接返回，唯一的 `child.wait()` watcher 仍只在启动就绪成功后创建。如果 taskkill 随后才完成，或 DSH 自行退出，前端会持续显示运行中，内存/磁盘登记保留；直到用户再点停止或重启 PHL 才清理。

静态确认，未进行真实 taskkill 延迟完成实验。返回 kept-alive 之前应将 child 交给持续 watcher，并处理退出事件早于错误结果到达前端的竞态。前端现有测试只 mock `KeptRunningError`，没有验证这条真实事件链。

## 可以关闭的内容与保留边界

- **R4 并发覆盖：通过。** 同一 state mutex 已覆盖修改、快照和落盘，独立临时文件名处理了碰撞；原先旧快照晚写覆盖的时序不再成立。
- **R1 原始提交顺序：通过。** 根指针成功持久化之后才清日志；指针失败保留 journal；新增完成切换入口接入了 handler、bridge 和界面。完整迁移验收仍受 V3 影响。
- **R3 前端保留停止入口：通过静态链路与现有单测。** coded error 优先于 signal.aborted，映射到 `KeptRunningError`，store 提供运行中状态及停止重试按钮；后端未知状态和持续监听仍受 V2/V4 影响。
- R4 落盘失败现在输出日志，但调用仍成功，用户界面未显示接管持久化风险；新增名为 `a_failed_rename_leaves_no_stale_tmp_behind` 的测试未真正注入 rename 失败，不能作为该失败分支验收证据。
- 上轮 API 旧密钥覆盖、快照版本绑定、Windows DSH_HOME 大小写、旧 WebUI URL/token 等相关产品文件/逻辑未修复；本轮 R1–R4 修改不代表这些 P2 已关闭。
- `PHL_ROOT` 仍关闭进程和迁移日志的持久化；普通隔离 runner 不能代替持久测试环境。干净 Windows 安装、真实插件加载、真实进程退出/重启接管与迁移故障注入仍未验收。

## 下一轮最小验收要求

先关闭 V1/V2，再补 V3/V4；新增测试应覆盖本文的失败时序而非仅复述当前成功分支。修复完成后跑全量 gate 和受影响的隔离回归，固定候选提交，再重建安装包并开展真实桌面验收。当前代码已经能构建，不应在已确认恢复缺陷尚存时用旧 CI 或旧安装包放行。
