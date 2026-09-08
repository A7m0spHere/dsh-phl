# Alpha 发行验收 · 2026-09-08

## 判定

**当前不通过公开 Alpha 发行验收（NO-GO）。** 已具备内部试用的功能基础，但当前提交的 Windows CI 未通过，进程接管/窗口生命周期仍有未关闭缺陷，发行版本标识和安装后的真实桌面验收也未完成。

> **2026-09-08 深夜更新**：候选提交的 Windows CI 已复跑全绿（见下文第 1 项与更新表），进程接管与窗口所有权两项代码缺陷也已修复。判定仍为 **NO-GO**，剩余理由是迁移恢复支持边界、Alpha 版本标识，以及干净环境安装与真实桌面验收尚未执行。

验收对象：GitHub `main` 的 `6c01c76b901b40759bd42c4d24b63a5176112792`，对应工作仓库 `f1b5061` 的代码树（按既定发布规则排除本地文档）。本次保持产品代码不变，只补验收记录；未创建 tag 或 GitHub Release。

## 证据矩阵

| 门禁 | 结果 | 证据及边界 |
|---|---|---|
| 发布源码可追溯 | PASS | 发布副本 `main` 已同步 `6c01c76`，工作区干净；上一轮已验证过滤文档后的完整代码树一致。 |
| 前端自动检查 | PASS | 本提交的 GitHub CI：`npm ci`、typecheck、bridge、78 项 Vitest、Node runner 测试、build 均成功。 |
| Windows Rust 格式与静态检查 | PASS | 同一 CI 的 fmt 与 `clippy --workspace --all-targets --all-features -- -D warnings` 成功。 |
| Windows Rust 测试 | **FAIL** | 桌面库 268 通过、2 失败、6 忽略；该次 cargo 命令未完成后续 workspace 测试。两个失败均是短/长路径文本比较，见下文。不能用同一代码先前在本机的 306 项通过代替 runner 结果。 |
| 浏览器模式页面冒烟 | PASS | 本轮运行 `scripts/browser-smoke.py`：版本、插件、API、Runtime、设置、实例六个导航，创建页、不存在实例详情页均通过；无 pageerror/console error。只覆盖 Mock 模式。 |
| 当前源码 NSIS 构建 | PENDING | 已重新执行 `npm run app:build`；结果与产物校验将在构建结束后登记。开始时磁盘已有的 20:00 产物早于本次修复，未用于放行。 |
| 真实桌面交互 | **未完成** | Computer Use 的 `list_apps` 首次、重试及重置后重试均超时，按工具规则停止；未使用旧截图或历史手测替代。 |
| 干净环境安装/卸载/首启 | **未完成** | 未在干净 Windows 用户环境执行 NSIS 安装、首启与卸载。构建成功不等同于此项通过。 |
| Alpha 标识与发布流程 | **未就绪** | package.json、Cargo.toml、tauri.conf.json 均为 `0.1.0`；现有 release workflow 要求 tag 等于应用版本，并仅把带 `-` 的 tag 标为 prerelease。直接打 `v0.1.0-alpha.1` 会版本检查失败，打 `v0.1.0` 则标为正式版。 |

