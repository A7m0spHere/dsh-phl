# DSH-PHL 开发路线

> **Project:** `dsh-phl`  
> **定位:** PCL-like Harness Manager / DSH Instance & Runtime Manager  
> **核心目标:** 为 DeepSeek Harness（DSH）提供类似 PCL / Prism Launcher 的多版本、实例化、环境隔离与一键运行体验。

---

## 1. 项目为什么要做

当前 DSH 生态中的 Launcher 大多解决的是：

- 安装 DSH
- 启动 DSH
- 更新 DSH
- 提供桌面 GUI
- 管理部分插件或配置

但这没有真正解决开发者遇到的核心问题：

> **当 DSH 发布新版本、正在进行版本适配或插件开发时，我仍然需要同时使用旧版本 DSH 进行测试。**

如果全局 DSH 被升级，旧版本环境就容易被覆盖或污染，导致：

- 无法方便地回退到旧版本
- 不同 DSH 版本之间互相影响
- 插件开发环境和稳定环境混在一起
- 测试旧版本时需要额外寻找另一套 DSH
- 很难复现某一个历史 DSH 环境

因此，`dsh-phl` 不应该只是一个 Launcher。

它真正要解决的是：

> **多个 DSH 版本如何共存，以及不同 DSH 环境如何彼此隔离。**

---

# 2. 产品定位

## 2.1 不是普通 DSH Launcher

不要把项目定义成：

```text
点击按钮
    ↓
启动 dsh
```

而应该定义成：

```text
DSH Manager
    ↓
Version
    ↓
Instance
    ↓
Runtime
    ↓
Plugins / Profiles / Workspace
    ↓
Launch
```

产品本质是：

> **DSH Instance & Runtime Manager**

也可以理解为：

> **PCL / Prism Launcher for DeepSeek Harness**

---

# 3. “PHL” 的含义

项目名称：

```text
dsh-phl
```

命名思路来自：

```text
PCL
 ↓
PHL
```

即将 Minecraft / 游戏启动器中的核心理念迁移到 Harness。

项目可以对外描述为：

> **PHL — Prism-like Harness Launcher**

或者：

> **PHL — Harness Instance & Runtime Manager**

这里的“Launcher”不应该理解成单纯的启动器，而是包含：

- 版本管理
- 实例管理
- Runtime 管理
- 环境隔离
- 插件管理
- 配置管理
- 启动与诊断

---

# 4. 参考项目与应该吸收的思路

开发过程中不需要直接复制任何项目，而是吸收不同项目已经验证过的设计。

## 4.1 `loudMore/dsh-launcher`

主要借鉴：

- DSH 安装
- 环境检测
- 更新机制
- 面向普通用户的使用体验
- Windows 桌面应用思路

它更接近：

```text
易用的 DSH Launcher / Manager
```

但没有完全解决 PHL 最核心的 Instance 隔离问题。

---

## 4.2 `WEP-56/DSH-Launcher`

主要借鉴：

- Tauri 桌面架构
- DSH WebUI 集成
- 多窗口 / Tab
- 插件与配置管理
- 桌面端交互方式

它更偏：

```text
DSH Desktop Shell
```

而不是完整的 Instance Manager。

---

## 4.3 DSHBox

这是目前最接近 PHL 核心理念的参考项目之一。

主要吸收：

- Instance / Container 思路
- 多版本共存
- 每个 Instance 固定 DSH 版本
- 独立运行环境
- Extension / Skill 管理
- Bundle / 环境复用

它验证了一个重要方向：

> **DSH 不应该只有一个全局环境，而应该存在多个彼此独立的运行实例。**

---

## 4.4 其他 DSH Desktop / Version Manager

可以参考其：

- 多版本安装
- Release 下载
- 更新管理
- Node Runtime 管理
- 版本缓存
- 原子安装
- 系统托盘
- 自动更新

这些能力可以作为 PHL 的基础设施层。

---

## 4.5 PCL / Prism Launcher

这是产品设计层面的主要参考对象。

