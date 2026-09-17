<p align="center">
  <img src="./assets/readme/hero.png" width="100%" alt="PHL — DeepSeek Harness 实例与运行时管理器。一个桌面，各自的 DSH 环境：日常使用、插件测试、版本尝鲜。">
</p>

<p align="center">
  <strong>Windows 10 / 11 · x64</strong> &nbsp; / &nbsp; Alpha 预览 &nbsp; / &nbsp; MIT 开源
</p>

<p align="center">
  <a href="https://github.com/A7m0spHere/dsh-phl/releases"><strong>下载安装 →</strong></a> &nbsp; · &nbsp;
  <a href="#首次运行">首次运行</a> &nbsp; · &nbsp;
  <a href="#从源码构建">源码构建</a> &nbsp; · &nbsp;
  <a href="#文档与反馈">文档与反馈</a>
</p>

# PHL

**在同一台机器上管理多个 DeepSeek Harness（DSH）环境。** 为日常使用、插件测试和版本尝鲜分别创建实例，各自选择 DSH 版本与 Node 运行时，分别保存配置、插件和工作目录。

PHL 是基于 Tauri 2 + Rust 的桌面应用。版本下载、实例管理、磁盘读写与进程启动均已接入真实后端；项目独立开发，与 DeepSeek 官方无隶属关系。

<p align="center">
  <img src="./assets/readme/app.png" width="100%" alt="PHL 桌面端版本页实拍：左侧按安装状态筛选，右侧展示 DSH 版本与更新说明，顶部可进入实例、插件、API 和运行时管理。">
</p>

*桌面端「版本」页实拍。多个版本可同时安装，由实例分别选择。*

## 一个实例，一套环境

| 使用场景 | 在 PHL 中怎么做 |
|---|---|
| **日常与测试分开** | 创建或克隆实例，分别绑定 DSH 版本、Node 运行时和插件 |
| **管理模型与 API** | 维护全局供应商与模型配置；实例可继承全局、自选供应商，或不托管 |
| **下载与启动** | DSH / Node 下载支持校验、进度与取消；启动时检测端口，遇到问题查看日志 |
| **修改前备份** | 保存实例快照，需要时回滚；也可通过 Bundle 导出配置清单 |
| **接入与迁移** | 发现本机 DSH、复制对话，通过 `.phlpack` 导入或导出整合包 |
| **日常维护** | 查看磁盘占用、清理缓存与孤立目录，诊断问题或迁移数据目录 |

## 下载与安装

目前提供 **Windows 10/11 x64 的 alpha 预览版**，界面与数据格式仍可能调整。macOS / Linux 尚未完成真机验收。

