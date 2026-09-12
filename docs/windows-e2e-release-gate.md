# Windows E2E Release Gate（2026-09-11 立项）

> 目标：把「普通用户安装后一定没问题」变成机器可判定的发布门禁。
> CI 此前的 Windows 覆盖到「代码正确」（fmt/clippy/test/check），本门禁补上
> 「装得上、起得来、跑得动、坏得了、删得净」。

## 车道（lanes）

| 车道 | 位置 | 运行方式 | 在哪个门禁 |
|---|---|---|---|
| 无头核心（headless core） | `src-tauri/src/release_e2e/` | `cargo test --workspace -- --ignored release_e2e --test-threads=1` | CI `windows-e2e` job（每次 PR/push）+ Release gate job |
| 安装器冒烟·宿主车（hosted lane） | `e2e/install-smoke.mjs --no-gui` | `npm run test:e2e:smoke -- --installer=<setup.exe> --expect-version=<V> --no-gui` | Release gate（tag 触发，构建产物上真跑）；CI dispatch `tauri-build` job 预演 |
| 安装器冒烟·真机车道（desktop lane） | `e2e/install-smoke.mjs`（不加 `--no-gui`） | 交互桌面上的提权 PowerShell 执行同一条命令 | 发布前人工门禁；报告存档 `docs/alpha5-installer-smoke-manual-2026-09-12.json` |
| VM 档（未自动化） | 手测清单 §15 剩余项 | 干净 Hyper-V VM | v1 前逐项人工执行并登记 |

统一本地入口：`node scripts/e2e-gate.mjs core|smoke|all`。
`--test-threads=1` 是硬要求：用例绑定真实 socket、拉起真实子进程、探测真实 pid。

无头车道的实现要点：所有下载走 `MockServer`（本地 HTTP，路由可中途变更以注入
故障），假 DSH 包无依赖（跳过真实 npm），假 Node 包内是**宿主真实 node.exe**
（Runtime 健康门禁会真的执行 `node --version`）。驱动的是生产同一条
`*_inner`/`run_install`/`run_launch` 管线（`run_launch` 为此把唯一的
`AppHandle` 依赖抽象成 `retain_child` 回调）。

## 场景 ↔ 手测清单映射

| Gate 用例 | 覆盖的手测清单项 | 状态 |
|---|---|---|
| J1 `j1_cold_install_launch_and_stop` | §1 Runtime 下载/校验 · §2 版本下载/校验/安装 · §3 创建实例 · §5 启动（真实 DSH 子进程 + readiness）· §5 停止 · §15 的「下载→安装→启动」数据面 | ✅ 本机 2026-09-11 通过 |
| J1 附加：无 integrity 的 catalog 项 | O-11 skip 路径、真实 `list_dsh_versions` 对 mock registry | ✅ |
| J2 `j2_restart_adopts_a_live_child` | §10 进程接管（PHL 重启按真实 pid/exe/创建时间对账接管）· R3/R4 登记语义 | ✅ |
| J3 `j3_snapshot_then_pack_rebuild` | §7 快照→修改→回滚与磁盘一致 · §13 Pack 导出→新机安装（凭据剥离链）| ✅ |
| J4 `j4_storage_migration_paths` | §10/O-06 迁移：完整提交、按 journal 续传、未提交整体撤销 | ✅ |
| J5 `j5_updates_manifest_contract` | §15 更新清单契约（真实 updates 分支 JSON、minisign keyid 对 `tauri.conf.json` 公钥） | ✅（在线） |
| F1 `f1_external_force_kill_leaves_a_clean_stop` | §5/§10 外部强杀后停止路径干净、不假运行 | ✅ |
| F2 `f2_aborted_download_leaves_no_half_install` | §15 中断类：下载半程断流→无半成品、重试成功 | ✅ |
| F2b `f2b_cancelled_download_resumes_and_rejects_changed_content` | O-11：取消保前缀 → 续传 206 完成 → **异体内容绝不混拼**（声明 A 的摘要拒收 B） | ✅ |
| F3 `f3_locked_destination_refuses_without_losing_the_old_install` | §10 文件锁注入：重安装目标被占 → 旧安装不丢（磁盘满的 CI 同族代理） | ✅ |
| F4 `f4_late_exiting_child_keeps_registration_until_confirmed` | §10 旧退出晚于新启动：kept-alive 拒绝双启动、登记不丢、确认退出后可停可再启 | ✅ |
| （无 F5 用例） | §10 磁盘满 | ⏸ VM 档人工执行——自动化占位会被计入 PASS 数却什么都不断言，故删除；`[disk-full]` 分类由 `storage.rs`/`versions/install.rs` 单测覆盖，CI 可跑的兄弟场景是 F3 |
| I-1…I-9 `install-smoke.mjs` | §15 安装包冷启动 ⭐ / 卸载 ⭐ / 首帧渲染 / console 零异常 / 升级 / 用户数据保留 | 真机车道 **11/11 通过**（2026-09-12，alpha.5，报告入库）；宿主车道 8 步通过 + 渲染三步显式 SKIP（见下「宿主边界」） |

## 诚实的覆盖边界（门禁不证明什么）

1. **窗口与图形交互**：`app_ready` 显窗、关闭确认、WebUI 内嵌窗、托盘、对话框
   ——冒烟只证明「窗口被创建且第一帧渲染、真实 IPC 可往返、3 秒 console 零异常」。
   视觉与交互细节仍属手测清单（alpha-desktop GUI-1..4）。
