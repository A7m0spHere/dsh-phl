# V1–V4 修复复验 · 2026-09-09

## 判定

**V1、V2、V3、V4 四项均已关闭：实现已落地，并且都有覆盖失败时序的回归测试。**

公开预览**仍不能放行**，但剩余条件不再是代码缺陷，而是真机验收：干净 Windows 环境的安装 / 首启 / 卸载、
真实断电与硬退出注入、以及安装后的完整业务链路。本轮没有做这些注入，因此不放行。

复验对象是 [上一轮复验报告](preview-release-reacceptance-2026-09-09.md) 点名的四个位置。
该报告描述的是它当时的代码状态；其点名的行号在本次复验时已指向别的实现，逐项对照见下。

## 证据口径

- 本地 HEAD：`943434c`（含图标与 README 提交；四项修复本体在 `f06c157`）。
- `npm run test:alpha` 全绿：版本、typecheck、bridge、前端测试、runner、fmt、clippy、Rust workspace。
- Rust workspace：**346 项通过 / 0 失败 / 6 忽略**（桌面库 310、Pack CLI 3、Pack Core 33）。
- 前端：14 个文件、**88 项通过**。
- Bridge：无缺失 handler、无重复注册。
- 所有断言都在产品测试集中，不依赖临时脚本；临时注入测试未留在仓库。

## 逐项关闭证据

### V1 · 插件安装中断不得删除唯一旧包 —— 已关闭

上一轮的问题是：`commit_install` 第一步就写 `phl-plugin.json`，而恢复只要看到可解析的 marker 就退役备份；
硬退出发生在 marker 之后、依赖与 Cordis 之前时，唯一旧包会被删掉。

现在的实现把「提交」做成真正的事务：安装前先在 `<profile>/.phl-plugin-txn/journal.json` 落盘
`Prepared`（含 `had_previous` 与 `registration_before`），包交换、依赖安装、Cordis 注册、启用状态全部成功后才持久化
`Committed`；恢复按 phase 判定——`Prepared` 一律放回旧包，`Committed` 才退役备份，且备份在清理阶段最后才删。
旧格式残留（没有事务目录）走「备份优先于落地副本」，早写的 marker 不再能证明提交。

覆盖测试（`plugins::install::tests`）：
`transaction_every_precommit_cut_restores_old_package_and_disabled_state`、
`transaction_only_final_commit_retires_old_backup`、
`transaction_recovery_crash_after_old_restore_is_idempotent`、
`transaction_committed_cleanup_interruption_never_rolls_back_new_package`、
`transaction_patch_restore_failure_retains_journal_and_restored_old_package`、
`transaction_failed_fresh_install_removes_only_its_registration`、
**`legacy_marker_does_not_prove_commit_or_retire_backup`**（即报告要求的
「marker 已写、依赖未装」场景：断言恢复后 `dest/old.js` 存在、`new.js` 不存在）、
`a_torn_marker_does_not_count_as_committed`。

### V2 · 进程查询失败不得当作已退出 —— 已关闭

探测改为三值 `ProcessState::{Alive, Unknown, Exited}`：`OpenProcess` / `GetExitCodeProcess` 失败返回 `Unknown`，
只有观测到 `Exited` 才清除内存与持久登记；`Unknown` 保留登记并以 `[state] kept-alive <pid> <port>` 编码上抛。

覆盖测试：**`termination_faults_keep_rows_until_exit_is_observed`** 对 `kill_ok × {Alive, Unknown, Exited}` 六种组合逐一断言
（失败或未知时登记必须保留，只有 Exited 才清），`termination_is_forgotten_only_once_exit_is_confirmed`、
`a_stop_of_a_process_that_died_after_the_keep_clears_the_rows`。

### V3 · 残留 committed journal 不得让下一次迁移假成功 —— 已关闭

`move_root_inner` 现在核对 journal 的 `from`/`to`：目的地不匹配的 committed 记录直接拒绝，不再把旧的 A→B 摘要当成 B→C 的成功；
`finish_committed_migration` 只接受 `committed` 记录，且由后端自己完成指针提交。

覆盖测试：**`stale_committed_journal_cannot_report_a_different_move_as_success`**（报告要求的最小复现：A→B 后保留 committed journal 再跑 B→C，
断言返回错误、B 的 sentinel 仍在、C 为空、journal 未被吞掉）、`committed_journal_left_behind_reads_as_finished`、
`an_unadopted_completed_journal_cannot_be_retired_for_another_move`、`locked_completed_journal_blocks_the_next_move_without_losing_data`、
`a_finished_root_can_retire_its_journal_and_really_move_again`、`finish_only_adopts_a_committed_journal`。

### V4 · 保留进程必须有持续退出监听 —— 已关闭

取消/超时保留进程的分支现在把 child 交给 `observe_child_exit`（三处调用点），退出后清除内存与持久登记、关闭该实例的窗口并发一次通知；
watcher 带 pid 归属判断，旧 watcher 不会误伤重新启动后的新进程。

覆盖测试：**`kept_child_exit_clears_rows_and_delivers_one_notification`**（保留后子进程自行退出 → 登记清空、只发一次通知）、
`a_watcher_only_closes_the_window_of_its_own_process`；前端侧覆盖了报告点名的竞态：
`does not resurrect a kept process whose exit preceded the launch error`（退出事件早于启动错误到达 → 不得复活成运行中）、
`observes a kept process exiting after the launch error`、`shows a surviving process as running and stoppable after an aborted launch`。

## 未关闭的边界（放行前仍必须完成）

- 真实断电 / 硬退出注入：本轮全部用隔离目录与注入的探测函数验证时序，未在真实迁移或真实插件升级中途断电。
- 干净 Windows 环境：安装、首启、卸载与完整业务链路未执行；历史本机安装记录不能代替。
- `cargo audit` 未安装，Rust 依赖漏洞审计未完成。
- 安装包已在本轮修复之上重建：src-tauri/target/release/bundle/nsis/PHL_0.1.0-alpha.1_x64-setup.exe，2,975,157 字节，构建于 2026-09-09 17:36，SHA-256 EA57009FC01CCCAA79E83D6A5FD66FD38DF79E85BFDAD9FDACB6E8346BAE9726；release 二进制的内嵌图标已是新标识。但**安装后的行为仍未验收**，构建成功不等于安装可用。

## 结论

代码层面的四项 P1/P2 已全部关闭并有回归保护。放行条件只剩上面三条真机与供应链检查。
在这三项完成之前，公开预览维持 **NO-GO**；内部 Alpha 可继续使用一次性测试环境试用。