真正需要借鉴的不是 UI，而是：

> **Instance-first（实例优先）**

用户管理的不是简单的“某个程序版本”，而是一个完整运行环境：

```text
Instance
├── DSH Version
├── Runtime
├── Plugins
├── Profile
├── Config
├── Workspace
└── State
```

---

# 5. 核心产品模型

PHL 最重要的设计原则：

> **Instance 是第一公民，而不是 DSH Version。**

用户真正操作的是：

```text
Instance
    ↓
指定 DSH Version
    ↓
指定 Runtime
    ↓
指定 Plugins
    ↓
指定 Profile
    ↓
指定 Workspace
    ↓
启动
```

例如：

```text
Plugin Development
├── DSH 0.1.0-rc.7
├── Node 22
├── routing-suite
├── 自研插件
└── 开发 Workspace
```

同时：

```text
Legacy Test
├── DSH 0.1.0-rc.5
├── Node 20
├── 旧版插件
├── 旧配置
└── 测试 Workspace
```

两个实例可以同时运行，并且互不污染。

---

# 6. 目标架构

推荐从逻辑上拆成以下几层：

```text
                    PHL
                     │
        ┌────────────┼────────────┐
        │            │            │
     Versions     Instances     Store
        │            │            │
    DSH rc.5      Dev/Test      Plugins
    DSH rc.6      Stable        Skills
    DSH rc.7      Legacy        Bundles
        │            │            │
        └────────────┼────────────┘
                     │
                Runtime Layer
                     │
            Node / Dependencies
                     │
                Process Manager
                     │
                 DSH Runtime
```

---

# 7. 推荐的数据目录结构

初期可以采用类似：

```text
dsh-phl/
├── versions/
│   ├── rc.5/
│   ├── rc.6/
│   └── rc.7/
│
├── runtimes/
│   ├── node-20/
│   └── node-22/
│
├── instances/
│   ├── plugin-dev/
│   │   ├── instance.json
│   │   ├── dsh-home/
│   │   ├── plugins/
│   │   ├── profiles/
│   │   └── workspace/
│   │
│   ├── production/
│   │   ├── instance.json
│   │   └── ...
│   │
│   └── legacy-test/
│       ├── instance.json
│       └── ...
│
└── cache/
```

最终实际目录可以根据 DSH 的真实运行机制调整。

重点不是目录名称，而是：

> **不同 Instance 的状态不能默认共享。**

---

# 8. 第一阶段必须解决的隔离范围

这是整个项目最重要的技术问题。

不能只做到：

```text
DSH rc.5
DSH rc.7
```

两个 package 并存。

需要逐项确认和隔离：

1. DSH package / executable
2. Node Runtime
3. `DSH_HOME`
4. Profiles
5. Plugins
6. Plugin dependencies / `node_modules`
7. Config
8. Session / state
9. Workspace
10. Port
11. Process environment variables

目标是：

```text
Instance A
    ↓
DSH rc.5
    ↓
独立 Runtime
    ↓
独立 DSH_HOME
    ↓
独立 Plugins
    ↓
独立 Config
    ↓
独立 Port
```

以及：

```text
Instance B
    ↓
DSH rc.7
    ↓
独立 Runtime
    ↓
独立 DSH_HOME
    ↓
独立 Plugins
    ↓
独立 Config
    ↓
独立 Port
```

两个实例可以同时存在。

---

# 9. MVP：第一版只解决一个核心问题

不要一开始做成“超级 DSH 平台”。

MVP 目标：

> **多个 DSH 版本共存，并通过独立 Instance 使用不同版本。**

第一阶段功能：

### 9.1 DSH Version Manager

- 查看可用版本
- 安装指定版本
- 保留多个版本
- 删除版本
- 查看当前已安装版本
- 版本元数据

例如：

```text
DSH Versions

0.1.0-rc.5   Installed
0.1.0-rc.6   Installed
0.1.0-rc.7   Installed
```

---

### 9.2 Runtime Manager

至少考虑：

