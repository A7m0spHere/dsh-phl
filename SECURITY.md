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

- Key 本体存放在两处：全局供应商库 `config/api.json` 可以保存用户粘贴的
  **Key 明文**（本地文件，不进实例目录、不进任何 Bundle），实例目录只持有
  「环境变量名」（`apiKeyEnv`）——`settings.yaml` 写的是名字，启动时由后端
  把 Key 注入子进程环境。
- PHL 自身日志不输出 Key、Authorization 头或完整 secret；子进程（dsh）的
  stdout/stderr 会写入实例 `logs/`，其内容由 DSH 决定，超出 PHL 的承诺范围。
- **Bundle（格式 2）导出不含凭据值**：变量名命中全局库声明的 `apiKeyEnv`，
  或名称含凭据字样（KEY / TOKEN / SECRET / PASS / CREDENTIAL / AUTH …）时，
  只导出变量名并提示导入方重新配置；`PATH`、`DSH_HOME` 等机器本地变量同样
  不导出。导入端对格式 1 旧包执行同一过滤，被剥离的凭据名在导入预览与
  导入结果中明示。
- 分类基于**变量名**：若把 Key 存进一个既非 `apiKeyEnv`、名称也不含凭据
  字样的普通变量，导出不会识别它——请把 Key 交给全局供应商库，或避免把
  secret 写入实例环境变量。
- 实例清单（`instance.json`）没有 secret 字段；快照是 `dsh-home` 的完整
  副本，同样只含变量名（Key 由启动时注入，不落盘到实例目录）。
- 应用状态文件（数据根指针 `root.json`、迁移日志 `migration.json`、进程登记
  `processes.json`，均位于用户配置目录的 `PHL/` 下、数据根之外）只保存路径、目录名、
  字节数与 PID 等元数据，不含任何凭据值。
- 如果发现 secret 被写入磁盘文件、日志或 Bundle，请立即报告。

### 插件供应链

- 插件是任意第三方 npm 包 / GitHub 归档，**安装即执行其声明的一切**——PHL 目前
  的信任边界是「实例内隔离」，不存在沙箱。安装前请审查来源。
- npm 安装校验 registry `dist.integrity`（sha512）；GitHub 来源在固定到 tag/commit 时
  解析出确切 commit 并对下载内容计算 sha512（`actualIntegrity` 写入安装标记，来源与
  信任状态一并记录）。HEAD（未固定）归档没有可校验的内容承诺，安装标记会记为
  `unverified`——需要完整性保障时，请把插件固定到 tag 或 commit 再安装。
- 插件注册表数据（`plugins.json`、截图链接）来自社区，打开外部链接前请自行确认。

### 更新与发布完整性（Updater / Release integrity）

- PHL 自身的更新器尚未实现；当前请只从本仓库的 GitHub Releases 下载安装包，
  并核对 Release 页公布的 SHA256。
- Release 流水线的产物签名与自动更新校验在 Roadmap 中（updater 任务），
  在那之前不存在可被伪造的更新通道。

## 支持版本

项目处于 `0.1.0-alpha` 阶段，仅最新 `main` 分支与其对应的最新 Release 获得安全修复。
