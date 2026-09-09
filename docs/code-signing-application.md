# SignPath Foundation 申请表 · 逐字段填写

表单地址 <https://signpath.org/apply.html>（2026-09-09 实测字段，HubSpot 嵌入）。
带 \* 的为必填。下面每格都能直接复制。

## 字段表

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

## 长文本（可直接复制）

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

## 提交后

1. SignPath 会先做资格与声誉评审（他们会在仓库里核对构建来源、许可证与政策）；
2. 通过后按 [code-signing.md](./code-signing.md) 的「通过之后」一节接入。
