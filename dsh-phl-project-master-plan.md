# DSH-PHL 项目总纲与开发路线

> Project: `dsh-phl`  
> Product: **DSH-PHL / PHL**  
> Positioning: **PCL-like Instance & Runtime Manager for DeepSeek Harness**

## 1. 核心目标

PHL 不是一个普通的 DSH Launcher，而是一个类似 PCL / Prism Launcher 的：

> **DSH Instance & Runtime Manager**

核心解决的问题是：当开发者需要适配新版本 DSH 时，仍然能够保留并同时运行旧版本 DSH 进行测试。

目标：

```text
多个 DSH Version
        ↓
多个独立 Instance
        ↓
每个 Instance 固定自己的 DSH / Runtime / Plugins / Profile
        ↓
多个实例可以同时运行
        ↓
开发 / 测试 / 生产互不污染
```

---

## 2. 产品核心原则

### Instance First

**Instance 是第一公民，而不是 DSH Version。**

一个 Instance 包含：

```text
Instance
├── DSH Version
├── Runtime
├── Plugins
├── Profile
├── Config
├── Workspace
├── Port
└── State
```

例如：

```text
Plugin Development
├── DSH 0.1.0-rc.7
├── Node 22
├── 12 Plugins
└── Port 3081
```

同时：

```text
Legacy Test
├── DSH 0.1.0-rc.5
├── Node 20
├── 5 Plugins
└── Port 3082
```

---

## 3. PHL 与普通 DSH Launcher 的区别

| 类型 | 核心目标 |
|---|---|
| 普通 DSH Launcher | 方便启动 DSH |
| DSH Version Manager | 管理多个 DSH 版本 |
| DSH Desktop | 将 DSH 包装成桌面应用 |
| **PHL** | **管理多个完整、隔离、可复现的 DSH Instance** |

PHL 真正的竞争力不是“启动按钮”，而是：

> **让 DSH 开发、测试和生产环境真正可以共存。**

---

## 4. 参考项目

### PCL

主要参考：

- Instance 产品模型
- Launcher 信息架构
- 版本选择
- Runtime 管理
- 卡片设计
- 页面过渡
- Loading
- 状态反馈
- 动效节奏
- 空状态 / Error 状态
- 下载 / 安装 / 启动反馈

原则：

> **可以认真借鉴 PCL 的 UX 与动效质量，但必须独立实现代码、组件、资源与视觉细节。**

目标是：

> **PCL 级别的 UX + DSH 原生产品模型 + 独立实现。**

### Prism Launcher

参考：

- Instance-first
- 多版本共存
- 实例复制
- 配置隔离

### DSHBox

参考：

- Instance / Container
- 多版本共存
- 每实例固定版本
- Extension / Skill
- Bundle
- 隔离环境

### 其他 DSH Desktop / Version Manager

参考：

- Version Manager
- Release 下载
- Runtime 管理
- 更新
- 进程控制
- 桌面应用体验

---

# 5. 开源许可证与代码来源边界

PHL 应作为**独立实现**开发。

### 可以参考

- 产品理念
- UX 模式
- Instance 模型
- 通用软件架构
- 已有项目验证过的功能设计

### 不直接复制

- PCL 源代码
- 其他项目源代码
- PCL 原始 UI 素材
- PCL 图标资源
- PCL 专有视觉资产
- 机械改写 / 翻译源码
- 大量复制具体实现结构

推荐代码来源关系：

```text
PCL
└── Product / UX inspiration

Prism Launcher
└── Instance model inspiration

DSHBox
└── DSH isolation inspiration

Other DSH Launchers
└── Version / Desktop ideas

dsh-phl
└── Independent implementation
```

不要直接 Fork PCL 再改造成 DSH Launcher。

同时建议在 `AGENTS.md` / `CLAUDE.md` 中明确：

```md
Existing launchers may be used as product and UX references only.
Do not copy, port, mechanically translate, or derive implementation
code or assets from them unless license and provenance have been
explicitly reviewed.
```

---

# 6. 技术栈

## Desktop

**Tauri 2**

用于：

- 文件系统
- 环境变量
- 进程管理
- 本地 Runtime
- DSH WebUI 集成
- Windows / macOS / Linux

## Frontend

```text
React
TypeScript
Vite
Tailwind CSS
Motion
Zustand
```

### React + TypeScript

负责完整 UI 与类型安全。

### Tailwind CSS