CI：[run 34229973192](https://github.com/A7m0spHere/dsh-phl/actions/runs/34229973192)。这是本轮查询到的最终结果，替代上一轮“正在运行”的临时状态。

## 发行前必须关闭的事项

1. ~~**修复 Windows CI 路径断言并在候选提交上重新跑绿。**~~ **已关闭（2026-09-08 深夜）**：发布副本 `458c3d5` 的 CI 全绿——[run 34245682014](https://github.com/A7m0spHere/dsh-phl/actions/runs/34245682014)，Frontend 52s + Rust（fmt · clippy · test · check）3m40s。
   - 两条断言改按**目录身份**比较（两侧 canonicalize），不再比较 8.3 短名与长名的字符串。
   - 首轮重跑暴露了第二个、只在 runner 上出现的缺陷：同盘迁移用**原始文本**分类链接，而 runner 的 `%TEMP%` 是 `C:\Users\RUNNER~1\…`、数据根 canonicalize 成 `C:\Users\runneradmin\…` → 受管链接被误判为“指向受管版本之外”。修复：`canonical_with_suffix` 解析两侧**存在的最长祖先**再比较（被搬迁子树的旧路径已不存在，canonicalize 无法直接使用），并补一条 Windows 大小写等价回归。
2. **补齐接管进程退出监听和窗口所有权。**（代码已修，见更新表；下方第 3 条的真机故障注入验收仍未执行。）
   - `launch/mod.rs:265` 的 `adopt_processes` 只登记，没有继续监听进程退出；PHL 重开后接管的 DSH 再崩溃，界面和 WebUI 可能保持运行状态。
   - `launch/mod.rs:629` 的旧 watcher 按 PID 清理进程表，却按 instance id 关闭 WebUI；快速重启时旧退出可能关闭新窗口。
   - 以上为当前代码路径确认，本轮未做真实 GUI 故障注入。验收应覆盖接管后退出、主动停止、PID 复用、旧退出晚于新启动，以及新 URL/token 的窗口重开。
3. **明确迁移的 Alpha 支持边界。** 目前歧义落位状态会保留双方并报错，需要人工核对；跨盘撤销仍依赖 rename。要么完成恢复日志与跨盘恢复验收，要么在 Alpha UI 中明确禁用尚未支持的迁移恢复操作，不能只在发布说明里声称“可恢复”。
4. **准备真实 Alpha 候选和安装验收。** 统一三处版本及锁文件，选择带 Alpha 后缀的候选版本；候选 CI 通过后在干净 Windows 环境验证安装→首启→创建→启动/停止→克隆/快照→Pack 导入/导出→重启接管→卸载，并给产物记录 SHA-256。

## 验收工具的额外缺口

`PHL_ROOT` 模式设置 `pointer: None`，`sibling_file()` 因此返回 `None`。`lib.rs` 不会绑定持久进程登记，`storage.rs` 也不会持久化迁移 journal，撤销会报“没有迁移记录可撤销”。所以 `npm run test:desktop` 的隔离启动虽然保护了真实目录，却**不能用于证明真实模式的重启接管与迁移恢复**。这些流程需要独立的持久测试配置目录或专用 Windows 用户环境。

先前 Alpha 场景文件还要求超长名称有明确拒绝，但手测记录将它列为未决观察项；正式验收应先统一该项预期。它是低优先级体验问题，不与数据恢复、进程状态等发行阻塞混为一谈。

## 收尾

浏览器冒烟遗留的 preview 子进程已按本次已确认的 PID 和命令行关闭；未结束用户原有 PHL 进程。过去的手测、单元测试和本轮的实际执行结果分别标注，没有将未完成项勾选为通过。

## 更新（2026-09-08 22:20，代码修复轮）

本节只记录代码侧进展，**不改变上面的 NO-GO 判定**：判定依赖候选提交的 CI 重跑与真机安装验收，两者都还没有复跑。

| 上文阻塞 | 现在 | 提交与证据 |
|---|---|---|
| Windows CI 路径断言（`storage.rs` 两条链接测试） | **已关闭** | `6788d80` 改断言为目录身份比较；runner 上又暴露同盘迁移按原始文本分类链接的缺陷，`458c3d5` 用 `canonical_with_suffix` 修掉。[run 34245682014](https://github.com/A7m0spHere/dsh-phl/actions/runs/34245682014) 全绿 |
| 接管进程退出监听（`launch/mod.rs:265`） | **已修** | `fd94a39`：新增 `watch_adopted_process`，用收养时的同一身份门轮询，判定不再是本机进程后执行与 launch watcher 一致的清理 |
| 旧 watcher 按 instance id 关窗（`launch/mod.rs:629`） | **已修** | `fd94a39`：`Processes::is_current_or_empty` 决定关窗权限——新 pid 保窗、纯 stop 仍关窗；回归 `a_watcher_only_closes_the_window_of_its_own_process` |
| 迁移恢复边界 | 未动 | 待产品决定：补跨盘恢复，或在 Alpha UI 中禁用尚不支持的恢复操作 |
| Alpha 版本标识 | 未动 | 待维护者定版本号与 tag 规则 |

仍未关闭的验收项：干净 Windows 用户的 NSIS 安装/首启/卸载、真实桌面交互（含接管后退出、PID 复用、旧退出晚于新启动的故障注入）、迁移恢复支持边界、Alpha 版本标识。候选提交的 Windows CI 已复跑通过。

补充：`6788d80` 同时修了 设置 → 存储 → 切换数据目录 的三个缺陷（后端拒绝时的静默假成功、取消迁移误报失败、切根后存储页不刷新）——不属发行阻塞，但与"迁移恢复"同一条路径。