2. **`watch_child_process` 的 GUI 侧退出事件**：无头车道覆盖到决策与数据面；
   「子进程退出 → 前端状态变化」的事件链在冒烟车道也只在无人偶时验证。
3. **一键更新点击安装**：清单契约（J5）+ 安装器升级（I-7）已自动化；
   「应用内点更新 → 弹安装 → 重启」整链留 VM 档。
4. **SmartScreen/真实下载体验**：CI 直接跑本地 exe，不经过浏览器下载。
5. **UI 状态机 mock 深测**（stale promise、遮罩残留等 GUI-3/GUI-4 项）：未进 v1，
   列为 smoke 车道的后续增强（CDP 页面上下文可注入 mock invoke）。
6. **磁盘满**：见 F5，VM 档。

### 宿主边界：WebView2 DevTools 在哪里能开（2026-09-12 定案）

渲染三步（I-3b 页目标 · I-4 真实 IPC · I-5 console 零异常）依赖 WebView2 的
`--remote-debugging-port`。alpha.5 发布轮用同一份安装包、同一套代码做的对照实验：

| 宿主 | 启动方式 | 结果 |
|---|---|---|
| 普通权限桌面进程 | 直接 spawn | 端口 ~2 秒内可用 ✅ |
| 提权桌面控制台（High IL） | 直接 spawn（含 `--no-sandbox`） | 端口始终不出现 ❌ |
| 提权桌面控制台 | Interactive+Limited 计划任务 broker | 端口可用 ✅（真机 11/11） |
| GitHub hosted runner | 同上（broker 生效，应用与 5 个 WebView2 子进程都起来了） | **`app=True wv=5 bind=0`：端口始终不绑定** ❌ |

结论：托管 runner 上「应用能起」≠「DevTools 能开」。因此 `--no-gui` 把渲染三步
变成**显式 SKIP**（报告里写明「deferred to the manual desktop lane」），其余 8 步
（安装 · 版本 · 清单契约 · 升级 · 卸载 · 用户数据）继续阻塞；渲染三步由**真机车道**
以同一脚本、不加 `--no-gui` 跑满 11 步并把报告入库。SKIP 永远留痕，不会静默变绿。

失败时的取证：`connectCdp` 超时的 FAIL detail 会附
`[scene: app=<进程存活> wv=<PHL 的 WebView2 子进程数> bind=<端口监听数>]`。

## 复跑与证据

- 本地核心：`npm run test:e2e:core`（需 `node` 在 PATH；全程不触网，J5 例外需可达 GitHub）。
- 本地仅运行冒烟（不装机）：`node e2e/install-smoke.mjs --exe=src-tauri/target/release/dsh-phl.exe`。
  **注意：`--installer` 全模式会真实安装并卸载 PHL，只在一次性环境使用。**
- CI 证据：`windows-e2e` job 的测试日志；Release gate 上传 `e2e-release-gate` artifact
  （`install-smoke-*.json`，含每步 verdict 与时间戳）。
- 首跑记录：本机 2026-09-11，`cargo test --ignored release_e2e` **11/11 通过**（约 5.5 分钟）；
  `cdp-spike.mjs` 对 release exe 6 步全过；`install-smoke` exe 模式 5 步过 + 升级/卸载 3 步 skip。

## 与发布流程的关系

`release.yml` 在「Hash artifacts」之后、「Build the updater manifest / Create
GitHub Release」之前运行双门禁：核心车道 + 安装器冒烟（真包、`--expect-version`
对照）。任一失败 → 不发 Release、不推 `updates` 分支——安装过的应用不会收到坏更新。
v1 前该 job 保持必过。

## 复跑记录

| 日期 | 结果 | 说明 |
|---|---|---|
| 2026-09-11 | 核心 11/11 通过（首建；F5 删除后口径为 10/10）；CDP spike 6 步过；冒烟 exe 模式过 | 见上文「首跑记录」 |
| 2026-09-12（alpha.5 发布轮） | 真机车道 **11/11 通过**（安装→版本→首启渲染→IPC→console→清单契约→升级→卸载→数据保留，exit 0，报告 `docs/alpha5-installer-smoke-manual-2026-09-12.json`）；宿主车道路由为 `--no-gui` | 门禁自身修了三处：① locate 只看 `PHL.exe` 且不剥注册表引号 → 改为按模板真实布局定位（`dsh-phl.exe`）；② 提权宿主下 WebView2 不开 DevTools → 应用改由 Interactive+Limited 计划任务 broker 启动；③ 超时不再盲报，附 `[scene: …]` 取证 |
| 2026-09-11 二轮 | 核心 11/11 过（node v24 重验机器无关性）；**全部 17 个 `--ignored` 测试首次全绿**；冒烟 exit=0；alpha-gate 8 步全绿 | 本轮修了两处门禁自身缺陷：① J5/I-6 将「网络传输失败」降级为 SKIP（端点答复了但内容不对才是契约 FAIL）——门禁 flaky 等于门禁作废；② 既有 `dependencies_stay_inside_the_plugin_and_scripts_do_not_run` 在 pnpm 12 + 无符号链接权限（WinError 1314，未开开发者模式）下失败：对照实验证明同套 pnpm flags 对 registry 依赖顶层链接正常，只有测试 fixture 用的跨根 `file:` 依赖在 store 里没建顶层链接——产品功能不受影响（插件依赖都是 registry spec），测试改为布局无关的封闭性断言后通过。 |
