# dsh-phl / PHL

> **PHL — DSH Instance & Runtime Manager**
> 一个独立实现的、PCL 风格的 DeepSeek Harness 实例与运行时管理器。

桌面端的实例、版本、运行时、插件、API 配置与进程管理已经是**真实实现**（Rust 管线，真实读写磁盘与启停进程）。
浏览器模式（`npm run dev`）下一切数据来自 Mock Repository，方便纯 UI 开发。

| 模块 | 状态 |
|---|---|
| Version Manager | ✅ 真实：GitHub Releases + npm 目录，下载 / sha512 校验 / 解包 / 删除 |
| Plugin Manager | ✅ 真实：社区注册表，安装进实例 profile 的 `node_modules` 与 `cordis.patch.yml` |
| Instance Manager | ✅ 真实：磁盘上的 `instance.json`，重启不丢、删除即清理、插件列表由磁盘反推 |
| Runtime Manager | ✅ 真实：nodejs.org dist 目录（支持 npmmirror 镜像），SHASUMS256 校验后解包 |
| Process / Port、真实启动 | ✅ 真实：`dsh web --port` 启动、端口探测与自动分配、进程树终止、崩溃事件、启动日志 |

---

## 环境要求

桌面端需要 Rust 工具链（前端本身不需要）：

| 依赖 | 说明 |
|---|---|
| Node.js ≥ 18 | 前端构建 |
| Rust (stable, MSVC) | `https://rustup.rs` → `rustup default stable-x86_64-pc-windows-msvc` |
| Visual Studio C++ 生成工具 | 勾选「使用 C++ 的桌面开发」 |
| WebView2 | Windows 11 已内置，无需安装 |

## 运行

Windows 上可以直接双击：

| 文件 | 作用 |
|---|---|
| `run-dev.cmd` | 开发模式打开桌面窗口（自动检查环境、装依赖、生成图标） |
| `build-app.cmd` | 打包成 exe 与安装程序，完成后自动打开产物目录 |

或者用命令：

```bash
npm install

# 桌面应用（推荐）——会自动拉起 Vite 并打开 Tauri 窗口
npm run app:dev

# 打包（输出见下）
npm run app:build

# 只在浏览器里调 UI（窗口控制会提示仅桌面端可用）
npm run dev
npm run typecheck
npm test
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

打包产物：

```text
src-tauri/target/release/PHL.exe                        可直接双击的可执行文件
src-tauri/target/release/bundle/nsis/PHL_0.1.0_x64-setup.exe   安装程序
```

安装程序装完会在开始菜单创建快捷方式；也可以直接把 `PHL.exe` 发送到桌面快捷方式。

首次生成应用图标（已提交生成脚本，图标本身由脚本产出）：

```bash
npm run icon
```

## 桌面端形态

- **无边框窗口** + 自绘标题栏：主导航、下载指示、主题切换与窗口按钮在同一条 44px 的栏里。
- 标题栏可拖动（`data-tauri-drag-region`），双击最大化 / 还原，最大化后按钮切换为「向下还原」字形。
- **关闭由应用决定**：Rust 拦截 `CloseRequested` 交回前端，弹出自己的确认框并列出仍在运行的实例，
  确认后才真正退出——避免留下孤儿 DSH 进程。
- 窗口初始隐藏，前端首帧绘制完成后再显示，**没有白屏闪烁**；Rust 侧有 4 秒兜底显示，前端崩溃时窗口不会消失。
- 禁用网页右键菜单与全局文本选择，输入框、代码与路径等值仍可正常选中复制。

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri 2 + Rust |
| UI | React 18 + TypeScript |
| 构建 | Vite 6 |
| 样式 | Tailwind CSS 3（CSS 变量驱动的 Design Tokens） |
| 动效 | Motion (`motion/react`) |
| 状态 | Zustand |

## 目录结构

```text
src/                     前端
├── types/        领域模型：Instance / Version / Runtime / Plugin
├── data/         种子与目录数据（浏览器 Mock 用；桌面端数据来自 Rust）
├── services/     Repository 接口 + Mock 实现 + Tauri 实现（services/index.ts 决定来源）
├── stores/       Zustand：ui / catalog / instance / view / wizard / settings
├── lib/          cn / format / hue / motion / hooks / desktop
├── components/
│   ├── ui/       Button / Card / Progress / Menu / Dialog / Toast …
│   ├── layout/   TitleBar / Panel / Page / Router / QuickSwitcher
│   └── instance/ InstanceCard / LaunchDock / LaunchTimeline / StatusPill
└── pages/        Instances / InstanceDetail / Create / Versions / Plugins / Runtimes / Settings