用于快速构建布局、状态和主题，并建立 PHL 自己的 Design Tokens。

### Motion

用于实现高质量 Launcher 动效：

- Page Transition
- Card Transition
- Hover / Press
- Loading
- Launch 状态切换
- Modal
- Wizard
- List Animation

### Zustand

负责：

```text
Instances
Versions
Plugins
Runtimes
Processes
Downloads
Settings
```

## Backend / Core

**Rust**

推荐架构：

```text
React
  ↓
Tauri Layer
  ↓
PHL Core
  ↓
Operating System
```

PHL Core：

```text
Instance Manager
Version Manager
Runtime Manager
Plugin Manager
Process Manager
Port Manager
File / Config Manager
```

## Persistence

MVP：

```text
JSON
```

成熟版本：

```text
SQLite
```

## Testing

```text
Vitest
Playwright
```

## CI / Release

```text
GitHub Actions
```

---

# 7. Core 架构

```text
                    PHL
                     │
            ┌────────┴────────┐
            │                 │
        Frontend          Tauri Layer
            │                 │
            └────────┬────────┘
                     │
                  PHL Core
                     │
     ┌───────────────┼────────────────┐
     │               │                │
 Instance Manager Version Manager Runtime Manager
     │               │                │
     ├───────────────┼────────────────┤
     │               │                │
 Plugin Manager Process Manager Port Manager
     │               │                │
     └───────────────┼────────────────┘
                     │
                    DSH
```

---

# 8. 运行环境隔离

不能只做到多个 DSH package 共存。

真正需要确认并尽量隔离：

```text
DSH Version
Node Runtime
DSH_HOME
Profiles
Plugins
Plugin Dependencies / node_modules
Config
Session / State
Workspace
Port
Process Environment
```

理想状态：

```text
Instance A
├── DSH rc.5
├── Node 20
├── DSH_HOME A
├── Plugins A
├── Profile A
├── Workspace A
└── Port 3082
```

```text
Instance B
├── DSH rc.7
├── Node 22
├── DSH_HOME B
├── Plugins B
├── Profile B
├── Workspace B
└── Port 3081
```

目标：

> **A 与 B 可以同时运行，并且不会因版本、插件、配置或状态产生非预期污染。**

---

# 9. 第一阶段：纯前端高保真 Prototype

第一阶段不要连接真实 DSH。

使用 Mock Data 验证产品模型。

目标体验：

```text
打开 PHL
  ↓
看到多个 Instance
  ↓
进入 Instance Detail
  ↓
看到 DSH / Runtime / Plugins / Port
  ↓
创建新 Instance
  ↓
选择 DSH Version
  ↓
选择 Runtime
  ↓
选择 Template
  ↓
Create
  ↓
Launch
  ↓
Starting
  ↓
Running
```

---

# 10. 前端页面

## Instances

首页，最重要页面。

展示：

```text
Plugin Development
DSH rc.7
Node 22
12 Plugins
Port 3081
Running

Production
DSH rc.7
Node 22
8 Plugins
Port 3080
Stopped

Legacy Test
DSH rc.5
Node 20
5 Plugins
Port 3082
Stopped
```

操作：

```text
Launch
Stop
Clone
Edit
Delete
```

## Instance Detail

展示：

```text
DSH Version
Runtime
Status
Port
DSH Home
Workspace
Profile
Plugins
```

操作：

```text
Launch
Stop
Clone
Edit
Delete
Snapshot
```

## Create Instance

流程：

```text
Basic Information
        ↓
DSH Version
        ↓
Runtime
        ↓
Template
        ↓
Create
```

## Versions

展示：

```text
rc.7
Latest / Installed

rc.6
Installed

rc.5
Legacy / Installed
```

核心信息：

> 多个 DSH Version 可以同时安装。

## Plugins

展示：

```text
Installed
Available
Updates
Compatibility
```

每个插件显示：

```text
Version
Compatible DSH Versions
Used by Instances
Status
```

## Runtimes

例如：

```text
Node 22
Installed
Used by 3 instances

Node 20
Installed
Used by 1 instance
```

## Settings

初期：

```text
General
Downloads
Appearance
Storage
Advanced
```

---

# 11. 前端视觉方向

产品定位：

> **现代桌面软件 + 开发者工具 + Launcher**

关键词：

```text
Clean
Technical
Desktop
Focused
Modern
Lightweight
```

应该具备：

