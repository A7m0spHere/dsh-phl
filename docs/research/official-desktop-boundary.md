# 官方桌面端协助诊断与独立测试的安全接口边界（Issue #14 · 研究交付）

调研日期：2026-10-31。研究对象：PHL 的 `official` 模块（发现/状态/启动/确认后退出，v1
已随 0.1.1-alpha.1 发布）与官方 DeepSeek Harness 桌面端（Electron 壳 + 冻结 DSH
host，nightly 更新通道）的**可合法观测面**。本文是研究交付，不直接实施；每个结论
标注证据位置，无法确认的项如实标注「未验证」。

证据纪律：官方侧事实来自对已安装官方桌面的安装清单、进程行为与其自带配置文件的
读取（2026-09-29 规划输入，内部编号 §1-§8），PHL 侧事实来自本仓源码行号引用；
不复制、不移植官方实现或资产，临时解包产物已删除、未进仓库。

## 1. 现状盘点（v1 已实现，issue 明确不重复请求）

PHL 当前对官方桌面端实现四件事，全部 fail-closed：

| 能力 | 实现 | 关键证据 |
| --- | --- | --- |
| 发现与状态 | Windows：卸载注册表三视图（HKLM/HKLM WOW64/HKCU）读 `DisplayName/DisplayVersion/InstallLocation`，健康判定 = `InstallLocation\DeepSeek Harness.exe` 存在；macOS：`/Applications` 与 `~/Applications` 的 `.app` + Info.plist | `src-tauri/src/official/windows.rs:43-132`、`macos.rs:18-53` |
| 运行判定 | 进程快照 + 「父进程不在同 exe 集合内」选主进程 + `probe_process` 复核 Alive 与全路径身份；无法确认一律报「无法确认」（`running: null`） | `src-tauri/src/official/mod.rs:134-176,347-380` |
| 启动 | 已运行即拒绝；启动时不注入环境、不设 cwd、不加 flags（双击语义，单实例锁兜底） | `src-tauri/src/official/mod.rs:249-266` |
| 退出 | 现场重探 pid → 信号前二次全路径身份门 → 礼貌请求（WM_CLOSE/SIGTERM）→ 15s 宽限 → 用户确认后才强杀 | `src-tauri/src/official/mod.rs:293-316` |

## 2. 能力边界矩阵（本研究的核心交付）

图例：**可读**（存在合法、稳定的观测面）· **官方允许操作**（官方自身行为证明安全）
· **不支持**（官方无受支持接口，或涉及私有格式/进程注入）· **未验证**（真机待验）。

| 能力 | 判定 | 依据 |
| --- | --- | --- |
| 安装来源（HKCU per-user vs HKLM 机器级） | 可读 | 卸载注册表视图本身就是安装来源（`windows.rs:39-42`，2026-10-03 实测条目在 HKCU） |
| 版本 | 可读 | 注册表 `DisplayVersion` / Info.plist `CFBundleShortVersionString`（`windows.rs:340-363`、`macos.rs:53`） |
| 健康状态（安装是否完整） | 可读 | 主程序文件存在性 + 进程快照身份复核；官方无更多受支持的健康接口 |
| 诊断日志位置 | 可读（位置）/ **不支持**（内容读取的稳定性） | Electron 用户数据目录（`%APPDATA%/DeepSeek Harness/logs`）可定位，但其内容属官方 userData 私有格式，随官方版本漂移——只建议给用户「打开日志目录」入口，不解析内容 |
| 启动 / 退出（确认后） | 官方允许操作 | 官方壳单实例锁会聚焦已有窗口（`requestSingleInstanceLock`），双击语义无害；退出走 WM_CLOSE/SIGTERM 礼貌路径，与用户手动关闭无异 |
| 单例锁的绕开 / 官方多开 | **不支持** | 多开需 `--user-data-dir` 注入官方进程——属修改官方启动参数，且多份数据目录会分裂官方账号会话；PHL 明确不提供 |
| 读取/修改官方 profile（`~/.dsh/profiles/desktop`） | **不支持** | Electron 独占 profile（自带 lock 文件），写入会与官方抢锁；最多只读，PHL v1 连读都不做 |
| 读取 `.credentials.yaml` / platform-session | **不支持** | 账号会话仅经官方 IPC 内存传递、凭据文件是官方私有格式；跨凭据边界被规划输入 §5 明确禁止 |
| 为实例生成 `profiles/desktop` 以「在实例 home 里开官方窗口」 | **不支持（暂缓）** | 官方 host 协议是内部契约（`hostProtocolVersion: 4`、端口/profile 名硬编码），官方版本升级即可能破坏——没有稳定性承诺就没有安全接口（规划输入 §6-G4） |
| 官方更新器交互（查询/暂停/回滚官方更新） | **不支持** | 官方 nightly feed + electron-updater 生命周期归官方所有；PHL 不读不写官方更新状态（`official/mod.rs:5-8` 模块文档） |
| 官方桌面端诊断信息接入 PHL 诊断导出 | 可读（最小面） | 仅限 §2 已列可读项（版本/安装路径/运行状态）；不解析官方日志内容 |
| macOS 全链路（发现/启动/退出） | 未验证 | 代码就绪（`macos.rs`），真机验收待办（规划输入 §8.5）；macOS CI 只覆盖 Unix 编译面 |

