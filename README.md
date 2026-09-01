# dsh-phl / PHL

> **PHL — DSH Instance & Runtime Manager**
> 一个独立实现的、PCL 风格的 DeepSeek Harness 实例与运行时管理器。

当前仓库处于 **Phase 1：纯前端高保真原型**，但已经是一个真正的 **Tauri 2 桌面应用**（无边框窗口、自绘标题栏、
原生窗口控制与安装包）。所有业务数据来自 Mock Repository，启动、下载、创建都是模拟过程，不会真正操作文件系统或进程。
这一阶段的目标只有一个：

> 验证「实例是第一公民」这一产品模型是否成立。

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
| 构建 | Vite 5 |
| 样式 | Tailwind CSS 3（CSS 变量驱动的 Design Tokens） |
| 动效 | Motion (`motion/react`) |
| 状态 | Zustand |

## 目录结构

```text
src/                     前端
├── types/        领域模型：Instance / Version / Runtime / Plugin
├── data/         Mock 种子数据（不在 JSX 中硬编码）
├── services/     Repository 接口 + Mock 实现（未来替换为 Tauri / PHL Core）
├── stores/       Zustand：ui / catalog / instance / view / wizard / settings
├── lib/          cn / format / hue / motion / hooks / desktop
├── components/
│   ├── ui/       Button / Card / Progress / Menu / Dialog / Toast …
│   ├── layout/   TitleBar / Panel / Page / Router / QuickSwitcher
│   └── instance/ InstanceCard / LaunchDock / LaunchTimeline / StatusPill
└── pages/        Instances / InstanceDetail / Create / Versions / Plugins / Runtimes / Settings

src-tauri/               桌面壳
├── src/lib.rs    窗口生命周期：延迟显示、关闭拦截
├── capabilities/ 权限声明（仅开放自绘标题栏所需的窗口能力）
└── tauri.conf.json

scripts/make-icon.mjs    无依赖生成应用图标（与应用内 Logo 同一套几何）
```

抽象方向：

```text
UI  →  Repository  →  Mock Data        （现在）
UI  →  Repository  →  PHL Core / Tauri （Phase 2+）
```

`src/services/index.ts` 是唯一决定数据来源的地方，替换实现时页面与 Store 无需改动。

## 信息架构

```text
标题栏：实例 / 版本 / 插件 / 运行时 / 设置
  └─ 左侧上下文面板（筛选、作用域、步骤、分区）
       └─ 内容区（页面）
            └─ 底部启动坞（仅实例页）
```

## 已实现的原型能力

- 实例列表（卡片 / 列表两种视图）、筛选、排序、搜索
- 实例详情：运行环境、隔离路径、插件、快照、统计、删除
- 创建实例向导：基本信息 → DSH 版本 → Runtime → 模板与端口 → 确认，附创建进度
- 模拟启动流程：7 个阶段的实时进度与阶段说明
- 真实的失败路径：版本未安装 / Runtime 未安装 / Node 版本不匹配 / 端口冲突
- 版本管理：多版本共存、下载进度与速度、取消、重试、删除保护
- Runtime 管理：Node 多版本、系统 Node、使用方提示
- 插件管理：以实例为作用域的安装 / 启用 / 更新 / 兼容性标注
- 设置：外观（主题、强调色、密度、动效强度）、下载、存储占用、高级、关于
- 快速跳转（Ctrl+K）、快捷键、Toast、确认与输入对话框、空状态与错误态

## 快捷键

| 快捷键 | 作用 |
|---|---|
| `Ctrl` `K` | 快速跳转 |
| `Ctrl` `N` | 新建实例 |
| `Ctrl` `,` | 打开设置 |
| `Esc` | 返回上一页 |

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

Phase 2 起接入真实能力，顺序参见 `dsh-phl-development-roadmap.md`：
Version Manager → Runtime Manager → Instance Manager → Process / Port Manager → 真实启动。
