# PHL 稳定化 / Hardening / 架构收敛 — 2026-09-10

本轮为 **Stabilization / Hardening Sprint**（非新功能）。基线分支 `fix/homomorphic-bug-audit`，
在既有审计修复之上，对两份审计（`PROJECT_REVIEW_2026-09-10_agent-team.md`、
`PROJECT_UX_REVIEW_2026-09-10.md`）与 `.scratch/review/*.md` 逐条**以当前代码为准**复核，
再用统一的机制修复一整个 bug 家族，并为关键边界补齐永久回归测试。

**全部验收门通过**（见 §9）：typecheck / 前端单测 / cargo workspace 测试 / clippy(-D warnings) /
bridge check / version consistency，且在**本机 Windows** 上确认了 `#[cfg(windows)]` 路径测试真实执行。

2026-09-11 发布复核又关闭了四类边界：迁移源必须是后端当前数据根；Windows junction
路径不再被 `cmd.exe` 元字符二次解析；整合包拒绝大小写碰撞与未声明的会话数据；
Pack CLI 用暂存目录解包并拒绝覆盖已有目标。另将供应商凭据绑定从“同 host”收紧为
“同 scheme + host + effective port”，并把 `package-lock.json` 纳入版本一致性门禁。

---

## 1. 本轮确认的问题（Confirmed）

| 编号 | 严重度 | 位置 | 结论 |
|---|---|---|---|
| A1 | **P0** | `versions/install.rs::sanitize_version` | `"..."` / `..` 之外，仍放行**尾随点/空格**与 **Windows 保留设备名**（`CON`、`NUL.txt`…）。`remove_version_dir` 会据此 `remove_dir_all` → 数据损失。报告成立。 |
| A2 | **P1** | `phl-pack-core/lib.rs::normalize_entry` | 只防遍历/绝对路径，**不拒绝冒号**（NTFS ADS）、尾随点、保留名。`payload:secret` 能写入 Explorer/完整性清单看不见的备用数据流。报告成立。 |
| A3 | **P1** | `instances/manifest.rs::write_manifest` | 落盘前**只规范化 version_id**，不校验 `id/profile/runtime_id/port/external_home`；`save_instance` 因此可把逃逸型 profile 写入，后续 `profile_root_of` 拼进 `<home>/profiles/<profile>` 再被 `create_dir_all`/插件装卸使用。报告成立。 |
| C1 | **P1** | `api_config/models.rs::fetch_models_inner` | 混淆代理：IPC 任意 `base_url` + `provider_id` → 后端从系统凭据库取该 provider 的 secret 发往**任意 host**。报告成立。 |
| D1 | **P1** | `runtimes/mod.rs::remove_runtime_dir_body` | 删除 Runtime **既无实例引用守卫，也不先改名隔离**，直接原地递归删（对照 version 删除两道都有）。报告成立。 |
| D2 | **P1** | `instances/manifest.rs::classify_manifest` | 把 `read_to_string` 的**所有** 错误（含 PermissionDenied/IO）折叠成 `Missing`，使"读不到"被当成"不存在"，可被 `remove_dir_all`。报告成立。 |
| D3 | **P2** | `instances/mod.rs::delete_instance_inner` | 破坏性删除前**不 `classify_manifest`**，不会拒绝新 schema / 不可读清单的实例。报告成立。 |
| D4 | **P2** | `versions/install.rs::promote_staged` | 回滚 `rename` 结果被 `let _ =` 丢弃，却**无条件**返回"已恢复原版本"；无原版本时也称"已恢复"。报告成立。 |
| E1 | **P2** | `launch/registry.rs::persist_locked` | 整表覆盖写 `processes.json`，**双开 PHL 时后写者用自身快照覆盖，丢掉另一进程的运行实例行**（且无单实例约束）。报告成立。 |
| U1 | **P2** | `services/tauriLaunches.ts` + 详情页 badge | 正在安装（downloading/extracting/installing-deps/verifying）的版本，点击启动得到 **"DSH 版本未安装"**、badge 也显示"未安装"——误导。报告成立。 |

## 2. 被降级 / 否定的问题及原因

- **`ParsedError.retryable` "死字段"** → **否定**：`errorCodes.ts` 产出、`errorCodes.test.ts` 断言、
  调用方据此加"重试"动作（`catalogStore.ts:95` 等）。不是死字段。
- **`instanceStore.load()` "死调用"** → **否定**：`packStore.ts:148`、`adoptionStore.ts:331` 均调用它刷新列表。是活代码。
- **`clear_download_cache` / manifest writer 未持正确锁** → **已核对，非 bug**：写盘均为 tmp+rename，
  `write_manifest` 用 per-writer 唯一临时名 + `commit_seq`，`PhlState.persist` 先落指针再改内存根。
