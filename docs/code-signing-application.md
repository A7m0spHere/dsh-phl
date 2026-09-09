# SignPath Foundation 申请材料 / Application material

用途：在 <https://signpath.org/apply.html> 的表单里逐项填写。表单是 HubSpot 嵌入的，字段以页面为准；
下面是各字段可**直接复制**的英文内容。政策正文见 [code-signing-policy.md](./code-signing-policy.md)。

## 复制区（英文）

**Project name**

```text
PHL
```

**Repository URL**

```text
https://github.com/A7m0spHere/dsh-phl
```

**License**

```text
MIT (OSI-approved, no commercial dual licensing)
```

**Project description**

```text
PHL is a Windows desktop application (Tauri 2: Rust backend, React/TypeScript frontend)
that manages multiple isolated DeepSeek Harness instances and runtimes on one machine.
Every instance pins its own DSH version, runtime and plugins, and can run at the same time
as the others without touching their files. It is distributed as an NSIS installer
(PHL_<version>_x64-setup.exe) published on GitHub Releases.
```

**What you want to sign**

```text
PHL_<version>_x64-setup.exe (NSIS installer, ~3 MB) and the PHL.exe it contains.
```

**Build system / CI**

```text
GitHub Actions on GitHub-hosted windows-latest runners. Releases are tag-triggered:
pushing a v* tag runs .github/workflows/release.yml, which verifies the version, runs
npm test, npm run build, cargo test --workspace, checks the bundle inputs, builds the
installer with tauri build, hashes it, and creates the GitHub Release. Nothing is uploaded
by hand.
```

**Code signing policy URL**

```text
https://github.com/A7m0spHere/dsh-phl/blob/main/docs/code-signing-policy.md
```

**Why code signing is needed**

```text
Without an Authenticode signature the installer triggers the browser's "usually not
downloaded" warning and Windows SmartScreen blocks the first run, which stops most users
from installing an open source tool. A SignPath Foundation certificate would also give users
a verifiable link between the published binary and this public repository.
```

**Malware-free / no proprietary components (short answers)**

```text
The application has no telemetry and sends no user data anywhere. It manages local
processes and files only. All dependencies are open source (see THIRD_PARTY_NOTICES.md)
and the project bundles no proprietary components. The application mark and all assets
are produced in this repository (see src-tauri/icons/master/PROVENANCE.md).
```

## 他们会核对什么

| 检查项 | PHL 现状 |
|---|---|
| 构建确实由 GitHub 工作流产生 | ✅ 标签触发 `release.yml`，无手工上传 |
| 来源可追溯 | ✅ 公开仓库 + 标签 + 工作流日志 |
| OSI 许可、无商业双许可 | ✅ MIT |
| 无恶意行为 | ✅ 无遥测、无回传 |
| 公开的代码签名政策 | ✅ 本目录的 policy 文件 |
| 成员 MFA | ⚠️ 需维护者在 GitHub 账号确认 |

## 通过之后（SignPath 侧）

1. 安装 SignPath GitHub App，并授权本仓库；
2. 在 SignPath 组织里新建 **Project**（关联本仓库）、**Signing Policy**（谁/什么条件下可签）、
   **Artifact Configuration**（要签哪些文件、哪些需要签名）；
3. 记下四个值，填进 GitHub：`SIGNPATH_API_TOKEN`（Secret）、`SIGNPATH_ORGANIZATION_ID`、
   `SIGNPATH_PROJECT_SLUG`、`SIGNPATH_SIGNING_POLICY_SLUG`、`SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`（Variables）；
4. 在 `release.yml` 里加一步 `signpath/github-action-submit-signing-request`（或让
   `scripts/sign-windows.ps1` 改调 SignPath 的 PowerShell cmdlet，签名范围同现有 signCommand）。
