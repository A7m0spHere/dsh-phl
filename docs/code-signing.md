# 代码签名（Code signing）现状与方案 · 2026-09-09

## 现状

`v0.1.0-alpha.1` 的安装包**未签名**。未签名的 Windows 可执行文件会触发：

1. 浏览器下载拦截（「通常不会下载 …」）；
2. 首次运行时的 SmartScreen「Windows 已保护你的电脑」。

两者都源于**没有 Authenticode 签名 + 没有下载信誉**，与文件内容无关。自签名证书对公众用户无用
（会因为不受信任的根证书而直接安装失败），只适合内部测试。

## 方案对比（据 Microsoft Learn《Code signing options for Windows app developers》，2026）

| 方案 | 成本 | 可用地区 | SmartScreen 表现 | 上架商店 |
|---|---|---|---|---|
| Microsoft Store（MSIX） | 免费 | 全球 | ✅ 无提示（商店重新签名） | ✅ |
| Azure Artifact Signing（原 Trusted Signing） | ~$9.99/月 | 组织：美/加/欧盟/英；个人：仅美/加 | ⚠️ 信誉需时间积累，初期仍有提示 | ❌ |
| OV 证书（DigiCert、Sectigo 等） | $150–300/年 | 全球 | ⚠️ 同上 | ❌ |
| EV 证书 | $400+/年 | 全球 | ⚠️ 自 2024 起**不再即时绕过** SmartScreen | ❌ |
| 自签名 | 免费 | — | ❌ 公众用户直接安装失败 | ❌ |
| 不签名（当前） | 免费 | — | ❌ 强提示 | ❌ |

**结论**：签名是必要条件，但没有「花钱立刻消失」的选项——除商店分发外，信誉都要慢慢积累。

## PHL 的候选路径

1. **SignPath Foundation（开源免费签名）** —— 最贴合当前项目：仓库公开、MIT、活跃维护、
   有可复现的 GitHub 构建。申请需要：每个成员启用 MFA、代码签名政策文档、许可证审计通过
   （`npm run test:alpha` 已覆盖版本/构建/测试，许可证清单见 `THIRD_PARTY_NOTICES.md`）、项目声誉评审。
   通过后由 SignPath 提供证书与 CI 集成。
2. **Microsoft Store（MSIX）** —— 免费且无提示，但分发形态从「直接下载安装包」变成「商店安装」。
3. **OV 证书** —— 全球可买，价格适中；签上后仍需要一段时间的信誉积累。
4. **Azure Artifact Signing** —— 便宜，但个人仅限美国/加拿大，当前不适合。

## 工程接入点（Tauri 2）

`src-tauri/tauri.conf.json` 的 `bundle.windows` 支持：

- `certificateThumbprint` + `timestampUrl` + `digestAlgorithm`：证书已装进 Windows 证书存储时使用；
- `signCommand`：交给外部签名器（SignPath、`signtool` 包装脚本等）签名。

## 已经接好的部分（默认关闭）

- `src-tauri/tauri.conf.json` → `bundle.windows.signCommand` 指向 `scripts/sign-windows.ps1`，
  Tauri 会把每个产出的二进制（应用 exe、NSIS 安装程序）以 `%1` 传给它；
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

申请 SignPath Foundation 的材料见 [code-signing-policy.md](./code-signing-policy.md)。
