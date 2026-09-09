# 预览版发行审查 · 2026-09-09

## 结论

**公开预览版：NO-GO。内部 Alpha：可继续使用一次性测试环境开展试用。**

功能主体与工程构建已经达到 Alpha 候选水平，但核心承诺中的失败恢复、进程持续管理仍有明确缺陷。阻塞不只是“缺一轮手测”：本次重新审查确认了 4 项 P1 代码问题。应先关闭这些问题，再对同一候选安装包完成干净 Windows 环境验收。

本次只审查和验证，未修改产品代码，未发布 tag/Release，未操作用户的真实实例或凭据。

## 对象与证据口径

- 本地 HEAD：`6caee5bb600f7547725bb6309903c162a45c88e6`；审查开始时工作区干净。
- 版本：`0.1.0-alpha.1`，package.json、Cargo.toml、Cargo.lock、tauri.conf.json 四处一致。
- 远端 main：`ea60b5d3fac9fe322a83a362434612f543b36aa4`。fetch 后逐树比较，差异只有 8 个本地文档文件，产品代码及工作流一致。远端没有 README，属于现有仅代码发布结果，不是本轮改动。
- [当前候选 CI](https://github.com/A7m0spHere/dsh-phl/actions/runs/34249375678) 成功，Frontend 与 Windows Rust 两个 job 全绿；Windows bundle job 因 push 触发被跳过。
- 本轮查询 GitHub Releases 列表为空；未见已发布预览版。
- 下列“静态确认”表示代码顺序或状态推演足以确认缺陷，不表示已经在安装后的应用中实施崩溃/磁盘故障注入。

## 本轮实际验证

| 检查 | 结果 | 边界 |
|---|---|---|
| `npm run test:alpha` | 全部通过 | 版本、typecheck、bridge、Vitest、runner、fmt、clippy、Rust workspace |
| Vitest | 14 文件、84 项通过 | settingsStore 测试有无 localStorage 的环境警告，无失败 |
| 桌面 runner 测试 | 1 项通过 | 测的是隔离目录清理和报告保留，不是真实桌面交互 |
| Rust workspace | 317 项通过、6 项忽略 | 桌面库 281、CLI 3、Pack Core 33；包括本机双卷撤销测试 |
| Bridge | 17 文件、72 个调用、75 个注册 | 无缺失 handler，无重复注册 |
| `npm run build` | 通过 | 主 JS 509.28 kB / gzip 160.50 kB；有分包警告 |
| `npm run app:build` | 通过 | Windows x64 NSIS 本轮重建成功；不是安装后验收 |
| 浏览器生产预览冒烟 | 8 个页面检查通过；控制台检查未全绿 | 六主导航、创建、缺失实例详情通过；唯一观察到的控制台错误为 `/favicon.ico` 404 |
| `npm audit --omit=dev --json` | 0 项已知漏洞 | 只覆盖 npm 生产依赖，不代表 Rust 或供应链全面无风险 |
| `npm audit --json` | 2 个 moderate 包项 | 同一 Vitest/mocker 公告，均为开发依赖；无 high/critical |
| `cargo audit --version` | 工具未安装 | Rust 依赖漏洞审计未完成，不记录为通过 |
| Windows 环境变量独立实验 | 确认大小写覆盖 | Rust Command 先设 `DSH_HOME=isolated-A` 再设 `dsh_home=redirected-B`，子进程读取为 B；仅使用虚构值 |
| `git diff --check` | 通过 | 无空白错误 |

浏览器脚本原使用 Python Playwright，本机两个 Python 环境均未安装该模块；本轮用 bundled Node Playwright 和无头 Chrome 执行等价页面检查。第一次默认 Chromium 启动因对应浏览器二进制缺失失败，随后使用已安装 Chrome。没有把工具环境失败当作产品缺陷，也没有把 favicon 错误隐藏成“控制台零错误”。

NSIS 本轮重建成功，产物 `src-tauri/target/release/bundle/nsis/PHL_0.1.0-alpha.1_x64-setup.exe`，2,909,720 字节，生成于本机 2026-09-09 08:38，SHA-256：`D1064E6B7243DCBFC3D2937AE139F98D41A3CAE75FAA89A0C7C4A775EBC616ED`。干净用户环境的安装、首启、卸载与完整业务链路本轮未执行。历史本机安装记录不能代替这项验收。

## 发行前应关闭的 P1 问题

### R1 · 迁移已经删日志，数据根尚未提交

- 依据：[storage.rs:489](../src-tauri/src/storage.rs#L489)、[SettingsPage.tsx:284](../src/pages/SettingsPage.tsx#L284)、[SettingsPage.tsx:295](../src/pages/SettingsPage.tsx#L295)。
- `move_root_data` 成功时先删除 migration journal，返回前端后才另发请求切换根指针。两次提交之间关闭/崩溃，数据已在目标目录，重启仍读搬空的旧根，且没有 journal 提供恢复入口。切根写盘失败也只能依赖短暂 Toast 提示用户手动改路径。
- 影响是实例看似消失和恢复入口丢失；不是已证明文件被物理删除。
- 修复方向：搬迁完成后保留持久日志，由后端在同一 DataRoot 操作内提交根指针，再清理日志。重启能识别“数据已搬完但根指针未切”的状态。
- 验收：在最后目录落位、根指针提交前后、journal 清理前后逐点中断，重启均能进入正确根或明确恢复入口。
- 证据级别：静态确认；未进行真实 GUI 故障注入。

### R2 · 插件升级中断后，重试可能删除唯一旧包

- 依据：[plugins/install.rs:164](../src-tauri/src/plugins/install.rs#L164)、[plugins/install.rs:193](../src-tauri/src/plugins/install.rs#L193)。
- 旧包先 rename 到 `.phl-old-*`，再放入新包；若两次 rename 之间硬退出，重试会无条件删除 `.phl-old-*`，而新包此时尚未解压成功。随后失败/取消，原插件已无可用副本。
- 当前调用的错误回滚有实现，但未见插件事务的持久化重启恢复；不能用普通异常分支测试证明硬崩溃安全。路线图 O-04 的 Drop/重启恢复说明应随修复校正。
- 修复方向：发现旧备份时先恢复或判定提交状态；新包验证完成前不删除唯一旧副本，记录可跨进程恢复的阶段。
- 验收：旧包搬走后硬退出 → 重启 → 重试并注入解压失败/取消，旧插件内容、标记和 Cordis 注册仍可恢复。
- 证据级别：静态确认；未在用户插件上实施硬退出。

### R3 · 启动取消/超时时，终止失败仍删进程登记

- 依据：[launch/mod.rs:651](../src-tauri/src/launch/mod.rs#L651)、[launch/mod.rs:675](../src-tauri/src/launch/mod.rs#L675)。
- 两处忽略 `kill_tree` 的错误，继续清除内存进程表和持久登记。若 taskkill 失败或超时，仍活着的 DSH 脱离 PHL 管理；再次启动可能遗留第二个进程，重启 PHL 也无法按原记录接管。超时错误还会误称“已终止进程”。
- 普通 `stop_instance` 已改为终止失败保留登记，不能据此认为启动失败清理也已经修复。
- 修复方向：以确认退出作为遗忘登记的前提；终止失败保留身份和可重试停止入口，并反馈实际失败原因。
- 验收：取消启动和 120 秒超时两条路径各注入终止失败；仍存活时记录保持，后续停止成功后再清理。
- 证据级别：静态确认；未执行真实 taskkill 故障注入。

### R4 · 并发进程状态落盘没有串行化

- 依据：[launch/registry.rs:197](../src-tauri/src/launch/registry.rs#L197)。
- `persist` 拷贝 entries 后释放锁，再写固定 `processes.json.tmp` 并 rename。两个实例可并发启动/退出：A 取旧快照，B 取新快照并写完，A 再写旧快照，即可让磁盘丢失活跃登记；同时写固定临时文件还会碰撞。写入/替换错误被忽略。
- 影响：本次会话内内存状态可能正常，但 PHL 重启后无法完整接管仍存活实例，违背多实例运行时管理的核心承诺。
- 修复方向：状态更新、快照与持久化采用一致的串行提交策略；独立临时文件只能解决碰撞，不能单独解决旧快照覆盖。
- 验收：确定性地交错两个实例 remember/forget/persist，并校验落盘结果；重新载入 registry 后和最后内存状态一致。
- 证据级别：代码时序确认；未做真实桌面并发压力试验。

## P2 与其他发现

| 问题 | 触发与影响 | 依据及处置 |
|---|---|---|
| 修改已有供应商密钥后保存失败，旧密钥已被覆盖 | `creds.set` 在 api.json 提交之前更新同一 provider id；后续写盘或另一凭据写入失败，配置保留旧版但密钥已变，错误提示“旧配置与凭据保持不变”不成立 | `api_config/library.rs:78,132,145`；采用版本化凭据引用或可恢复事务，并覆盖“修改已有 key”测试；建议公开发包前一并修复 |
| 快照没有恢复 DSH/Runtime 绑定 | A/Node22 快照 → 在详情页改为 B/Node24 → 恢复，仍用 B/Node24；快照元数据解析后被丢弃 | `instances/snapshot.rs:33,270,326`、`InstanceDetailPage.tsx:450`；恢复绑定或把 UI 明确限定为 dsh-home 内容恢复 |
| 快照交换中断无自动恢复 | 备份 current 后、放入快照前退出，会缺少 dsh-home；再次恢复直接拒绝，旧数据在隐藏目录 | `instances/snapshot.rs:276,315,318`；纳入 R2 同类持久恢复设计，保留与识别旧目录 |
| 快速重启可能复用旧 WebUI URL/token | 同实例窗口已存在时只 focus，忽略新 URL；接管 watcher 的 2 秒检测窗口允许新进程登记后保留旧窗口 | `webui.rs:85`、`launch/mod.rs:387`；窗口绑定启动代次并校正 URL；桌面时序尚未实测 |
| Windows DSH_HOME 大小写绕过 | 本地 manifest 的 `dsh_home` 能覆盖隔离目录；Bundle 导入已有大写过滤，不能描述成 Bundle 漏洞 | `launch/mod.rs:590`；保留变量统一比较，并在最终环境构造时保证隔离值；底层覆盖行为已独立实测 |
| PID 身份判定过宽 | 相同 node.exe 在十分钟内复用 PID 可通过判定；创建时间缺失时也继续接管，该判定还用于停止 | `launch/registry.rs:57,91`；保存精确内核创建身份，缺失时拒绝自动托管；真实 PID 复用未复现 |
| 验收隔离模式覆盖不到关键持久链路 | `PHL_ROOT` 令 pointer=None，因此 processes.json 与 migration journal 不持久化 | `paths.rs:127,133,166`、`lib.rs:288`；建立独立持久配置目录的测试模式或干净 Windows 用户，现有 runner 不能证明重启接管/迁移恢复 |
| 浏览器 favicon 404 | 页面可用，但零控制台错误检查失败 | `/favicon.ico`；低优先级资源补全，非独立发行阻塞 |

## 发布工程与用户交付

- 已具备版本一致性校验、Windows CI、NSIS 构建、tag 版本校验、SHA-256 产物与 prerelease 标记。它们证明发布机制主体存在。
- `release.yml` 在 `workflow_dispatch` 时未区分“只构建”与“创建 Release”，末尾发布步骤仍运行且没有显式 tag_name；常规分支手动运行不能作为可靠的只构建演练。现有 CI 的手动 bundle job 可承担构建演练。正式发布前应验证受控 Alpha tag 对应安装包、哈希和 prerelease 标记。
- Release 流程只运行 npm test/build 和 Rust workspace test，未复用 CI 的 bridge、runner、fmt/clippy 全套门禁，也未验证 tag 对应提交已经通过 CI。建议复用同一 gate 或强制候选 CI 成功，避免 tag 绕过完整检查。
- 本轮最新 CI 的 bundle job 为 skipped，不能把该次 CI 成功写成“干净 runner 已打包并验收”。
- 远端缺少 README；本地 README 仍以开发者构建流程为主，并把可执行文件写成 `PHL.exe`，实际 Cargo 二进制为 `dsh-phl.exe`。公开发行需在可见的发行说明提供安装路径、支持系统、首次创建/启动、数据位置、更新方式和问题反馈入口。
- 仓库未选定 LICENSE，本地 README 也明确将此列为发布前维护者决策。这里记录交付状态，不作法律结论；需要明确本项目与随包依赖的使用/分发说明。
- 本机历史安装显示 `installMode: both` 静默安装可能走按机器安装并忽略 `/D=`；该行为应进入目标用户安装验收和发行说明，而不能仅凭退出码 0 判定通过。
- 自动更新、代码签名、托盘/开机启动、跨平台、主 JS 分包不是 Windows Alpha 的单独硬阻塞；应说明未提供的能力，不必为预览版补齐全部路线图。
- npm 开发依赖的两个 moderate 包项来自同一 [Vitest/mocker 公告](https://github.com/advisories/GHSA-82fw-gwwq-j7x9)，涉及开发服务的 redirect mock 文件读取；本项目使用 `vitest run`，未据此确认打包后的桌面应用可被该漏洞利用。升级与 Rust 依赖审计列入发布卫生项，不能把 npm 生产审计为零写成“全项目无漏洞”。

## 放行标准与建议顺序

1. 先修 R1–R4，并把已有凭据覆盖与 Windows 保留环境变量处理一并收口；所有恢复承诺和测试必须与实际行为一致。
2. 修复窗口代次和快照语义；若 Alpha 暂不支持完整环境回滚，直接缩小并明确界面承诺。未完成的高风险功能可先禁用，但要真正从可执行入口收口。
3. 用独立、持久的测试配置环境完成：迁移中断继续/撤销、插件交换硬退出、取消/超时终止失败、双实例并发启停、重启接管后退出、旧退出晚于新启动。普通成功路径和返回 Err 的测试不能替代硬退出场景。
4. 在干净 Windows 用户环境安装同一候选：首启 → 下载 Node/DSH → 创建两个隔离实例 → 配置 API → 插件实际加载 → 启停 → 克隆/快照 → Pack 导入导出 → 重启接管 → 卸载；同时检查数据保留、非管理员安装和 WebView2 前提。
5. 固定候选提交、记录安装包 SHA-256、确认该提交的 CI 与上述验收通过，补齐发行说明和许可决策后发布 `v0.1.0-alpha.1` prerelease。若候选变化，复验受影响路径并重新生成产物记录。

上述工作以关闭可靠性缺陷和补齐证据为目标，不要求在预览发行前新增大型功能或重构架构。
