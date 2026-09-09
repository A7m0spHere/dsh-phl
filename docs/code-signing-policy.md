# 代码签名政策 / Code Signing Policy

本文件是 PHL 的代码签名政策，也是申请 [SignPath Foundation](https://signpath.org/) 免费开源签名的材料之一。

## 1. 项目与产物

- 项目：PHL（`dsh-phl`）—— DeepSeek Harness 的实例与运行时管理器，Tauri 2 桌面应用。
- 仓库：<https://github.com/A7m0spHere/dsh-phl>（公开）
- 许可证：[MIT](../LICENSE)，无商业双许可。
- 需要签名的产物：`PHL_<version>_x64-setup.exe`（NSIS 安装程序）及其内嵌的 `PHL.exe`。

## 2. 构建与签名的可验证性

发布只能由**公开仓库的标签**触发，不能由本地手工上传：

1. 维护者推 `v*` 标签（例如 `v0.1.0-alpha.1`）；
2. `.github/workflows/release.yml` 在 GitHub 托管的 `windows-latest` runner 上执行：
   版本一致性校验 → `npm test` → `npm run build` → `cargo test --workspace` → bundle 输入检查 →
   `npm run app:build` → 产物哈希 → 创建 GitHub Release；
3. 若仓库变量 `WINDOWS_SIGNING=true`，构建前会把证书注入 runner 环境，由
   `scripts/sign-windows.ps1`（经 `tauri.conf.json` 的 `bundle.windows.signCommand`）对二进制签名，
   并用 `signtool verify /pa` 复核；未配置证书时该脚本会显式失败（`PHL_SIGN_REQUIRED=1`），
   不会静默发布未签名产物。

因此：**发布的二进制可以从公开的标签与工作流日志追溯到源代码**。

## 3. 私钥与凭据处理

- 采用 SignPath 时：**私钥由 SignPath 的 HSM 生成与保管，PHL 不接触私钥**；签名通过 CI 请求完成。
- 若改用自购证书：PFX 只以 base64 形式存放于 GitHub Actions Secrets（`WINDOWS_CERTIFICATE` /
  `WINDOWS_CERTIFICATE_PASSWORD`），签名脚本在临时目录解码并在结束时删除，仓库内不存在任何私钥材料。
- 仓库不包含、也不接受任何私钥、口令或证书文件。

## 4. 用户如何校验

```powershell
# 1) 校验下载完整性（Release 附带同名 .sha256）
Get-FileHash .\PHL_0.1.0-alpha.1_x64-setup.exe -Algorithm SHA256

# 2) 校验 Authenticode 签名（签名启用后）
Get-AuthenticodeSignature .\PHL_0.1.0-alpha.1_x64-setup.exe | Format-List Status, SignerCertificate
```

`Status` 应为 `Valid`；签名者主体应与 Release 说明中公布的主体一致。

## 5. 对照 SignPath Foundation 的资格条件

| # | 条件 | 现状 | 证据 / 待办 |
|---|---|---|---|
| 1 | 不含恶意或潜在有害行为 | ✅ | 仅本地事件处理、配置读写、进程启停与打包；无遥测、无网络回传用户数据 |
| 2 | 全部组件使用 OSI 认可的开源许可，无商业双许可 | ✅ | 项目 MIT；依赖清单与许可证见 `THIRD_PARTY_NOTICES.md` |
| 3 | 无专有代码或捆绑的专有组件 | ✅ | 应用标识与全部资源为本仓库独立产出（见 `src-tauri/icons/master/PROVENANCE.md`） |
| 4 | 项目在活跃维护 | ✅ | 2026-09 持续提交、发布 `v0.1.0-alpha.1` |
| 5 | 公开仓库 | ✅ | <https://github.com/A7m0spHere/dsh-phl> |
| 6 | 可从公开 CI 复现构建 | ✅ | `release.yml`（标签触发、GitHub 托管 runner） |
| 7 | 安装包带元数据 | ✅ | `tauri-build` 写入 VERSIONINFO（产品名、版本、发布者 `dsh-phl`） |
| 8 | 公开的代码签名政策 | ✅ | 本文件 |
| 9 | 所有成员启用多因素认证 | ⚠️ 待确认 | 需维护者在 GitHub 账号上确认并记录 |
| 10 | 项目声誉评审 | ⚠️ 由 SignPath 裁量 | 无法自证 |

## 6. 申请信息（通过后填写）

| 字段 | 值 |
|---|---|
| organization id | 待填 |
| project slug | `dsh-phl` |
| signing policy slug | 待填 |
| artifact configuration slug | 待填 |
| CI 集成 | `signpath/github-action-submit-signing-request`（或 `signCommand` 指向 SignPath CLI） |

## 7. 变更记录

- 2026-09-09 初版：`v0.1.0-alpha.1` 发布后发现未签名安装包会触发浏览器与 SmartScreen 提示，
  建立本政策并接入可选签名步骤。
