# Alpha 发行验收 · 2026-09-12（v0.1.0-alpha.5 发布与远端核验）

## 判定

**已发布，远端资产、签名与更新清单全部核验通过。** 本轮除发布外还修掉三处发布门禁自身的缺陷（安装产物定位、提权宿主下的应用启动、托管 runner 的渲染步骤归属）与一处用户可见缺陷（「最新」徽章跟随滞后的 npm dist-tag）；产品代码改动集中在 `src-tauri/src/versions/catalog.rs`（徽章语义 + 3 条单测）。真机项（干净环境安装/卸载、应用内一键更新点击）仍按手测清单开放。

## 过程

1. 发布准备：`chore(release): prepare v0.1.0-alpha.5`（`63d3288`）——五处版本一致（package.json · package-lock.json ×2 · tauri.conf.json · Cargo.toml · Cargo.lock）+ 发布说明重写。
2. 首轮 tag 触发的 Release（run `34619309533`）被安装器冒烟门禁拦下：`I-1` 报「installed PHL.exe not found」。根因是门禁自身：它假定产物叫 `PHL.exe`，且直接拼接未去引号的注册表值；NSIS 模板实际安装的是 cargo 二进制 `dsh-phl.exe`，`InstallLocation`/`DisplayIcon` 又写成带引号形式。修复 `9667f6c`（按注册表 IL/DI 去引号定位，同时接受两种文件名，DisplayIcon 目录兜底）。
3. 第二轮（run `34625174606`）：定位已过，`I-3b` 超时——取证显示应用起来了、窗口闪现，但 DevTools 端口始终不出现。对照实验定性：**普通权限宿主 ~2 秒内端口可用；提权宿主（管理员控制台，含 `--no-sandbox`）永不出现**。修复 `7fe99ff`：提权时改由 `Interactive + Limited` 计划任务的 `.bat` broker 启动应用（并把 `PHL_ROOT` 带过任务边界），CDP 超时的 FAIL detail 附 `[scene: app=… wv=… bind=…]` 取证。
4. 第三轮（run `34676629084`）：托管 runner 上取证为 `app=True wv=5 bind=0`——应用与 5 个 WebView2 子进程都在，端口仍不绑定。据此把渲染三步（I-3b 页目标 · I-4 真实 IPC · I-5 console 零异常）以 `--no-gui` **显式 SKIP** 并注明移交真机车道（`15b2eec`），其余 8 步继续阻塞；真机车道用同一脚本、不加 `--no-gui`，**11/11 通过**，报告入库 `docs/alpha5-installer-smoke-manual-2026-09-12.json`（`e3ebf41`）。
5. 用户可见缺陷修复 `46a6fc1`：「版本」页「最新」徽章此前跟随 npm `dist-tags.latest`（上游把它停在 rc.1，rc.2 走 `next`，于是 rc.2 已发布而徽章仍在 rc.1），现改为跟随目录中版本号最高的一版；实机截图核对 `0.1.5-rc.2 | 最新:true`，徽章、列表排序与创建向导默认选中三者口径一致。
6. 最终 tag：`v0.1.0-alpha.5` → `c15fa82`（该提交 CI run `34677702589` 全绿，约 10.6 分钟）。Release run `34678169041` 成功（22.4 分钟）。
7. Release：<https://github.com/A7m0spHere/dsh-phl/releases/tag/v0.1.0-alpha.5>，prerelease=true，附 4 个资产（安装包 / `.sig` / `.sha256` / `latest.json`）。

## 证据矩阵

| 门禁 | 结果 | 证据与边界 |
|---|---|---|
| 发布源码可追溯 | PASS | tag `v0.1.0-alpha.5` 指向 `c15fa82`；该提交 CI run `34677702589` 三 job 全绿（Windows bundle 按策略 skip）。 |
| 安装包哈希 | PASS | 本机从 Release URL 下载 `PHL_0.1.0-alpha.5_x64-setup.exe`：3,132,565 字节，SHA-256 `B205ED0709D5F65F3FEC751A48C6863196F114AA4BF60D57908289FCD06C4C16`，与 `.sha256` sidecar、GitHub 资产 digest 三方一致。 |
| minisign 签名 | PASS | `.sig` 为 base64 包裹的 minisign 文本（untrusted comment + ED 预哈希签名 + trusted comment + 全局签名）。解码后 keyid `aef74d979fa34850` 与 `tauri.conf.json` 公钥一致；以 Ed25519 对 BLAKE2b-512(安装包) 验签 **PASS**。 |
| 公钥延续性 | PASS | alpha.3 → alpha.5 公钥未变（keyid `AEF74D979FA34850`），alpha.4 安装的应用内更新器会接受本版本。 |
| 更新清单发布 | PASS | Release 的 `latest.json` 与 `updates` 分支清单版本/URL/签名一致；唯一端点 raw.githubusercontent 在线返回 `0.1.0-alpha.5`。 |
| 流水线门禁 | PASS | run `34678169041`：版本一致校验、`npm test`、`npm run build`、`cargo test --workspace`、NSIS + updater 签名、无头核心门禁、安装器冒烟（宿主 8 步 PASS + 渲染 3 步显式 SKIP，exitCode 0）、创建 Release、推送清单全部成功；`Configure Windows signing` 按策略 skip（未做 Authenticode）。 |
| 真机渲染车道 | PASS | `docs/alpha5-installer-smoke-manual-2026-09-12.json`：11/11（安装 → 版本匹配 → 首启页目标 → 真实 IPC 往返 → console 零异常 → 清单契约 → 覆盖升级 → 卸载 → 用户数据保留），exitCode 0。 |

## 门禁自身的修复（本轮副产品，详见 docs/windows-e2e-release-gate.md）

| 缺陷 | 症状 | 修复 |
|---|---|---|
| 安装产物定位 | I-1 永远找不到装出来的 exe | 读注册表 IL/DI（去引号）+ 接受 `PHL.exe`/`dsh-phl.exe` + DisplayIcon 目录兜底（`9667f6c`） |
| 提权宿主不开 DevTools | 应用起来、窗口闪现，端口不监听 | `Interactive+Limited` 计划任务 + `.bat` broker 启动应用，`PHL_ROOT` 随行（`7fe99ff`） |
| 托管 runner 无法渲染核验 | `app=True wv=5 bind=0`，接入后仍无端口 | `--no-gui` 把 I-3b/I-4/I-5 显式 SKIP 并注明移交真机车道（`15b2eec`） |
| 徽章跟随滞后 dist-tag | rc.2 已发布、徽章仍在 rc.1 | 徽章 = 目录最高版本，与向导默认选中同源（`46a6fc1`） |

## 仍开放的实机项

- 干净 Windows 用户环境的 alpha.5 安装包 安装 → 首启 → 卸载（手测清单 §15 ⭐ 项）。
- alpha.4 → alpha.5 应用内「设置 → 关于」一键更新的真机点击确认（协议链已在离线证据矩阵中验证）。
- Authenticode 代码签名按策略未启用，首次下载与运行的 SmartScreen 提示已在安装说明中声明。

## 收尾

- tag 之后未再改动产品代码；`updates` 分支、Release 资产与本地 tag 三者一致。
- 本机为核验下载的安装包、签名文件与临时校验脚本已从 `%TEMP%` 清理。
