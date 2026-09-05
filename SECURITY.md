# 安全策略 / Security Policy

PHL 是一个管理本地文件系统、下载并执行第三方代码（插件）、以及持有 API 凭据引用的桌面应用，
攻击面集中在**路径逃逸、供应链与凭据**三类。我们按下列方式处理与接收安全问题。

## 报告漏洞

- **不要**在公开 Issue / Discussion 中描述可被利用的细节。
- 请使用 GitHub 的 [Private vulnerability reporting](https://github.com/A7m0spHere/dsh-phl/security/advisories/new)
  私密报告，或通过仓库主页提供的联系方式联系维护者。
- 请包含：影响的任务/模块（如 versions 安装、插件安装）、复现步骤、影响评估。
- 修复会跟随常规版本发布；在修复版本发布前，请勿公开细节。

## 重点风险域与当前缓解

### 路径逃逸（Path containment）

- 所有破坏性命令（删除实例/版本/Runtime/插件/快照、快照还原）只接受**稳定 ID**，
  路径一律由 Rust 后端从受管状态（`PhlState`）解析，WebView 无法指定目标路径。
- ID/目录名经 `sanitize_segment` / `sanitize_version` 白名单校验；删除类操作在
  词法（组件级 `starts_with`）与规范化（canonicalize，处理 Windows `\\?\` 前缀与
  symlink/junction）两层遏制之后才会执行。
- 压缩包解压通过 `safe_join` 拒绝 `..`、绝对路径与前缀越界条目。
- 如发现任何破坏性命令能触达数据根之外，请按上述方式报告——这是最高优先级问题。

### 凭据与 Secret

- PHL **不存储 API Key 明文**：全局供应商库只保存「环境变量名」（`apiKeyEnv`），
  启动时由后端解析并注入子进程环境；日志不输出 Key、Authorization 头或完整 secret。
- Bundle 导出/导入不含任何凭据；实例清单（`instance.json`）不含 secret 字段。
- 如果发现 secret 被写入磁盘文件、日志或 Bundle，请立即报告。

### 插件供应链

- 插件是任意第三方 npm 包 / GitHub 归档，**安装即执行其声明的一切**——PHL 目前
  的信任边界是「实例内隔离」，不存在沙箱。安装前请审查来源。
- npm 安装校验 registry `dist.integrity`（sha512）；GitHub HEAD 归档目前**没有**
  完整性保障，这是已知且在改进中的缺口（pin/commit SHA 固定尚未实现）。
- 插件注册表数据（`plugins.json`、截图链接）来自社区，打开外部链接前请自行确认。

### 更新与发布完整性（Updater / Release integrity）

- PHL 自身的更新器尚未实现；当前请只从本仓库的 GitHub Releases 下载安装包，
  并核对 Release 页公布的 SHA256。
- Release 流水线的产物签名与自动更新校验在 Roadmap 中（updater 任务），
  在那之前不存在可被伪造的更新通道。

## 支持版本

项目处于 `0.1.0-alpha` 阶段，仅最新 `main` 分支与其对应的最新 Release 获得安全修复。
