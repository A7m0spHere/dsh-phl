# Alpha 发行验收 · 2026-09-11（v0.1.0-alpha.4 发布与远端核验）

## 判定

**已发布，远端资产与更新清单全部核验通过。** 本轮不改产品代码：接续上一会话（因使用上限中断于流水线等待中）完成发布跟踪与证据核验，并在此记账。真机项（干净环境安装/卸载、应用内一键更新点击）仍按手测清单开放。

## 过程

- 发布准备提交（公开树 `65a469f` / 本地 `72b1a24`）的首轮 main CI Rust job 失败：GitHub runner 的 Rust stable 已是 1.98，新增 `some_filter` lint 把一处等价写法在 `-D warnings` 下判为错误（run `34549713203`）。本机 1.95 未触发，属版本兼容差异而非产品缺陷。
- 兼容修复为单行 `then_some` 改写（公开 `6ba04c7` = 本地 `1f38ff6`），main 第二轮 CI 全绿：[run `34550242283`](https://github.com/A7m0spHere/dsh-phl/actions/runs/34550242283)（Frontend 与 Rust 两 job 均成功）。
- 随即创建 annotated tag `v0.1.0-alpha.4`（指向公开 main 顶端 `6ba04c7`），触发 Release 流水线：[run `34550553053`](https://github.com/A7m0spHere/dsh-phl/actions/runs/34550553053) 于 2026-09-11T01:40:45Z 成功，全程约 16 分钟（版本校验 → 前端测试/构建 → `cargo test --workspace` → NSIS + updater 签名 → Release → 清单推送全部通过）。
- Release：<https://github.com/A7m0spHere/dsh-phl/releases/tag/v0.1.0-alpha.4>，prerelease=true，正文为发布说明 + 安装校验指引 + 自动 changelog，附 4 个资产（安装包 / `.sig` / `.sha256` / `latest.json`）。

## 证据矩阵

| 门禁 | 结果 | 证据与边界 |
|---|---|---|
| 发布源码可追溯 | PASS | tag tree = 本地 HEAD 树仅排除 15 个本地文档文件（既定公开规则），产品代码零差异；tag 即 CI 全绿的 `6ba04c7`。 |
| 安装包哈希 | PASS | 本机从 Release URL 下载 `PHL_0.1.0-alpha.4_x64-setup.exe`：3,130,382 字节，SHA-256 `2CFF19816DAB8803FA6D239B92B81A4DA935D3E769E769817F11377F44DD79D1`，与 `.sha256` sidecar、GitHub 资产 digest 三方一致。 |
| minisign 签名 | PASS | `.sig` 资产与 `latest.json` 的 `signature` 字段逐字节一致（单行 base64 的 minisign 块）；解码后 keyid `5048a39f974df7ae` 与 `tauri.conf.json` 公钥 `AEF74D979FA34850` 一致；对下载文件做 BLAKE2b-512 + Ed25519 验签通过——即 updater 的完整校验链在真实资产上成立。 |
| 公钥延续性 | PASS | alpha.3 → alpha.4 配置公钥未变，alpha.3 安装的应用内更新器会接受本版本（清单版本 `0.1.0-alpha.4` > `0.1.0-alpha.3`）。 |
| 更新清单发布 | PASS | Release 资产与 `updates` 分支的 `latest.json` 版本/URL/签名一致；updater 配置的唯一端点 <https://raw.githubusercontent.com/A7m0spHere/dsh-phl/updates/latest.json> 在线返回 0.1.0-alpha.4。 |
| 流水线门禁 | PASS | run `34550553053` 内版本四处一致校验、前端测试/构建、`cargo test --workspace`、NSIS 构建与 `Create GitHub Release`、`Publish the updater manifest` 均成功；无跳过（`Configure Windows signing` 仍按策略 skip，Authenticode 未签，SmartScreen 提示已在安装说明中声明）。 |

## 仍开放的实机项

- 干净 Windows 环境的 alpha.4 安装包 安装 → 首启 → 卸载 验收（手测清单第 15 节 ⭐ 项）。
- alpha.3 → alpha.4 应用内「设置 → 关于」一键更新的真机点击确认（协议链已在上表离线验证）。

## 收尾

本轮无代码、配置或远端资产改动；未重新触发任何流水线，tag / Release / updates 分支保持上一会话产物原样。核验中在 `%TEMP%` 下载的安装包与临时脚本已清理。