- **snapshot restore / delete 谎报成功** → **上一轮已修**（commit `cabf330`：restore/delete 成为诚实长任务）。本轮仅补 UI 入口预禁用。
- **WebUI 无条件自动打开不是安全 bug** → 确认；按产品 UX 决策处理（加开关，见 §4）。
- **reduced-motion "关闭动画是假的"** → **部分成立并修正**：Framer 路径早已 `MotionConfig` 生效，
  系统 `prefers-reduced-motion` 有独立 CSS 兜底且未被破坏；真正缺的是**应用内 off 未覆盖 CSS animation**（见 D5/U-motion）。
- **port allocation TOCTOU** → **降级为已知债务**：现有 `port_free` 已做真实 `TcpListener` bind 探测
  （远优于旧的纯查表），剩余"探测后到子进程 bind 之间"的窄竞态属"理论上可发生、实践中影响极小"，
  本轮不引入跨进程预留锁（避免过度工程）。记录为债务。

## 3. 本轮确立 / 收敛的架构不变式

1. **可移植文件名的 Windows 不变式只实现一次**：`paths::is_windows_safe_name`
   成为 id/profile/version/runtime 段的共享底座（拒绝 `.`/`..`/`...`、尾随点/空格、前导点、保留设备名）。
   `sanitize_segment` 与 `sanitize_version` 均由它收窄。**archive 条目名**（pack-core）因语义不同
   （合法 dotfile）只在**同一不变式家族**下共享"冒号/尾随点/保留名"子集，差异在代码注释里写明。
   > 以后同类"某个 Windows 路径怪象"只需改这一处，不再逐点打补丁。
2. **凡写 `instance.json` 必过后端统一持久化校验**：`write_manifest` 是唯一落盘点，
   内部 `validate_instance_manifest_for_persistence` 覆盖 id/profile/runtime_id/port/external_home；
   create/adopt/pack/save 各自的前置 `sanitize_segment` 保留为早失败，但不再是安全边界。
3. **凭据身份 ⇄ 请求目的由后端可信配置绑定**：系统凭据（`creds.get(provider_id)`）只允许发往
   该 provider 在 `ApiConfig` 里登记的 base host；一次性 `api_key` 才可用任意 URL。
4. **破坏性操作 fail-closed**：`ManifestRead` 新增 `Unreadable`，所有消费点（list / orphan 扫描 /
   orphan 删除 / `load_manifest` / `delete_instance_inner`）对"存在但读不到"与"新 schema"一律
   **拒绝继续**，只有 `Missing`/`Corrupt`（可回收垃圾）才允许删除。Runtime 删除补齐"引用守卫 + 先改名"。
5. **回滚成功才声称回滚成功**：`promote_staged` 的失败信息按"是否有原版本 / 恢复是否真的成功"如实措辞，
   并给出 backup 路径。
6. **共享 durable state 不被后写者整表覆盖**：`processes.json` 落盘改为**加跨进程文件锁的 read-modify-write 合并**
   （fs2 已在依赖内），本进程的删除用 **tombstone** 精确表达，别的管理器的行不再被误删。
7. **"实例此刻是否活跃"只有一份定义**：`isInstanceLiveForSnapshot(status)` 同时被 store 守卫与详情页禁用逻辑读取。

## 4. 具体修复

**Phase 2A（路径安全）**
- `paths.rs`：新增 `WINDOWS_RESERVED` + `is_windows_safe_name`；`sanitize_segment` 复用它。
- `versions/install.rs::sanitize_version`：复用 `crate::paths::is_windows_safe_name`（runtime 删除已共享此函数）。
- `phl-pack-core/lib.rs::normalize_entry`：逐段调用新增的 `is_safe_entry_component`（拒绝 `:`/尾随点/尾随空格/保留名，保留合法 dotfile）。

**Phase 2C（凭据信任边界）**
- `api_config/models.rs`：`fetch_provider_models` 命令新增 `PhlState` 依赖，加载该 `provider_id` 在 `ApiConfig` 中的 `base_url`；
  新增纯函数 `url_host` / `saved_credential_trusted`；`fetch_models_inner` 仅在目的 host == provider 可信 host 时才动用系统凭据，
  否则降级为一次性 key / 环境变量（或 `ENV_MISSING`）。

**Phase 3（破坏性操作）**
- `instances/manifest.rs`：`classify_manifest` 仅 `NotFound → Missing`，其余 IO 错误 → 新 `Unreadable`；
  `read_manifest`（列表隐藏但不当垃圾）、`load_manifest`（id 定位操作中止）、`write_manifest`（统一持久化校验）；
  新增 `validate_instance_manifest_for_persistence`。
