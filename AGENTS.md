# AGENTS.md — dsh-phl 开发约定

## 项目定位

PHL 是 DeepSeek Harness 的**实例与运行时管理器**，不是普通 Launcher。
核心命题：让多个 DSH 版本在同一台机器上共存，且每个实例的运行环境彼此隔离。

形态是 **Tauri 2 桌面应用**（无边框窗口 + 自绘标题栏），不是网页。
当前阶段：核心链路（版本 / 插件 / 实例 / Runtime / 启动 / Bundle / 快照）已真实接入 Rust
管线，会真实读写磁盘与拉起进程；浏览器模式（`npm run dev`）仍走 Mock，便于纯 UI 开发。
桌面端启动 DSH 的事实约定：只有 `dsh web` 子命令解析 `--port`，`DSH_HOME` 是隔离注入点，
实例 profile 固定为 `web`（详见 `src-tauri/src/launch/mod.rs` 模块文档）。

## 桌面壳约定

- 窗口 `decorations: false` + `visible: false`：标题栏由前端绘制，窗口在前端首帧后由 `app_ready` 显示。
  Rust 侧有 4 秒兜底显示，前端崩溃时不要让窗口永远不出现。
- 关闭流程：Rust 拦截 `CloseRequested` → 发 `phl://close-requested` → 前端确认 → `exit_app`。
  不要在前端直接 `destroy()` 绕过确认。
- 拖拽用 `data-tauri-drag-region`（不是 Electron 的 `-webkit-app-region`）。可交互元素不要带这个属性。
- 新增任何 Tauri 能力必须同时在 `src-tauri/capabilities/default.json` 声明权限，保持最小授权。
- 前端不得假设自己一定跑在 Tauri 里：所有窗口/系统调用走 `src/lib/desktop.ts`，浏览器下降级为 no-op。

## 代码来源边界（必须遵守）

> Existing launchers may be used as product and UX references only.
> Do not copy, port, mechanically translate, or derive implementation
> code or assets from them unless license and provenance have been
> explicitly reviewed.

- 可以参考：产品理念、UX 模式、实例模型、通用架构、已被验证的功能设计。
- 不可以：复制 PCL 或其他项目的源代码、UI 素材、图标与专有视觉资产；不做机械改写或翻译式移植。
- 图标使用 lucide-react（ISC），应用标识与配色体系在本仓库内自行定义。
- 应用标识（鲸鱼尾鳍）的母版在 `src-tauri/icons/master/`，由 `npm run icon` 生成全套
  PNG 阶梯、`icon.ico`（9 档）与 `icon.icns`；**小尺寸是重画而非缩放**：≤32px 去掉内部负空间
  并给标记更多画布，≤20px 用 alpha gamma + 边缘对比度压缩把抗锯齿边缘压硬（半透明像素从约 1/4
  降到 3–5%）。小尺寸策略由 `--small=` 选择，**四种模式都保留**，可随时回退：
  `soft`（最早的发虚版）/ `crisp`（用户确认过的那版）/ `hard`（当前默认，最硬）/
  `median`（二值化 + 中值 + 羽化，实测反而更糊，仅留档）。
  字节级回滚：`src-tauri/icons/snapshots/` 下是已确认版本的快照，
  `node scripts/restore-icon-snapshot.mjs <name>` 一键还原。
  注意两条已踩过的坑：**形态学加粗（dilate）**会把两片尾鳍之间的缺口堵死，16px 变成一团蓝块；
  **开运算（erode+dilate）**会把尾鳍尖端削钝而并不减少发虚，两样都不要再用。
  应用内 `Logo.tsx` 用同一母版做 CSS mask，颜色跟随主题 accent。
  改图标只改母版并重跑命令，不要在别处手写图形；母版来源与「非官方」边界见
  `src-tauri/icons/master/PROVENANCE.md`。
- **Windows 任务栏图标要按 DPI 自己挑**：Tauri 只会从 `icon.ico` 里解出一张（16×16）设为
  窗口图标，125% 缩放下任务栏要 20px，于是被放大成糊的。清空窗口图标也不行——系统会回退到
  exe 内嵌图标，而那个由 Explorer 的图标缓存管，缓存里可能还是上一版图形。
  `lib.rs` 的 `apply_taskbar_icon` 按窗口 `scale_factor` 从生成的 PNG 阶梯里选 16/20/24 那档
  直接设上去。改窗口/任务栏图标时不要删掉这个调用，也不要删 Cargo.toml 的 `image-png` 特性。

## 架构约定

- **Instance 是第一公民**，Version / Runtime / Plugin 都从属于实例。
- UI 不直接访问数据源，一律经过 `src/services`（`PhlRepository`）。
  替换为 Tauri / PHL Core 时只改 `src/services/index.ts`。
- 数据不硬编码在 JSX：种子数据放 `src/data`，类型放 `src/types`。
- 页面级视图状态（筛选、作用域、步骤）放 `src/stores/viewStore.ts` 等 store，
  因为左侧上下文面板与内容区是两棵独立子树，必须共享同一份状态。
- 长耗时操作（启动、下载、创建）由 repository 提供进度回调与 `AbortSignal`，store 只负责映射到 UI 状态。

## 设计与动效约定

- Design Tokens 全部走 CSS 变量（`src/index.css`），Tailwind 通过 `hsl(var(--x) / <alpha-value>)` 消费。
  新增颜色请加 token，不要写死色值。
- 动效统一从 `src/lib/motion.ts` 的 `useMotion()` 取，不要在组件里另写 duration / easing。
  用户可在设置里把动画强度调成「精简 / 关闭」，所有动效必须尊重该设置（`scale === 0` 时降级为无位移）。
- 圆角克制：卡片 `rounded-lg`(10px)，按钮 `rounded`(8px)，标签 `rounded-xs`(4px)。
- 状态必须可读：运行中用光晕呼吸点，启动中用阶段文案 + 进度，失败给出原因 + 修复建议 + 可执行按钮。

## 命令

```bash
npm run dev        # 开发
npm run build      # tsc -b && vite build
npm run typecheck
```

TypeScript 开启了 `noUnusedLocals` / `noUnusedParameters`，提交前请确保 `npm run typecheck` 通过。

## 文档分工（O-15）

- `AGENTS.md`（本文件）是开发约定的唯一权威来源；`CLAUDE.md` 只是指向这里的兼容入口。
- 当前状态与优先级的单一入口是 `PHL_OPTIMIZATION_ROADMAP.md`；
  `dsh-phl-project-master-plan.md`、`dsh-phl-development-roadmap.md` 保留历史愿景，
  `PROJECT_REVIEW.md` 是带日期的审查快照，`dsh-phl-manual-test-checklist.md` 承载真机验收记录。
