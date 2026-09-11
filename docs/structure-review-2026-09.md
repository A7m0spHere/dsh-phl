# 结构审查 2026-09：控制大文件再生长（下一轮拆分的工作单）

> 定性：不是 bug，是趋势控制。当前后端重新长出了复杂度中心，
> `instances/mod.rs` 与 `storage.rs` 为下一轮结构 Review 重点（路线图 O-13 续程）。
> 本 brief 给出实测数据、建议拆分边界与迁移安全规则；执行时一次一个文件，
> 不混进行为改动。

## 实测基线（2026-09-11，HEAD 含 E2E 门禁轮）

| 文件 | 字节 | 总行 | 生产行 | 测试行 | 测试占比 |
|---|---|---|---|---|---|
| `src-tauri/src/instances/mod.rs` | 108 KB | 2706 | 991 | 1715 | 63% |
| `src-tauri/src/storage.rs` | 84 KB | 2064 | 993 | 1071 | 52% |
| `src-tauri/src/launch/mod.rs` | 69 KB | 1807 | 1074 | 733 | 41% |
| `src-tauri/src/plugins/install.rs` | 57 KB | 1381 | 734 | 647 | 47% |
| `src-tauri/src/instances/adoption.rs` | 53 KB | 1322 | 818 | 503 | 38% |
| `src-tauri/src/instances/copy.rs` | 49 KB | 1133 | 675 | 457 | 40% |
| `src/pages/SettingsPage.tsx` | 46 KB | — | — | — | — |
| `src/pages/InstanceDetailPage.tsx` | 40 KB | — | — | — | — |

关键观察：**后端前两名的一半以上是内联测试**。仓库已有拆出式测试文件的先例
（`pack/export/tests.rs`、`pack/install/tests.rs`、`api_config/*tests*`），
照搬该模式即可先把体量砍半，再谈生产代码的按职责拆分——两步分开提交。

## 建议拆分边界

### 1. instances/mod.rs（先测试、后生产）
- `#[cfg(test)] mod tests;` → `instances/tests.rs`（−1715 行，mod.rs 立降至 ~40 KB）。
- 生产侧三个内聚块各自成文件：
  - `instances/plugins_scan.rs` ← `scan_plugins`/`collect_packages`/`package_json_version`
    （插件清单是独立概念，与实例 CRUD 无共享状态）；
  - `instances/remove.rs` ← `remove_tree_progress`/`remove_tree_step` +
    `scan_orphan_instances*`/`delete_orphan_instance*`（删除与孤儿清扫是一个故障域）；
  - `instances/usage.rs` ← `disk_cache`/`invalidate_disk_usage`/`instance_disk_usage`/
    `instance_session_count`（带缓存的只读统计）。
  - `run_clone` 移入既有 `copy.rs`（它的自然归属；copy.rs 自身随后按同类规则处理）。
- 留在 mod.rs：wire types、路径 helpers、命令组（list/create/save/delete/clone）。

### 2. storage.rs（journal 是天然切口）
- `#[cfg(test)] mod tests;` → `storage/tests.rs`（含跨卷用例与 `double_root` 环境探测，
  全部随行）。
- `storage.rs` + `storage/` 目录（同 `launch` 的结构先例）：
  - `storage/journal.rs` ← EntryState/JournalEntry/MigrationJournal/read/write/
    `journal_started`/`retire_completed_migration`；
  - `storage/move.rs` ← `move_root_data` 命令 + `move_root_inner` + `complete_move` +
    `ensure_migration_source_is_current`；
  - `storage/undo.rs` ← `storage_migration_undo`/`undo_migration`/`copy_back`/
    `ensure_recovery_is_unambiguous`/`storage_migration_finish`/`same_location`；
  - mod.rs 留 RootDataSummary/free_space/Move* 类型 + re-export（ipc 面零变化，
    `check-tauri-bridge` 与 `ipc_contract` 兜底）。

### 3. plugins/install.rs（事务日志独立成域）
- tests 拆出 → `plugins/install/tests.rs`（沿用 pack/install 先例）。
- `plugins/transaction.rs` ← `transaction_dir`/`save_transaction`/`read_transaction`/
  `begin_transaction`/`mark_transaction_committed`/`finish_transaction`/
  `recover_legacy_backups`/`recover_interrupted_swap`（O-04/R2 的全部跨进程恢复逻辑，
  本身就是一个文件该讲一个故事）。
- `install_plugin_dependencies`/`dependency_specs`/`stderr_tail` → 并入
  `plugins/deps.rs`（pnpm 边界，与 R3 真机项同域）。

### 4. launch/mod.rs（第二批）
- tests 拆出 → `launch/tests.rs`。
- `launch/terminate.rs` ← `confirm_exit`/`finish_termination`/`terminate_and_forget`/
  `stop_permission`/`terminate_or_gone`（R3 语义族）；adoption（`adopt_processes` +
  watcher）→ `launch/adopt.rs`。`run_launch` 留主文件（刚为 release_e2e 做过接缝，暂稳）。

### 5. 前端（O-13 既立项，按完整交互拆）
- `SettingsPage` → 按分区组件化：general / downloads / advanced / **storage
  （迁移横幅 + 撤销 + 完成切换，交互最重）**各一文件，页面留装配壳。
- `InstanceDetailPage` → API 卡 / 对话迁移面板 / 快照列表 / env+args / 诊断修复
  各按一个完整交互迁移（O-13 铁律：每次只迁一个交互，行为与错误路径不变）。

## 迁移安全规则（每步执行）

1. 纯移动提交：代码逐字搬移 + `pub(crate)` 可见性调整，**零行为改动**；
   `cargo test --workspace` 354+ 项必须逐提交全绿。
2. 命令注册面（`lib.rs` 的 `phl_command_handler!`）不动；ipc_contract 与
   bridge check 作为防漂移网。
3. 测试拆分先行且单独提交（收益最大、风险最低），生产拆分随后逐块进行。
4. release_e2e 门禁在拆分窗口内照常运行（它测的是管线行为，正是防回退的网）。
5. 一次一个文件，一个 PR；不与其他功能混提。

## 趋势防护（已实施，2026-09-11）

Ratchet 预算表已上线：`scripts/check-file-size.mjs` + `scripts/file-size-budget.json`
（25 个 ≥25 KB 的源文件全部登记，预算 = 当时值 +5%），接入 alpha-gate 与 CI
frontend job。语义：A 登记文件超预算 → FAIL；B 未登记源文件长到 ≥40 KB → FAIL
（登记必须是一次有说明的人工决定）；C 登记路径消失 → FAIL，`--update` 退休它。
文件缩了 >10% 会提示，`node scripts/check-file-size.mjs --update` 把天花板
锁下去——**拆分腾出的空间不允许被悄悄回填**。四条路径均有故障演练验证。
本地命令：`npm run check:size`。