- `instances/mod.rs`：`delete_instance_inner` 删除前拒绝 `UnsupportedSchema`/`Unreadable`；
  orphan 扫描/删除对 `Unreadable` 也保护（continue / Err）。
- `runtimes/mod.rs::remove_runtime_dir_body`：新增 `instances_referencing_runtime` 全量扫描引用该 runtime 的实例并拒绝；
  删除改为 `.phl-REMOVE-*` 先改名隔离（对称 version 删除）。
- `versions/install.rs::promote_staged`：抽出 `restore_backup` + `rollback_message`，如实报告恢复结果。

**Phase 4（并发 durable state）**
- `launch/registry.rs`：`RegistryState` 新增 `tombstones`；`forget`/`forget_pid` 仅在删除自有行时打 tombstone；
  `persist_locked` 改为对 `processes.json.lock` `fs2` 独占锁内的 read-modify-write 合并（RAII 解锁）+ 原子 rename。
  `Cargo.toml` 的 `fs2` 保持 `0.4`（`FileExt` 无需 feature，**不引入新依赖**）。

**Phase 5/6（UX 语义收敛）**
- `tauriLaunches.ts` + `mockRepository.ts` + `InstanceDetailPage.tsx`：区分"正在安装"与"未安装"（启动前置错误 + badge"安装中"）。
- `settingsStore.ts` + `instanceStore.ts` + `SettingsPage.tsx`：新增 `autoOpenWebUi`（默认 true，保持现状），启动就绪时按其决定是否自动开 WebUI。
- `apiConfigStore.ts`：另一实例正在同步时由**静默 return** 改为 toast「已有同步任务正在执行」。
- `types/instance.ts::isInstanceLiveForSnapshot` + `InstanceDetailPage.tsx`：实例活跃时创建/回滚快照按钮**提前禁用 + Tooltip「请先停止实例」**（restore 不再让用户走完确认才报错）。
- `uiStore.ts::setMotion` + `index.css`：应用内"关闭"写 `data-motion` 属性，新增 `[data-motion='off']` 规则关停 CSS `animation/transition`；`onRehydrateStorage` 重新应用；系统 `prefers-reduced-motion` 保持独立不破坏。
- `TaskCenter.tsx`：消费此前**死掉的** `taskStore.recentFailures()`，在无运行任务但存在失败时于"任务"按钮显示红色失败计数徽标，让短暂 toast 消失后失败仍可发现。

### 复核（本轮 review 遍）期间发现并修复的问题
1. **`registry::remember` 与 tombstone 冲突**（真实回归）：同进程内先 `forget`（stop）再 `remember`（start）同一实例，
   `merge_and_write`「先插入 entries、后按 tombstones 删除」会把刚记住的活行从磁盘抹掉，重启后不被接管。
   → `remember` 现在清除该 id 的过期 tombstone，并补 `remember_after_forget_clears_the_stale_tombstone` 回归。
2. **`Tooltip` 空内容空气泡**：新加的快照按钮用 `content=''` 表示"无需提示"，但 `Tooltip` 仍会在 hover 后弹出一个无文字的深色方块。
   → `Tooltip.show()` 在 `content` 为空时直接不触发（对所有调用方向后兼容）。

## 5. 新增回归测试（26 项）

**后端（Rust）**
- `paths::tests::segments_reject_windows_special_names`（`...`/尾随点空格/前导点/保留名 + 合法版本号不误伤）
- `versions::install::sanitize_tests::version_segment_refuses_windows_dangerous_shapes`（含 `bound_id_for_version`）
- `phl-pack-core::normalize_entry_rejects_windows_unsafe_names_allows_dotfiles`（**冒号 ADS**、尾随点/空格、保留名；dotfile 放行）
- `api_config::tests::saved_credential_never_sent_to_a_foreign_host`（**凭据 host 绑定**）
- `instances::manifest::tests::an_unreadable_manifest_is_not_reported_as_missing`
- `instances::manifest::tests::persistence_validation_refuses_path_escaping_fields`
- `instances::manifest::tests::write_manifest_refuses_a_profile_that_would_escape_the_home`（真实写边界）
- `instances::tests::delete_refuses_a_newer_schema_manifest`
- `runtimes::tests::deleting_a_runtime_bound_to_an_instance_is_refused_and_leaves_it_intact`
- `runtimes::tests::instances_referencing_runtime_scans_real_manifests`
- `launch::registry::tests::a_commit_preserves_another_process_registration`（**双开合并** + tombstone 语义）
- `launch::registry::tests::remember_after_forget_clears_the_stale_tombstone`（**复核新增**：同进程 stop→start 后，过期 tombstone 不得抹掉重新 remember 的活行）
- `versions::tests::promote_reports_a_failed_rollback_instead_of_a_false_restore`（final_check 失败**且** `rename(backup→dest)` 恢复失败 → 断言文案含「恢复原版本失败」+ backup 路径，且**不含**「已恢复原版本」——D4 的失败分支）