## 3. 单例运行限制与独立测试实例（issue 的研究问题二）

官方桌面端是机器单例：第二次启动由官方壳聚焦已有窗口，多开需要注入
`--user-data-dir`。把官方桌面端做成「PHL 管理的独立测试实例」有三条路，全部不满足
「稳定受支持接口」标准：

1. **注入 `--user-data-dir` 多开**：修改官方启动参数 + 分裂账号会话数据 → 不支持。
2. **为 PHL 实例 home 生成 `profiles/desktop` 并直开官方 host**：依赖官方内部契约
   （硬编码端口/profile/hostProtocolVersion），无版本兼容承诺 → 暂缓，且这正是 v1
   范围决策（§8）把它排除的原因。
3. **读取官方 userData 做状态迁移**：私有格式 → 不支持。

**结论**：社区提出的「官方桌面端独立测试」诉求在官方提供受支持接口之前，PHL 应
明确标注**不支持**，已有的「复制并试用新版」（trial 通路）继续作为独立测试的答案。
这一条已由 v1 的实例模型承担：trial 副本 + `DSH_HOME` 隔离 + `dsh web` 就是官方
CLI 层面的独立测试入口。

## 4. 不能做什么（明确写出，防范围膨胀）

- 不绕开、不破解官方单例锁；不向官方进程注入参数、DLL 或用户数据目录。
- 不读写官方 userData / `.credentials.yaml` / platform-session 等私有存储。
- 不解析官方诊断日志内容（只提供目录入口）；不把官方日志并入 PHL 诊断导出。
- 不劫持官方更新器：不下载、不安装、不回滚官方版本，不把官方版本引入 PHL 版本
  目录（两者可信模型不同：npm sha512 vs 官方 feed 签名，不能互认）。
- 不在无版本兼容承诺的前提下依赖官方 host 内部协议（`hostProtocolVersion`、
  硬编码端口/profile）。
- 探测与退出永远 fail-closed：无法确认身份不操作，礼貌退出失败需用户确认才强杀。

## 5. 结论：是否拆分新的实现 Issue

**暂不拆分。** 逐项核对后，除 v1 已实现的四件事外，没有发现任何「稳定、受支持、
可合法使用」的新接口值得落成实现 Issue：

- 官方没有对外暴露诊断/健康/多实例的受支持接口；能做的最小扩展（「打开官方日志
  目录」按钮）收益有限，可作为小改动并入既有卡片，不值得独立 Issue。
- 「官方窗口接入实例 home」依赖官方内部契约，若官方未来公开 host 协议或提供测试
  实例入口，再依事实拆分。
- 待办观察项两项：macOS 真机验收（已完成代码路径的实机验证）；官方
  `hostProtocolVersion` 变化监测（变化即确认「暂缓」结论仍然成立）。

## 6. 与 PHL 既有约定的衔接

- 「PHL 是遥控器，不是保姆」：`official/mod.rs:1-20` 模块文档的四件事边界与本文
  矩阵一致。
- 探测零缓存：官方 nightly 通道可能在两次轮询之间替换安装——每次现场重探
  （`official/mod.rs:184-198`）。
- 内部规划输入（含 asar 级上游事实与 §8 v1 范围决策）在维护者本地工作区
  （`internal/planning/2026-09-29-official-desktop-integration.md`）；本文只引用
  其结论，不复制官方实现或资产。