- 清晰的信息层级
- 合理留白
- Card / Panel
- 明确状态
- 克制圆角
- 高质量过渡
- 即时操作反馈

避免：

- 传统 SaaS Dashboard
- KPI 大屏
- 巨型图表
- 花哨渐变
- 过度玻璃拟态
- 无意义装饰

---

# 12. PCL UX 借鉴原则

认真研究 PCL 的：

- 页面信息架构
- 导航
- Card
- Button
- Dialog
- List
- Progress
- Hover / Press
- Page Transition
- Loading
- Success / Error
- 下载进度
- 启动反馈

例如：

```text
Stopped
  ↓
Starting
  ↓
Running
```

以及：

```text
Creating
  ↓
Preparing Environment
  ↓
Applying Configuration
  ↓
Done
```

这些体验可以重新实现，不应复制 PCL 的实现代码。

---

# 13. Mock Data 与代码结构

不要把数据硬编码在 JSX。

推荐：

```text
src/
├── data/
│   ├── instances.ts
│   ├── versions.ts
│   ├── plugins.ts
│   └── runtimes.ts
│
├── types/
│   ├── instance.ts
│   ├── version.ts
│   ├── plugin.ts
│   └── runtime.ts
│
├── stores/
├── components/
├── pages/
└── services/
```

抽象：

```text
UI
 ↓
Repository
 ↓
Mock Data
```

未来：

```text
UI
 ↓
Repository
 ↓
PHL Core / Tauri
```

---

# 14. MVP

第一版只解决：

> **多个 DSH 版本共存，并且每个 Instance 可以固定使用自己的 DSH Version。**

MVP：

1. Version List
2. Install Version
3. Create Instance
4. 指定 DSH Version
5. 指定 Runtime
6. 独立 DSH_HOME
7. 独立 Port
8. Launch / Stop
9. Clone Instance
10. Delete Instance

---

# 15. 后续开发路线

> **当前进度与顺序调整（2026-09-03）**
>
> 实际落地顺序已经偏离本节最初的排布，原因是 Plugin Manager 提前实现了。
>
> | 模块 | 状态 |
> |---|---|
> | Version Manager | ✅ 已接入（GitHub Releases + npm，真实下载 / 校验 / 解包 / 删除） |
> | Plugin Manager | ✅ 已接入（社区注册表 + 真实安装管线、`cordis.patch.yml` 读写） |
> | Instance Manager | ✅ 已接入（磁盘持久化，详见 `dsh-phl-instance-manager-plan.md`） |
> | Runtime Manager | ✅ 已接入（nodejs.org dist 目录 + npmmirror 镜像，SHASUMS256 校验，Windows zip / Unix tar.gz 解包，系统 Node 探测） |
> | Process / Port、真实启动 | ✅ 已接入（`dsh web --port` 启动、`DSH_HOME` 指向实例目录、端口探测与自动分配、进程树终止、崩溃事件、启动日志） |
> | Bundle | ✅ 已接入（manifest 级导出/导入：配置 + 插件记录，暂存重命名建树；插件文件经正常管线重装。实例模板为静态产品内容） |
> | Snapshot | ✅ 已接入（dsh-home 级创建/回滚/删除；回滚复制还原、快照保留可反复使用；运行中拒绝操作；克隆不携带快照历史） |
> | Diagnostics 诊断 | ✅ 已接入（数据目录可写、版本/Runtime 完整性、实例引用有效性、孤立目录一览、缓存清理；设置页「诊断」分区） |
> | **高级能力** | ⬅ **下一步**（Source Build / 环境修复 / 兼容性矩阵，见路线图 §13） |
>
> **顺序以 `dsh-phl-development-roadmap.md` §14 为准**（Version → Instance → Runtime
> → Process/Port → Launch）。启动事实来自对 `@deepseek-ai/dsh` 实际源码的核查：
> 只有 `dsh web` 子命令解析 `--port` 并提供 WebUI，`DSH_HOME` 是官方认可由 launcher
> 注入的隔离点 —— 因此实例 profile 固定为 `web`，隔离靠每实例独立的 `DSH_HOME`。

## Phase 0 — DSH Runtime Research

确认：

- DSH package 结构
- Release 获取方式
- `DSH_HOME`
- Profile
- Plugin
- `node_modules`
- Node 要求
- Runtime 要求
- Port
- Process
- Session
- Workspace
- Config
- Instance 删除后的清理范围