1. 打开 **[Releases 下载页](https://github.com/A7m0spHere/dsh-phl/releases)**，在所选预览版的 **Assets** 中下载 `PHL_<version>_x64-setup.exe` 和同名 `.sha256` 文件。
2. 在下载目录执行下方命令，与 `.sha256` 中的值核对一致后运行安装程序。
3. 从开始菜单打开 **PHL**，创建第一个实例。

```powershell
Get-FileHash .\PHL_*_x64-setup.exe -Algorithm SHA256
```

安装包尚未做 Windows 代码签名，首次下载或运行可能出现 SmartScreen 提示，处理方法见[安装说明](./docs/install-note.md)。桌面窗口依赖 WebView2，系统缺失时安装器会联网安装。

已有 alpha.3 或更新版本，可在「**设置 → 关于**」检查更新、下载并安装；更早版本需手动安装一次，详见[应用内更新](./docs/auto-update.md)。

## 首次运行

1. **下载 DSH**：在「版本」页选择需要的版本。
2. **选择 Node**：在「运行时」页下载运行时，或使用检测到的系统 Node。
3. **创建实例**：填写名称，选择 DSH 版本、运行时与端口。
4. **配置 API**：需要由 PHL 管理模型时，在「API」页配置供应商与模型，再在实例中选择配置来源并同步。
5. **启动使用**：在实例页点击「启动」，等待就绪后打开 WebUI。需要插件时，在「插件」页选定目标实例后安装。

已经有本机 DSH？从[本机发现与接入](./docs/p0-local-dsh-adoption.md)开始，按向导选择接入方式和需要迁移的对话。

常用快捷键：`Ctrl+K` 快速跳转 · `Ctrl+,` 打开设置 · `Esc` 返回。完整清单见「设置 → 快捷键」。

## 隔离如何工作

**共享程序文件，分别保存实例状态。** DSH 版本与 Node 运行时集中存放，实例按需引用；每个受管实例拥有自己的 `DSH_HOME`、配置、插件、工作目录和端口。

<p align="center">
  <img src="./assets/readme/how-it-works.svg" width="100%" alt="受管实例隔离示意：共享的 DSH 版本库和 Node 运行时库供实例按需引用。日常实例与测试实例各自保存 DSH_HOME、配置、插件和工作目录，通过不同端口打开各自的 WebUI。">
</p>

这种隔离指环境与数据目录分离，不是操作系统级安全沙箱。启动时，PHL 注入对应实例的 `DSH_HOME`，用所选 Node 执行 `dsh web --port <端口> --no-open`，固定使用 `web` profile。

<details>
<summary>查看数据目录与备份范围</summary>

```text
<数据目录>/
├── versions/                   DSH 版本文件
├── runtimes/                   Node 运行时文件
└── instances/<实例 ID>/
    ├── instance.json           版本、运行时与端口等配置
    ├── dsh-home/               实例自己的 DSH_HOME
    │   └── profiles/web/       插件与 profile 配置
    ├── workspace/              工作目录
    └── logs/                   启动日志
```

- **快照**：保存实例的 `dsh-home`，不包含 workspace、日志或版本绑定。
- **Bundle**：JSON 配置清单，记录实例配置与插件信息；不含插件文件、凭据值和机器本地环境变量，导入后需重新安装插件并配置凭据。
- **`.phlpack`**：整合包，可按导出选项携带插件文件与对话；具体内容和迁移范围见[对话与环境迁移](./docs/p1-session-pack.md)。

</details>

## 从源码构建

前端使用 **React 18 · TypeScript · Vite 6**，桌面端使用 **Tauri 2 · Rust**。

Windows 开发需准备 **Node.js 22（与 CI 一致）、Rust stable / MSVC、Visual Studio C++ Build Tools 的「使用 C++ 的桌面开发」工作负载，以及 WebView2**。

```bash
git clone https://github.com/A7m0spHere/dsh-phl.git
cd dsh-phl
npm ci
npm run app:dev
```

也可双击 [`run-dev.cmd`](./run-dev.cmd) 检查环境并启动；首次编译 Rust 需要一些时间。

| 命令 | 用途 |
|---|---|
| `npm run dev` | 浏览器 UI 预览，使用 Mock 数据 |
| `npm run app:dev` | 启动桌面应用，连接真实 Rust 后端 |
| `npm run app:build` | 构建 Windows 应用与 NSIS 安装包 |
| `npm run icon` | 从仓库内母版生成应用图标 |

安装包输出到 `src-tauri/target/release/bundle/nsis/`，也可双击 [`build-app.cmd`](./build-app.cmd) 构建并打开产物目录。浏览器预览不执行真实桌面操作。

<details>
<summary>开发检查与代码导航</summary>

```bash
npm run typecheck
npm run check:size
npm run bridge:check
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
```

| 目录 | 内容 |
|---|---|
| [`src/pages/`](./src/pages/) / [`src/components/`](./src/components/) | 页面与 UI 组件 |
| [`src/services/`](./src/services/) / [`src/stores/`](./src/stores/) | Repository、桌面桥接与共享状态 |
| [`src/types/`](./src/types/) / [`src/data/`](./src/data/) | 领域模型与浏览器 Mock 数据 |
| [`src-tauri/src/`](./src-tauri/src/) | 下载、实例、插件、运行时、进程与磁盘操作 |
| [`src-tauri/crates/`](./src-tauri/crates/) | 共用 Pack Core 与 `phl-pack` CLI |

页面通过 Repository 接口访问数据，入口见 [`src/services/index.ts`](./src/services/index.ts)。开发约定以 [`AGENTS.md`](./AGENTS.md) 为准，完整检查流程见 [CI 配置](./.github/workflows/ci.yml)。

</details>

## 文档与反馈

| 你想了解 | 对应文档 |
|---|---|
| 安装、版本与更新 | [安装说明](./docs/install-note.md) · [发布记录](https://github.com/A7m0spHere/dsh-phl/releases) · [应用内更新](./docs/auto-update.md) |
| 接入已有环境、迁移对话 | [本机 DSH 接入](./docs/p0-local-dsh-adoption.md) · [对话与环境迁移](./docs/p1-session-pack.md) |
| 集成整合包能力 | [Pack Core、CLI 与导出 Skill](./docs/p2-pack-core-skill.md) |
| 安全与数据处理 | [安全说明](./SECURITY.md) · [隐私说明](./docs/privacy.md) |

遇到问题或有建议，请[提交 Issue](https://github.com/A7m0spHere/dsh-phl/issues)，附上 PHL 版本、复现步骤与相关日志（移除凭据后）。

## 参考与许可

PHL 的产品模型与交互参考了 PCL、Prism Launcher、DSHBox。实现遵守[代码来源边界](./AGENTS.md#代码来源边界必须遵守)，不复制、移植或机械改写其他 Launcher 的代码，不使用其专有视觉资产。鲸鱼尾鳍标识来自仓库内的[独立母版](./src-tauri/icons/master/PROVENANCE.md)。

[MIT](./LICENSE) © 2026 A7m0spHere · [第三方许可](./THIRD_PARTY_NOTICES.md)