src-tauri/               桌面壳
├── src/lib.rs     窗口生命周期 + 命令注册
├── src/versions.rs / plugins.rs / instances.rs / runtimes.rs
│                  Version / Plugin / Instance / Runtime 的真实实现
│                  （目录抓取、下载、完整性校验、磁盘读写与清理）
├── capabilities/  权限声明（仅开放自绘标题栏所需的窗口能力）
└── tauri.conf.json

scripts/make-icon.mjs    无依赖生成应用图标（与应用内 Logo 同一套几何）
```

抽象方向：

```text
UI  →  Repository  →  Tauri / Rust     （桌面端：版本、插件、实例、Runtime）
UI  →  Repository  →  Mock Data        （浏览器模式；桌面端的静态模板）
```

`src/services/index.ts` 是唯一决定数据来源的地方，替换实现时页面与 Store 无需改动。

## 信息架构

```text
标题栏：实例 / 版本 / 插件 / 运行时 / 设置
  └─ 左侧上下文面板（筛选、作用域、步骤、分区）
       └─ 内容区（页面）
            └─ 底部启动坞（仅实例页）
```

## 已实现的能力

- 实例列表（卡片 / 列表两种视图）、筛选、排序、搜索
- 实例持久化（真实）：`<root>/instances/<id>/`，重启不丢；创建 / 克隆 / 删除 / 改名 / 端口与 env 修改全部落盘
- 实例详情：运行环境、隔离路径、插件（由磁盘 `node_modules` 反推）、快照、统计、删除
- 创建实例向导：基本信息 → DSH 版本 → Runtime → 模板与端口 → 确认，附创建进度
- 版本管理（真实）：npm + GitHub Releases 目录、下载进度与速度、sha512 校验、取消、删除保护
- Runtime 管理（真实）：nodejs.org dist 目录（支持 npmmirror 镜像）、SHASUMS256 校验、Windows zip / Unix tar.gz 解包、系统 Node 探测
- 插件管理（真实）：社区注册表、npm / GitHub / 直链来源、安装进实例 profile 并注册 `cordis.patch.yml`
- 存储视图（真实）：实例与 Runtime 占用按磁盘实测，孤立目录扫描与一键回收
- Bundle 导出 / 导入（真实）：实例配置与插件记录打包为 JSON 清单；导入走与创建同款的暂存 + 重命名管线，插件文件不进 Bundle、通过插件页重新安装
- 快照（真实）：创建 / 回滚 / 删除 —— 复制实例的 dsh-home（插件与配置），回滚后快照保留、可反复还原；运行中的实例拒绝快照操作
- 诊断（真实）：数据目录可写、DSH 版本与 Runtime 完整性、实例引用有效性、孤立目录与下载缓存一览；缓存一键清理
- 真实启动：解析版本与 Runtime → 端口探测 / 自动分配 → 以实例自己的 `DSH_HOME` 启动 `dsh web` → 端口就绪探测 → 打开 WebUI；停止即终止进程树，崩溃自动反馈到 UI，启动日志落在实例 `logs/` 下
- 设置：外观（主题、强调色、密度、动效强度）、下载源、存储、高级、关于
- 快速跳转（Ctrl+K）、快捷键、Toast、确认与输入对话框、空状态与错误态

## 快捷键

| 快捷键 | 作用 |
|---|---|
| `Ctrl` `K` | 快速跳转 |
| `Ctrl` `,` | 打开设置 |
| `Esc` | 返回上一页；无返回路径时回到上层栏目 |

完整清单（含命令面板、对话框与下拉、插件发现等上下文快捷键）见应用内「设置 → 快捷键」。

## 参考与实现边界

PHL 在**产品模型与交互质量**上参考了 PCL、Prism Launcher、DSHBox 等已被验证的成熟设计，
包括实例优先的模型、信息架构、状态反馈节奏与动效质量。

但本项目的**代码、组件、视觉细节与全部资源均为独立实现**：

> Existing launchers may be used as product and UX references only.
> Do not copy, port, mechanically translate, or derive implementation
> code or assets from them unless license and provenance have been
> explicitly reviewed.

未复制、移植或机械改写上述任何项目的源码，未使用其 UI 素材、图标或专有视觉资产。
应用内的标识、图标与配色体系均在本仓库内定义。

## 下一步

核心链路（版本 → 插件 → 实例 → Runtime → 启动 → Bundle → 快照 → 诊断）已全部真实接入。剩余：
Source Build、环境修复等更高级的能力（路线图 §13），以及持续的真机打磨。
实例模板为静态产品内容（形状预设），不需要后端模块。