**前端（Vitest）** — `src/stores/stabilization.test.ts`
- `uiStore.setMotion` 写 `data-motion` 属性
- `isInstanceLiveForSnapshot` 活跃/静止状态集
- `settings.autoOpenWebUi` 默认 true + 可设

（既有 `phl.windows` / `verbatim` / `symlink` `#[cfg(windows)]` 测试在本机 Windows 下确认执行并通过。）

## 6. 尚未处理 / 明确记录的剩余债务

1. **port allocation TOCTOU**：bind-probe 与子进程真正 bind 之间的窄竞态未消除（刻意不引入跨进程预留锁）。
2. **processes.json 合并的残余竞态**：跨进程锁串行化了每次读改写；但两进程"同一瞬间"的读改写被锁保护，
   而**跨进程对同一 instance_id 的所有权语义**（谁有权 stop 一个 PID）仍未建模——双开管理器同时管一个实例本就不是受支持用法。
3. **实例卡片菜单「创建快照」运行时的预禁用**：store 已用 toast 拦截（非静默），仅详情页主按钮做了提前 disable；卡片菜单入口保留 toast 反馈。
4. **launch 安装中态的按钮级禁用**：详情页 badge + 点击后错误文案已诚实；实例卡片/启动坞的主"启动"按钮未针对"版本正在安装"额外置灰
   （数据流需把 version 下传到卡片；点击已给出正确提示，成本低收益有限，暂缓）。
5. **前端专属失败的集中留存入口**：TaskCenter 徽标只覆盖**后端登记的** task；纯前端错误（如快照预禁用前的旧路径、API 同步）
   仍依赖瞬时 toast + 详情页 error Notice。未搭全局错误平台（刻意）。
6. `SettingsPage`/`launch` 体量、列表虚拟化等：未做机械拆分（不满足"多职责/测试边界/减少重复"门槛）。

## 7. 是否仍存在 release blocker

**无 P0/P1 级 release blocker 遗留。** 本轮所有 P0/P1（A1、A2、A3、C1、D1、D2）均已修复并有回归测试。
D3/D4/E1（P2）与全部 UX 项亦完成。剩余项（§6）均为"理论上可发生、实践中影响小或属既不受支持的用法"的债务，不阻断发布。

## 8. 下一轮继续新功能开发前的建议

- 先补 **Windows 真机手测清单**（`dsh-phl-manual-test-checklist.md`）里针对本轮的三项：
  ① 造一个尾随点/保留名目录确认扫描不列、删除拒；② 双开 PHL 各起一个实例，确认 `processes.json` 不互覆盖；
  ③ `动画=关闭` 下确认 `animate-spin`（如运行光晕、下载进度）确实停。
- 若产品要正式支持"多窗口管理同一数据根"，需要先设计**跨进程进程所有权模型**，再谈把 §6.2 的合并升级为带属主的持久状态机；
  否则建议在 `README`/设置里**明确写"单实例管理器"**，与当前不变式对齐。
- 本轮确立的共享不变式（尤其 `is_windows_safe_name`、`validate_instance_manifest_for_persistence`、`isInstanceLiveForSnapshot`、
  凭据 host 绑定）应视为后续任何新路径类型 / 新 IPC 写点的**必过关卡**——新代码复用它们，而不是再写局部 `if`。

## 9. 验收门（全部绿）

| 门 | 命令 | 结果 |
|---|---|---|
| TypeScript typecheck | `npm run typecheck` | 通过 |
| 前端单测 | `npx vitest run` | 21 files / **126** passed |
| Rust workspace 测试 | `cargo test --workspace` | app **354** + pack-core **37** + cli **4**，0 failed（6 ignored=网络探针）|
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | 通过，0 warning |
| Bridge check | `npm run bridge:check` | missing `[]`，duplicateRegistrations `[]` |
| Version consistency | `node scripts/check-versions.mjs` | `VERSION OK: 0.1.0-alpha.4`；package / package-lock / Cargo / Cargo.lock / tauri.conf 一致 |
| Windows 重点验证 | 本机 Windows `cargo test`（`#[cfg(windows)]` 路径/容器测试实际执行并通过）| 通过 |