这是最重要的底层验证。

## Phase 1 — Frontend Prototype

完成：

- Instances
- Instance Detail
- Create Instance
- Versions
- Plugins
- Runtimes
- Settings
- Mock Launch
- Mock Stop
- Mock Clone

## Phase 2 — Version Manager

实现：

```text
List Versions
Install Version
Remove Version
Version Metadata
Version Cache
```

## Phase 3 — Runtime Manager

管理多个 Node Runtime，并允许 Instance 指定 Runtime。

## Phase 4 — Instance Manager

真实实现：

```text
Create
Clone
Delete
Edit
Start
Stop
```

## Phase 5 — Process / Port Manager

实现：

```text
Process Lifecycle
Port Allocation
Port Conflict Detection
PID Tracking
Crash Detection
Logs
```

## Phase 6 — Real DSH Integration

启动流程：

```text
Instance
 ↓
Resolve Version
 ↓
Resolve Runtime
 ↓
Resolve DSH_HOME
 ↓
Resolve Plugins
 ↓
Allocate Port
 ↓
Spawn DSH
 ↓
Detect Ready
 ↓
Open WebUI
```

## Phase 7 — Plugin Manager

加入：

```text
Installed
Available
Updates
Compatibility
Dependencies
```

## Phase 8 — Clone / Bundle

支持：

```text
Instance → Clone
```

以及环境 Manifest：

```text
DSH
Node
Plugins
Profile
Config
```

## Phase 9 — Snapshot

支持：

```text
Create Snapshot
Restore Snapshot
Rollback
```

用于：

- 回归测试
- Bug 复现
- 插件开发
- 环境备份

## Phase 10 — Advanced

后续可加入：

- Git Tag / Commit
- Local DSH Source
- Compatibility Matrix
- Plugin Store
- Bundle Store
- Import / Export
- Diagnostics
- Environment Repair
- Instance Templates
- Automatic Migration

---

# 16. 典型使用场景

### Production

```text
DSH rc.7
Node 22
8 Plugins
Port 3080
```

### Plugin Development

```text
DSH rc.7
Node 22
12 Plugins
Port 3081
```

### Legacy Test

```text
DSH rc.5
Node 20
5 Plugins
Port 3082
```

最终：

```text
Production       ──────── rc.7
Plugin Dev       ──────── rc.7
Legacy Test      ──────── rc.5
```

可以同时运行。

---

# 17. Agent 开发规范

所有 coding agent 必须遵守：

### 代码独立实现

- 不复制 PCL 源代码
- 不复制其他项目源码
- 不复制 PCL 素材
- 不机械迁移已有实现

### UX 可以深入参考

允许：

- 研究 PCL
- 研究 Prism Launcher
- 研究 DSHBox
- 研究其他 Launcher
- 借鉴通用设计模式

### 架构原则

- UI 与 Core 解耦
- Mock Repository 与真实 Repository 解耦
- Instance 作为核心模型
- Version 与 Instance 解耦
- Runtime 与 DSH Version 解耦
- 底层系统操作集中在 PHL Core

---

# 18. 当前阶段的最高优先级

不要一开始做“超级 DSH 平台”。

第一目标只有一句话：

> **让用户能够在同一台机器上稳定运行多个不同版本的 DSH，并保证它们的运行环境彼此隔离。**

前端 Prototype 阶段则进一步验证：

> **“PCL-like Instance Manager for DSH”这一产品模型是否成立。**

---

# 19. 最终愿景

```text
安装多个 DSH 版本
        ↓
创建多个独立 Instance
        ↓
每个 Instance 固定 DSH / Runtime / Plugins
        ↓
多个 Instance 同时运行
        ↓
开发 / 测试 / 生产互不影响
        ↓
Clone
        ↓
Snapshot
        ↓
Export / Import
        ↓
可复现 DSH Environment
```

最终用户不需要理解：

- DSH 安装在哪里
- Node 安装在哪里
- `DSH_HOME` 怎么设置
- 哪个插件属于哪个版本
- 哪个 Port 可以使用
- 如何启动某个特定版本

用户只需要：

> **选择一个 Instance，然后点击 Launch。**

---

# 20. 一句话产品定义

> **DSH-PHL is an independent, PCL-inspired instance and runtime manager for DeepSeek Harness, designed to make multiple DSH versions and development environments coexist safely, cleanly, and reproducibly.**