- Node 版本
- Runtime 下载
- Runtime 缓存
- Runtime 与 Instance 的绑定

例如：

```text
Plugin Dev
DSH rc.7
Node 22

Legacy Test
DSH rc.5
Node 20
```

---

### 9.3 Instance Manager

支持：

- 创建 Instance
- 删除 Instance
- 重命名 Instance
- 启动 Instance
- 停止 Instance
- 查看 Instance 状态
- 查看 Instance 使用的 DSH 版本
- 查看 Instance 使用的 Runtime

---

### 9.4 Instance Clone

非常重要。

例如：

```text
Plugin Dev
DSH rc.7
Plugins: 15
        ↓
       Clone
        ↓
Plugin Dev Test
DSH rc.7
Plugins: 15
```

这样可以快速创建测试环境。

---

### 9.5 Port 自动分配

不同 Instance 启动时应该自动获得不同端口：

```text
Production      → 3080
Plugin Dev      → 3081
Legacy Test     → 3082
```

避免多个 DSH 实例冲突。

---

### 9.6 一键启动

用户只需要：

```text
选择 Instance
    ↓
Launch
```

PHL 自动完成：

```text
准备 Runtime
    ↓
准备 DSH
    ↓
设置 DSH_HOME
    ↓
加载 Instance 配置
    ↓
分配 Port
    ↓
启动 DSH
```

---

# 10. 第二阶段：插件生态

MVP 稳定后，再增加：

```text
Plugins
├── Installed
├── Store
├── Update
├── Dependency
└── Compatibility
```

插件必须明确安装到某个 Instance，而不是默认全局安装。

例如：

```text
Plugin Dev
├── routing-suite
├── plugin-x
└── plugin-y

Production
├── routing-suite
└── plugin-z
```

---

# 11. 第三阶段：Bundle / Template

支持把一个完整环境定义成：

```text
DSH Environment Manifest

DSH: 0.1.0-rc.5
Node: 20

Plugins:
  routing-suite@1.2.3
  plugin-x@0.4.1
```

然后可以：

```text
Manifest
   ↓
Create Instance
   ↓
自动安装 Runtime
   ↓
自动安装 DSH
   ↓
自动安装 Plugins
```

这会让 DSH 环境具有可复现性。

---

# 12. 第四阶段：Snapshot

支持：

```text
Create Snapshot
```

保存：

- DSH 版本
- Runtime
- Plugins
- 配置
- Profile
- Instance metadata

例如：

```text
Snapshot
2026-08-31

DSH: rc.6
Node: 20
Plugins: 17
Profile: coding
```

用于：

- 回滚
- 调试
- 测试
- 复现问题

---

# 13. 第五阶段：高级开发者功能

后续可以考虑：

### Source Build

支持：

- 官方 Release
- Git Tag
- Git Commit
- Local Source

例如：

```text
DSH Source

Release
Git Tag
Git Commit
Local Repository
```

这样插件作者可以直接使用：

```text
本地 DSH 源码
```

进行适配测试。

---

### Compatibility

进一步可以记录：

```text
Plugin
    ↓
Compatible DSH Versions
```

例如：

```text
routing-suite

✓ rc.5
✓ rc.6
✗ rc.7
```

这会逐步形成真正的 DSH 生态兼容性管理。

---

# 14. 推荐的开发顺序

不要同时开发所有功能。

建议按以下顺序：

```text
Phase 0
研究 DSH 实际运行机制
        ↓
Phase 1
Version Manager
        ↓
Phase 2
Instance Manager
        ↓
Phase 3
Runtime Isolation
        ↓
Phase 4
Process / Port Manager
        ↓
Phase 5
One-click Launch
        ↓
Phase 6
Plugin Manager
        ↓
Phase 7
Clone / Bundle
        ↓
Phase 8
Snapshot / Import / Export
        ↓
Phase 9
Compatibility / Diagnostics
```

---

# 15. Phase 0：在写 UI 前先验证的技术问题

这是项目最值得投入时间的一阶段。

