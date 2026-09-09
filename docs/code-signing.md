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

CI 侧（`.github/workflows/release.yml`）在拿到证书后增加一步签名即可；在证书就位前不要开启，
否则构建会因找不到证书而失败。
