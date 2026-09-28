# 代码签名（Code Signing）· 现状、政策与申请

> 2026-09-09 初版三篇（现状方案 / 政策 / 申请表）于 2026-09-27 合并为一篇，
> 内容不变、按「现状 → 候选路径 → 工程接入 → 政策 → SignPath 申请材料」组织。
> 本文件同时是申请 [SignPath Foundation](https://signpath.org/) 免费开源签名的
> 政策材料之一。

## 1. 现状

`v0.1.0-alpha.1` 起的安装包**未签名**。未签名的 Windows 可执行文件会触发：

1. 浏览器下载拦截（「通常不会下载 …」）；
2. 首次运行时的 SmartScreen「Windows 已保护你的电脑」。

两者都源于**没有 Authenticode 签名 + 没有下载信誉**，与文件内容无关。自签名证书对公众用户无用
（会因为不受信任的根证书而直接安装失败），只适合内部测试。

## 2. 方案对比（据 Microsoft Learn《Code signing options for Windows app developers》，2026）

| 方案 | 成本 | 可用地区 | SmartScreen 表现 | 上架商店 |
|---|---|---|---|---|
| Microsoft Store（MSIX） | 免费 | 全球 | ✅ 无提示（商店重新签名） | ✅ |
| Azure Artifact Signing（原 Trusted Signing） | ~$9.99/月 | 组织：美/加/欧盟/英；个人：仅美/加 | ⚠️ 信誉需时间积累，初期仍有提示 | ❌ |
| OV 证书（DigiCert、Sectigo 等） | $150–300/年 | 全球 | ⚠️ 同上 | ❌ |
| EV 证书 | $400+/年 | 全球 | ⚠️ 自 2024 起**不再即时绕过** SmartScreen | ❌ |
| 自签名 | 免费 | — | ❌ 公众用户直接安装失败 | ❌ |
| 不签名（当前） | 免费 | — | ❌ 强提示 | ❌ |

**结论**：签名是必要条件，但没有「花钱立刻消失」的选项——除商店分发外，信誉都要慢慢积累。

PHL 的候选路径按优先级：

1. **SignPath Foundation（开源免费签名）** —— 最贴合当前项目：仓库公开、MIT、活跃维护、
   有可复现的 GitHub 构建。申请需要：每个成员启用 MFA、代码签名政策文档（本文 §4）、许可证审计通过
   （`npm run test:alpha` 已覆盖版本/构建/测试，许可证清单见 `THIRD_PARTY_NOTICES.md`）、项目声誉评审。
   通过后由 SignPath 提供证书与 CI 集成。
2. **Microsoft Store（MSIX）** —— 免费且无提示，但分发形态从「直接下载安装包」变成「商店安装」。
3. **OV 证书** —— 全球可买，价格适中；签上后仍需要一段时间的信誉积累。
4. **Azure Artifact Signing** —— 便宜，但个人仅限美国/加拿大，当前不适合。

## 3. 工程接入（Tauri 2，已接好、默认关闭）

`src-tauri/tauri.conf.json` → `bundle.windows.signCommand` 指向 `scripts/sign-windows.ps1`，
Tauri 会把每个产出的二进制（应用 exe、NSIS 安装程序）以 `%1` 传给它（也支持
`certificateThumbprint` + `timestampUrl` + `digestAlgorithm`：证书已装进 Windows 证书存储时使用）。

- `scripts/sign-windows.ps1` 从环境变量读取凭据（`PHL_SIGN_PFX_BASE64` / `PHL_SIGN_PFX_PASSWORD`，
  或 `PHL_SIGN_PFX_PATH`，或 `PHL_SIGN_THUMBPRINT`），用 `signtool` 签名并复核；
  **没有凭据时只打印一行说明并退出 0**，所以本地未签名的构建照常可用；
- `.github/workflows/release.yml` 的「Configure Windows signing」步骤由仓库变量
  `WINDOWS_SIGNING=true` 控制：开启后把 `WINDOWS_CERTIFICATE`（base64 PFX）与
  `WINDOWS_CERTIFICATE_PASSWORD` 注入 runner，并设置 `PHL_SIGN_REQUIRED=1`——
  此时证书缺失会让发布**失败**，而不是静默发出未签名包。

### 两个已经踩过的坑（改配置前必读）

- **脚本路径要写 `../scripts/sign-windows.ps1`**：Tauri 打包器执行签名命令时的工作目录是
  `src-tauri`，而它只把**在当前目录下确实存在**的相对参数转成绝对路径。写成 `scripts/...`
  不会被转换，pwsh 找不到脚本并以非 0 退出，构建报 `failed to run pwsh`。
- **不要用 `-Command`**：`%1` 会被追加到命令字符串尾部（`... exit 0 D:\...\dsh-phl.exe`），
  PowerShell 直接 ParserError。必须用 `-File`，`%1` 才是独立参数。

### 拿到证书后要做的三步

1. 把 PFX 转成 base64 存进仓库 Secrets：`WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`；
2. 新建仓库变量 `WINDOWS_SIGNING` = `true`；
3. 打下一个版本标签。流水线会签名并用 `signtool verify /pa` 复核后才创建 Release。

本地想试签：设置同样的环境变量后跑 `npm run app:build`。

## 4. 代码签名政策 / Code Signing Policy

（申请 SignPath Foundation 的政策材料；发布只能由公开仓库的标签触发，不能由本地手工上传。）

### 4.1 项目与产物

- 项目：PHL（`dsh-phl`）—— DeepSeek Harness 的实例与运行时管理器，Tauri 2 桌面应用。
- 仓库：<https://github.com/A7m0spHere/dsh-phl>（公开）
- 许可证：[MIT](../LICENSE)，无商业双许可。
- 需要签名的产物：`PHL_<version>_x64-setup.exe`（NSIS 安装程序）及其内嵌的 `PHL.exe`。

### 4.2 构建与签名的可验证性

1. 维护者推 `v*` 标签（例如 `v0.1.0-alpha.1`）；
2. `.github/workflows/release.yml` 在 GitHub 托管的 `windows-latest` runner 上执行：
   版本一致性校验 → `npm test` → `npm run build` → `cargo test --workspace` → bundle 输入检查 →
   `npm run app:build` → 产物哈希 → 创建 GitHub Release；
3. 若仓库变量 `WINDOWS_SIGNING=true`，构建前会把证书注入 runner 环境，由
   `scripts/sign-windows.ps1`（经 `tauri.conf.json` 的 `bundle.windows.signCommand`）对二进制签名，
   并用 `signtool verify /pa` 复核；未配置证书时该脚本会显式失败（`PHL_SIGN_REQUIRED=1`），
   不会静默发布未签名产物。

因此：**发布的二进制可以从公开的标签与工作流日志追溯到源代码**。

### 4.3 私钥与凭据处理

- 采用 SignPath 时：**私钥由 SignPath 的 HSM 生成与保管，PHL 不接触私钥**；签名通过 CI 请求完成。
- 若改用自购证书：PFX 只以 base64 形式存放于 GitHub Actions Secrets（`WINDOWS_CERTIFICATE` /
  `WINDOWS_CERTIFICATE_PASSWORD`），签名脚本在临时目录解码并在结束时删除，仓库内不存在任何私钥材料。
- 仓库不包含、也不接受任何私钥、口令或证书文件。

### 4.4 用户如何校验

```powershell
# 1) 校验下载完整性（Release 附带同名 .sha256）
Get-FileHash .\PHL_0.1.0-alpha.1_x64-setup.exe -Algorithm SHA256

# 2) 校验 Authenticode 签名（签名启用后）
Get-AuthenticodeSignature .\PHL_0.1.0-alpha.1_x64-setup.exe | Format-List Status, SignerCertificate
```

`Status` 应为 `Valid`；签名者主体应与 Release 说明中公布的主体一致。

### 4.5 对照 SignPath Foundation 的资格条件

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

### 4.6 申请信息（通过后填写）

| 字段 | 值 |
|---|---|
| organization id | 待填 |
| project slug | `dsh-phl` |
| signing policy slug | 待填 |
| artifact configuration slug | 待填 |
| CI 集成 | `signpath/github-action-submit-signing-request`（或 `signCommand` 指向 SignPath CLI） |

## 5. SignPath Foundation 申请表（逐字段）

表单地址 <https://signpath.org/apply.html>（2026-09-09 实测字段，HubSpot 嵌入）。
带 \* 的为必填。下面每格都能直接复制。

### 5.1 字段表

| 字段 | 必填 | 填什么 |
|---|---|---|
| Project Name | \* | `PHL` |
| Repository URL | \* | `https://github.com/A7m0spHere/dsh-phl` |
| Homepage URL | \* | `https://github.com/A7m0spHere/dsh-phl` |
| Download URL | | `https://github.com/A7m0spHere/dsh-phl/releases` |
| Privacy Policy URL | | `https://github.com/A7m0spHere/dsh-phl/blob/main/docs/privacy.md` |
| Wikipedia URL (optional) | | 留空 |
| Tagline | \* | 见下方「长文本」 |
| Description | \* | 见下方「长文本」 |
| Reputation | \* | 见下方「长文本」 |
| Maintainer Type | | `Individual maintainer(s)` |
| Build System | | `GitHub Actions` |
| First Name | \* | 你的名 |
| Last Name | \* | 你的姓 |
| Email | \* | 你的邮箱（SignPath 用它联系你） |
| Company Name | | 留空 |
| Primary Discovery Channel | \* | 按实际选：`Organic search` / `Developer platforms (e.g. GitHub)` / `AI / LLM tools` / `Other (please specify)` 等 |
| Please specify the exact source | | 可留空 |
| Code of Conduct 同意项 | \* | **勾选** |
| 接收 SignPath 通讯 | | 可不勾 |
| 同意数据存储与处理 | \* | **勾选** |
| reCAPTCHA | \* | 通过验证 |

### 5.2 长文本（可直接复制）

**Tagline**

```text
Run multiple isolated DeepSeek Harness instances side by side on Windows.
```

**Description**

```text
PHL is a Windows desktop application (Tauri 2: Rust backend, React/TypeScript frontend) that
manages several isolated DeepSeek Harness instances and runtimes on one machine. Each instance
pins its own DSH version, runtime and plugins, and can run at the same time as the others
without touching their files. It ships as an NSIS installer (PHL_<version>_x64-setup.exe)
published on GitHub Releases.
```

**Reputation**

```text
PHL is a new project: the first public preview (v0.1.0-alpha.1) was published on 2026-09-09, so
there is no download history or user community to point at yet. What is verifiable from the
repository today:

- public, MIT-licensed code base with continuous history;
- every release is tag-triggered and rebuilt on GitHub-hosted runners, so each published binary
  can be traced to a workflow log (nothing is uploaded by hand);
- an alpha gate on every commit: version consistency across four files, TypeScript typecheck,
  92 frontend tests, rustfmt, clippy with -D warnings, 352 Rust workspace tests, and an
  IPC-bridge check;
- a dependency inventory in THIRD_PARTY_NOTICES.md, with cargo audit reporting zero
  vulnerabilities across 513 locked crates;
- asset provenance recorded in src-tauri/icons/master/PROVENANCE.md.

PHL is an unofficial companion for DeepSeek Harness; that boundary is stated in the README and
inside the application.
```

### 5.3 提交后

1. SignPath 会先做资格与声誉评审（他们会在仓库里核对构建来源、许可证与政策）；
2. 通过后按 §3「拿到证书后要做的三步」接入。

## 变更记录

- 2026-09-09 初版（三篇）：`v0.1.0-alpha.1` 发布后发现未签名安装包会触发浏览器与 SmartScreen 提示，
  建立政策并接入可选签名步骤。
- 2026-09-27 三篇合并为本文件，内容未变。