需要通过真实 DSH 环境确认：

### A. DSH 版本安装方式

确认：

- npm package 安装结构
- Release 版本获取方式
- 是否可以同时存在多个版本
- 是否需要独立 `node_modules`

### B. `DSH_HOME`

确认：

- `DSH_HOME` 的实际作用
- Profile / Plugin / State 保存在哪里
- 哪些路径仍然是全局共享的
- 是否可以通过环境变量完全隔离

### C. Node

确认：

- DSH 对 Node 的版本要求
- 不同 DSH 版本是否要求不同 Node
- 是否需要 bundled Node

### D. Plugin

确认：

- 插件安装位置
- Plugin dependencies
- `node_modules` 是否会发生跨版本污染

### E. Process

确认：

- 如何启动 DSH
- 如何停止 DSH
- 如何检测 DSH Ready
- 如何获取 PID
- 如何处理崩溃
- 如何处理端口冲突

### F. Instance 删除

确认：

> 删除一个 Instance 能否真正删除它拥有的全部状态。

---

# 16. UI 产品方向

UI 不需要一开始追求复杂。

首页首先展示：

```text
Instances

┌─────────────────────────────────────┐
│ Plugin Development                  │
│ DSH rc.7 · Node 22 · 12 plugins    │
│                          [Launch]   │
├─────────────────────────────────────┤
│ Production                          │
│ DSH rc.7 · Node 22 · 8 plugins     │
│                          [Launch]   │
├─────────────────────────────────────┤
│ Legacy Test                         │
│ DSH rc.5 · Node 20 · 5 plugins     │
│                          [Launch]   │
└─────────────────────────────────────┘
```

然后提供：

```text
+ Create Instance
+ Install DSH Version
+ Import
Settings
```

核心体验必须是：

> **“我不用关心 DSH 到底装在哪，我只需要选择我要使用的 Instance。”**

---

# 17. 项目的核心竞争力

PHL 最终不应该和已有 DSH Launcher 比：

> 谁的启动按钮更漂亮。

而应该比：

> **谁能让 DSH 开发环境真正可并存、可隔离、可复现。**

核心竞争力：

```text
Multi-Version
+
Instance Isolation
+
Runtime Isolation
+
Plugin Isolation
+
Reproducible Environment
```

---

# 18. 最终产品形态

成熟后的 PHL 可以形成：

```text
                         PHL
                          │
       ┌──────────────────┼──────────────────┐
       │                  │                  │
    Versions          Instances           Store
       │                  │                  │
       │                  │              Plugins
       │                  │              Skills
       │                  │              Bundles
       │                  │
       │          ┌───────┼───────┐
       │          │       │       │
       │        Dev     Stable   Legacy
       │          │       │       │
       └──────────┼───────┼───────┘
                  │
             Runtime Layer
                  │
           Node / Dependencies
                  │
             Process Manager
                  │
              DSH Runtime
```

最终目标是让用户可以像管理 Minecraft Instance 一样管理 DSH：

```text
安装多个 DSH 版本
        ↓
创建多个独立 Instance
        ↓
每个 Instance 固定自己的 DSH / Runtime / Plugins
        ↓
多个 Instance 可以同时运行
        ↓
开发、测试、生产互不影响
        ↓
环境可以 Clone / Snapshot / Export / Reproduce
```

---

# 19. 当前阶段的明确目标

`dsh-phl` 的第一目标不是成为一个功能很多的桌面软件。

第一目标只有一句话：

> **让用户可以在同一台机器上同时、稳定地运行多个不同版本的 DSH，并保证它们的运行环境彼此隔离。**

只要这一点真正做好，PHL 就已经与“普通 DSH Launcher”产生了本质区别。

后续的插件商店、Bundle、Snapshot、Compatibility 等能力，都建立在这一核心模型之上。

---

## 一句话产品定义

> **PHL is a PCL-like instance manager for DeepSeek Harness, designed to make multiple DSH versions and development environments coexist safely and reproducibly.**
