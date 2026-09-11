# PHL 手测清单

> 覆盖范围：Runtime Manager → 启动/进程 → Bundle → 快照 → 诊断 → 故障恢复与冲突（§10）
> → 本机 DSH 接入（§11）→ 对话迁移（§12）→ 整合包 `.phlpack`（§13）→ `phl-pack` CLI 与 Skill（§14）。
> 测试方式：`npm run app:dev`（或双击 `run-dev.cmd`）打开桌面端。
> 建议顺序按编号走，前面的结果是后面的前置条件。
> 标注 ⭐ 的是必须通过的核心路径；其余是边界与守卫。
> §11–§13 对应《下一阶段开发规格》§32 测试矩阵与 §33 隐私测试；Windows 之外平台的
> 发现路径（macOS/Linux 的 npm/Homebrew bin 目录、GUI 启动 PATH 限制）属 L-12 专项。

## 0. 准备

- [ ] ⭐ `npm run app:dev` 能编译并打开窗口；窗口无白屏闪烁（先画完再显示）
- [ ] 设置 → 通用：确认数据目录（默认 `%LOCALAPPDATA%\PHL`），记下来，后面要对照文件系统

## 1. Runtime Manager（Node）

- [ ] ⭐ 「运行时」页：目录正常加载，显示多个 Node 大版本（LTS 标记、代号），排序从新到旧
- [ ] ⭐ 安装一个 Node 22：进度条有真实速度，完成后行内显示「已安装 · 真实大小」（去数据目录看 `runtimes/node-22` 存在）
- [ ] ⭐ 重启 PHL：该 Runtime 仍是已安装状态
- [ ] 安装中途点「取消」：状态回到「未安装」，缓存里无 `.part` 残留（有也没关系，诊断能看见）
- [ ] 取消后再次安装：能成功
- [ ] 删除 Runtime：数据目录里 `runtimes/node-22` 整个消失
- [ ] 机器装了 Node 的话：列表出现「系统 Node」，版本号与终端 `node --version` 一致

## 2. Version Manager（DSH）

- [ ] ⭐ 「版本」页：目录加载（npm + GitHub），有版本、频道标记
- [ ] ⭐ 下载一个 DSH 版本：进度 / 取消 / 重试可用；完成后状态为已安装
- [ ] 重启 PHL：已安装状态保持
- [ ] 设置 → 下载：开启「保留压缩包」再装一个版本，缓存里出现 `.tgz`；关掉则无

## 3. Instance Manager

- [ ] ⭐ 创建实例向导走完：版本选刚装的 DSH，Runtime 选刚装的 Node 22 → 创建成功
- [ ] ⭐ 检查磁盘：`instances/<id>/` 里有 `instance.json`、`dsh-home/profiles/web/node_modules`、`workspace`、`logs`；profile 固定为 `web`
- [ ] ⭐ 重启 PHL：实例还在
- [ ] 改名、改端口、加环境变量/启动参数：重启后修改仍在
- [ ] 克隆实例：新实例出现；两个实例的 `node_modules` 相互独立；克隆不带原实例的快照（快照节先跳过，做完 §7 回来验证）
- [ ] 删除实例：整棵目录树消失
- [ ] 删除守卫：running/starting/stopping 状态下删除被拦截
- [ ] 端口：实例 A 固定 3080，实例 B 也固定 3080 → B 启动报「端口已被实例 A 占用」；B 开自动分配则跳过占用端口

## 4. Plugin Manager

- [ ] ⭐ 插件市场加载（真实注册表）；断网时显示离线提示而不是白屏
- [ ] ⭐ 给实例装一个插件：装完立刻出现在实例详情的插件列表（由磁盘反推）
- [ ] 启用 / 禁用：实例目录 `dsh-home/profiles/web/cordis.patch.yml` 有对应变化
- [ ] 卸载：`node_modules` 里包目录消失，列表同步
- [ ] 插件安装进行中：实例的「创建快照」被拒绝（提示等安装完成）

## 5. 启动与进程（核心）

- [ ] ⭐ 启动实例：七个阶段进度推进，结束弹「已就绪」toast，端口为分配的端口
- [ ] ⭐ 浏览器能打开 `localhost:<port>`（toast「打开」按钮、详情页「打开 WebUI」都试一下）
- [ ] ⭐ 实例 `logs/` 下出现 `launch-*.log`，内容是 DSH 的输出
- [ ] ⭐ 停止实例：WebUI 立刻不可访问；任务管理器里没有残留的 node 进程（整棵树被杀）
- [ ] 杀进程测试：实例运行中，在任务管理器里直接杀 node → PHL 自动把状态变回「已停止」并提示
- [ ] 启动中途点取消：进程被终止，无残留
- [ ] 用别的程序占住实例固定端口再启动：报错信息清晰（非 PHL 实例占用）；PHL 实例间占用应带 `[port-conflict]` 前缀并可引导自动端口
- [ ] 关闭 PHL 主窗口：确认框列出运行中的实例；确认后退出
- [ ] 实例的「打开实例目录」：资源管理器打开正确目录

## 6. Bundle

- [ ] ⭐ 实例菜单「导出 Bundle」：保存对话框 → 生成的 JSON 包含 version / runtime / profile / env / args / 插件记录；含密钥值的 env 变量被剔除并列入「待补凭据」（见 §10）
- [ ] ⭐ 实例页「更多导入方式 → 导入 Bundle」：选文件 → 预览确认框显示名称/版本/插件数 → 导入成功出现在列表
- [ ] 导入后的实例：id 是新的、端口是建议端口；版本/Runtime/profile 等环境字段与导出时一致
- [ ] 导入含插件记录的 bundle：toast 提示「N 条插件记录待重装」；插件列表为空（文件不随 bundle）
- [ ] 喂一个损坏 / 手改 `phlBundle: 99` 的 JSON：报「不支持的 Bundle 版本」，不产生半成品实例

## 7. Snapshot

- [ ] ⭐ 实例停止状态下创建快照：卡内出现复制进度条，完成后列表多一条（时间 / 插件数 / 大小）
- [ ] ⭐ 装一个新插件 → 回滚到快照：新插件从列表消失；`node_modules` 里包目录消失
- [ ] 回滚后快照仍在：可以再回滚一次（存档不被消耗）
- [ ] 回滚 / 删除快照都有确认框；删除后磁盘上 `snapshots/<id>` 消失
- [ ] 实例运行中：创建快照 / 回滚 / 删除快照全部被拒（Rust 守卫）
- [ ] 重启 PHL：快照列表还在
- [ ] 克隆验证（接着 §3）：克隆体不带快照历史

## 8. 诊断

- [ ] ⭐ 设置 → 诊断：进入自动检查；健康状态下全部绿点
- [ ] 人为弄坏一个版本（删掉 `versions/<v>/lib/bin.js`）→ 诊断黄点并点名「缺少 lib/bin.js」
- [ ] 卸载某个实例引用的版本 → 诊断点名「版本 … 未安装」
- [ ] 缓存有内容时：显示占用 + 「清理下载缓存」按钮 → 清理后 toast 显示释放字节数，检查项变绿
- [ ] 孤立目录（手工造一个无 `instance.json` 的目录）→ 诊断可见，存储页可回收

## 9. 存储与杂项

- [ ] 设置 → 存储：实例 / Runtime 占用与资源管理器里看到的实际大小一致
- [ ] 下载源切到「国内镜像」：Node / DSH 目录与下载都能走镜像
- [ ] 快照复制进行中删除该实例：复制被中止、无报错堆栈
- [ ] 浏览器模式 `npm run dev`：一切照旧走 Mock（启动是演示流程），设置 → 诊断显示「仅桌面端可用」，不崩

## 10. 故障恢复与冲突回归（优化轮新增，对应路线图 O-04 ~ O-09）

前置：至少一个可运行的实例、一个已装插件、可写第二数据目录（如同机另一分区或 `D:\PHL-mig`）。
每个场景完成后在下表登记结果；失败必须附错误码（消息前缀 `[code]`）与 `logs/` 片段。

- [ ] ⭐ **冲突互斥**：实例 A 正在安装插件时，同时对实例 A 发起快照 → 后发起者立即收到
      `[busy] 实例 <实例id> 正被另一个操作占用，请等待其完成后重试`（id 是实例的内部标识，
      不是显示名）；等安装结束后重试快照成功。
- [ ] ⭐ **迁移进行中被重启**：设置 → 存储 迁移数据目录，进度过半时强杀 PHL（任务管理器结束进程）
      → 重开 PHL：出现「检测到未完成的数据目录迁移」提示；进入存储页有横幅，数据目录指针未变。
- [ ] ⭐ 上条之后点「继续迁移」：从记录处续跑，完成后新旧目录数据总量与迁移前一致；重启 PHL 一切正常。
- [ ] 替代路径：恢复横幅里点「撤销并还原」→ 已复制目录原路退回旧根，横幅消失，列表恢复旧目录。
- [ ] ⭐ **进程接管**：实例运行中 → 正常退出 PHL（选择不停止实例）→ 重开 PHL：
      实例显示为运行中，「打开 WebUI」可用且能打开原端口页面，不重复启动，停止按钮可正常杀掉进程。
- [ ] 接管边界：实例运行时用任务管理器结束 PHL，再手动结束那个 node 进程，然后重开 PHL →
      不弹「已接管」，实例显示为已停止，端口未被幽灵占用。
- [ ] PID 复用守卫（可选，较难复现）：接管提示里出现「已被其他程序复用」的实例，PHL 不得显示运行中也不得允许停止。
- [ ] ⭐ **凭据失败回滚**：API 配置里新增一个供应商并填入密钥 → 断开凭据服务（或用假账户）→ 保存失败：
      旧配置原样保留、旧供应商密钥仍可用；恢复后重试保存成功且新密钥落库。
- [ ] 删除已保存的供应商：保存成功后其凭据条目才消失（Windows「凭据管理器」里 `PHL:provider:*` 对照）。
- [ ] **设置真实性**：通用/下载/高级分区中「启动时」「最小化到托盘」「自动检查更新」「同时下载数」「日志级别」
      均显示为禁用并注明未接入；改「端口分配起点」为新实例建议端口（重启生效）。
- [ ] Bundle 密钥边界：实例 env 里放一个自造 `MY_TOKEN=abc123` → 导出 Bundle → 文本里 grep 不到 `abc123`，
      导出预览列出「待补凭据」；导入端提示重新配置。
- [ ] 下载中断恢复（可选）：版本下载中途取消 → 再次下载显示从断点继续（速度先跳一段）；
      取消后源文件被替换的场景（需可控服务器）不常见，可跳过，由 `versions::download` 单测覆盖。

### 回归结果登记

| 日期 | PHL 版本/提交 | 系统 | 场景编号 | 结果 | 备注（错误码 / 日志） |
|---|---|---|---|---|---|
|  |  |  |  |  |  |

## 11. 本机 DSH 发现与接入（Discovery + Adoption，规格 §28）

前置：这台机器上存在至少一个真实的、已在用的 DSH（自带插件与历史对话最佳）。若只有 PHL
管理的实例，用「设置 → 通用」记下数据目录后，手动在别处装一个 DSH（`npm i -g` 或已有
`~/.dsh`）。§32 Discovery/Adoption 段。

- [ ] ⭐ **自动扫描**：实例页「接入本机 DSH」→ 发现步骤列出候选；默认 `~/.dsh` 与 PATH 上的
      DSH 命令各成一卡，显示版本 / 插件数 / 历史对话数 / 体积 / 来源徽标；无 macOS/Linux 死代码。
- [ ] ⭐ **手动选择 DSH_HOME**：选一个合法 DSH 目录 → 生成候选卡；选一个非 DSH 目录 →
      toast「该目录不是可接入的 DSH」，不入库。
- [ ] **手动选择可执行文件**：选 `dsh` 命令 → 按规则推断 home，卡上标注「可执行文件为手动选择；
      DSH_HOME 按…推断」。
- [ ] **去重**：同一 home 同时被默认扫描与 PATH 命中 → 只出现一张卡（手动来源优先）。
- [ ] **已被管理检测**：选一个 PHL 实例自己的 `dsh-home` → 卡片置灰并注明「已由「实例名」管理」；
      「下一步」被拦截；即便绕过 UI，后端 `adopt_instance` 仍拒绝（双主守卫）。
- [ ] **重新扫描**：改动 `~/.dsh`（装个插件）后点重扫 → 数字更新，选中项不被打乱。
- [ ] ⭐ **Copy 模式（推荐）+ 全部迁移**：预览显示版本/插件/对话数/预计复制量/符号链接数 →
      复制并接入 → 新实例出现在列表，`instances/<id>/dsh-home/` 完整；**原 DSH 目录一字未动**
      （对照：`settings.yaml`、`sessions/` 仍在原处）。启动该实例 → DSH 里能看到原来的历史对话。
- [ ] **Copy 模式 + 不迁移**：预览标注「不含历史对话」；接入后实例 `dsh-home/sessions` 不存在、
      `storages/session_projcache` 被排除，但 `storages/workspace.json` 与 profiles 照常进来。
- [ ] ⭐ **Copy 模式 + 按对话选择迁移**：勾「选择对话」→ 出源对话清单（id/ cwd / 时间 / 子代理标记）
      → 选 1 条（或几条）接入 → 实例 `dsh-home/sessions` 只含选中的目录（原 id 保留），未选的不进；
      空选被前端拦（「预览」禁用）+ 后端 `[state] 未选择…`；清单里一个已消失的目录提交 → 后端拒绝并回滚。
- [ ] **符号链接插件**：源里放一个 `link:` 本地插件 → 预览提示「含 N 个符号链接，跳过」；接入后
      该插件缺失但可重装，不跟随链接进外部目录。
- [ ] ⭐ **External（原地接入）**：预览常驻「原地接入会继续使用现有 DSH_HOME，历史对话保持可见」；
      接入后不复制目录树（`dsh-home/` 不存在，home 指向用户目录）；实例可启动、可读插件，但
      克隆 / 创建快照 / 快照还原 / 插件写 / API 同步 全被拒；「删除实例」文案为「从 PHL 移除」，
      删后**绝不**删除用户的 DSH_HOME。
- [ ] **中途失败回滚**：Copy 大环境复制中途点取消 → 无半成品实例（`instances/.phl-adopt-*` 已清），
      原 DSH 不变；重开接入向导能干净重来。
- [ ] **凭据边界（Copy）**：接入后的实例 `api` 绑定为未绑定（不套全局库），沿用自带的 `settings.yaml`
      —— 复制模式按设计保留用户自带配置，与 `.phlpack` 导出的强制剥离不同（规格 §12 边界，见 §13）。

## 12. 对话迁移 / 分发（Session copy，规格 §29 P1-1）

前置：至少两个 Copy 模式受管实例（A 有若干历史对话，B 为空），两者都**未运行**。§32 Session 段。

- [ ] ⭐ **单条复制到单目标**：A 详情「数据 → 对话迁移」→ 选 1 条 → 目标勾 B → 复制 → 确认框
      说明「每个目标得到全新对话，保留内容与工作目录、记录血缘」→ 成功后 B 启动 DSH 可见该对话；
      **A 的原对话不变**；B 里新对话 id 是新的、`parentSession` 指向 A 的原 id、`cwd` 一致。
- [ ] **多目标 fan-out**：同一对话复制到 B+C → 两个目标各得一条独立新会话。
- [ ] **多源**：一次勾多条对话复制 → 每条都进目标。
- [ ] **子代理会话**：list 里带「子代理」标记（展示但不特殊处理，规格 §9 留待真机反馈）。
- [ ] ⭐ **运行中保护**：目标实例运行中 → 不出现在可选目标里；源运行中 → 「对话迁移」面板要求先停；
      绕过前端直发命令 → 后端 `ensure_not_running` 拒绝。
- [ ] **external 目标拒写**：原地接入的实例不作为复制目标（后端也拒绝向 external home 写会话）。
- [ ] **源不受损**：任一复制后，源 `sessions/` 目录内容字节不变。
- [ ] **兼容版本护栏**：伪造一个 `version != 0` 的会话 header → 复制报 `UnsupportedVersion`，
      不产出半条会话（staging `.part` 被清）。
- [ ] 复制后计数：A 与 B 的「历史对话」计数即时更新（无需重开页面）；目标重启 PHL 计数仍对。

## 13. 整合包 `.phlpack` 导出与安装（Pack，规格 §29 P1-2~4 + §33 隐私）

前置：一个已管理的 Copy 实例（带插件、带历史对话）；同机第二数据目录便于验证安装落点。§32 Pack 段。

### 导出

- [ ] ⭐ **预览**：实例菜单「导出整合包」→ Preview 列 DSH / Node / 在线插件数 / 内置插件数 /
      历史对话「不包含」/ 敏感数据「已排除」/ 预计大小；本地插件默认建议嵌入且列出许可，
      `license unknown` 的显示「PHL 无法确认是否允许重新分发」提示（不判合法）。
- [ ] ⭐ **不含会话导出**：确认默认（不含对话）→ 选保存路径 → 生成 `<x>.phlpack`；
      用 `phl-pack validate` 能过；ZIP 内含 `phlpack.json` + `embedded/plugins/*`，
      无 `sessions/`，manifest `content.secretsExcluded=true`。
- [ ] ⭐ **会话隐私门禁**：勾「包含历史对话」→ 必须先弹隐私警告（列出用户输入/文件内容/项目路径/
      Tool 结果/私有代码/Token），二次确认后才放行；未确认直接导出 → 后端 `[state]` 拒。
- [ ] **凭据剥离（§33，值层）**：实例 env 放自造 `OPENAI_API_KEY`/`DEEPSEEK_API_KEY`/`MY_TOKEN` →
      导出（含会话）→ 对生成的 `.phlpack` 逐条目文本 grep **搜不到任何密钥值**；manifest 列出被剥离的
      凭据**名字**；会话正文里若本身含 Token，PHL 不假装能识别，仅靠「包含会话必须提示隐私风险」兜底。
- [ ] ⭐ **凭据剥离（§12/§33，文件层）**：给一个将被嵌入的本地插件目录里放一个 `.env`（含
      `SECRET=abc`）与一个 `id_rsa` → 导出预览的 warnings 点名「插件 X 含疑似凭据文件，无论是否嵌入都不打包」
      → 导出 → `phl-pack unpack` 出来的包里 **没有** `.env`/`id_rsa`，导出成功 toast 列出「已扣下」；
      同时确认正常文件（`index.js`/`package.json`）照常进包（过滤是文件名级，不误伤 `tokenizer.js` 等）。
- [ ] **CLI 同规则**：`phl-pack build` 一个布局目录，内放 `.env` 与 `.env.example` → 输出「已扣下 …/.env」
      且包内无 `.env`，但 `.env.example` 照常进包（模板文档不算凭据）。

### 安装

- [ ] ⭐ **依赖检查 + 一键装**：实例页「安装整合包」→ 选/拖放 `.phlpack` → Import Preview 显示
      作者/版本/DSH/Runtime/插件(在线/内置)/历史对话数（含「⚠ 包含用户对话数据」若带会话）→
      检查依赖列出 `installed/downloadable/embedded/missing` → 安装 → staging 原子落位 →
      新实例出现在列表，embedded 插件已就位，可选会话进入 `dsh-home/sessions`。
- [ ] ⭐ **缺 DSH 不静默升级**：包引用本机没有的 DSH 版本 → 预览标 downloadable + 「找不到 DSH xxx，
      用兼容版本可能有兼容问题」，允许先建实例后补，**不自动改版本**（规格 §19）。
- [ ] **缺 required 插件**：required 且无任何来源的插件 → `blocked`，默认阻止安装；可选缺失 → 可跳过继续。
- [ ] **远程插件延后**：registry 插件在提交后由 UI 逐个走正常插件安装管线，不在 pack 事务内联网装。
- [ ] ⭐ **中途失败回滚**：解包阶段制造失败（如把 embedded 目标设成只读盘）→ 删除 staging，
      不留半个实例（`instances/.phl-pack-*` 已清）。
- [ ] **安全边界（§22）**：手造含 `../` 条目 / 绝对路径 / 软链 / 重复条目 / `formatVersion:999` /
      篡改某 embedded 文件使其与 integrity 不符 的 `.phlpack` → 逐一被 `read_pack` 拒（越界/软链/重复/
      超大/版本/一致性），且**拒绝发生在写盘之前**。
- [ ] **完整性篡改检测**：装完后手改包里某 embedded 文件 1 个字节再校验 → integrity 报「校验值与内容不符」。
- [ ] **schema 版本护栏**：manifest 用更高的 `schemaVersion`/`formatVersion` → 老 PHL 拒绝、不猜测解析。

## 14. `phl-pack` CLI 与 phl-export Skill（P2，规格 §15/§30）

前置：`cargo build -p phl-pack-cli` 产出 `target/debug/phl-pack(.exe)`，`phl-pack --help` 可用。

- [ ] ⭐ **CLI 闭环**：`phl-pack build <布局目录> out.phlpack`（含 phlpack.json + embedded + sessions
      布局）→ `validate` 通过 → `inspect` 摘要可读 → `unpack out.phlpack <目标>` 全载荷落到目标目录，
      且每个写点被限制在目标目录内（dest-root confinement）。
- [ ] **CLI 拒绝无效包**：`validate`/`inspect` 对缺 manifest、坏 JSON、越界条目、integrity 不符 均报错，
      `build` 缺 `phlpack.json` 或声明的 embedded path 无 package.json 时报 consistency，绝不产坏包。
- [ ] ⭐ **Skill 与 GUI 同格式**：一个用 `phl-export` Skill 产的 `.phlpack`，能被 PHL「安装整合包」正常
      导入；反向，GUI 导出的包能被 `phl-pack inspect` 读懂（一套格式，两个入口不漂移）。
- [ ] **Skill 敏感确认**：走 `docs/skills/phl-export/SKILL.md`：包含会话/本地插件/未知文件/覆盖输出
      逐项询问，不发明格式、不手算哈希（全交 CLI）。

### §11–§14 回归结果登记（并入 §10 同表亦可）

| 日期 | PHL 版本/提交 | 系统 | 场景编号 | 结果 | 备注（错误码 / 日志） |
|---|---|---|---|---|---|
|  |  |  |  |  |  |

## 15. 2026-09-07 Review 收口验收（自动 / 真机边界）

以下项目已由本机自动化或浏览器 Mock 验证，记录在这里，避免真机重复验证实现细节：

- [x] **M3 / M5 模块边界**：catalog store 已按 version/runtime/plugin actions + 通知策略组合；
      API config 已拆为 types/validation/launch_keys/tests；外部调用面不变。
- [x] **R1 / R3 / R4 / R5 自动路径**：凭据文件剥离、embedded 插件 marker + Cordis 登记、
      安装失败返回预览、更新查询失败/重试/去重/不可检查状态均有测试并通过。
- [x] **R7 协作式取消**：扫描、压缩、完整性哈希、解包在 64 KiB 循环内检查取消；
      大文件中途取消测试通过，解包半成品会删除；导出/安装 UI 已提供取消按钮。
- [x] **R6 可复现基准**：64 MiB + 1000 小文件、基线/当前 release 各 3 次，数据记录在
      `PERF_BASELINE.md`；三条 Pack 路径峰值工作集下降 95% 以上。
- [x] **M2 bridge 静态门禁**：`npm run bridge:check` 检查 17 个 bridge 文件 / 71 个 invoke，
      无缺失 Rust handler、无重复注册。
- [x] **浏览器 Mock 回归**：设置分区、实例创建、插件搜索/详情/安装/启停/更新页可用；
      修复嵌套 button 与 Toast ref 后，控制台无 warning/error，DOM 无 `button button`。
- [x] **自动门禁和构建**：前端 73 项、Rust workspace 268 项通过 / 5 忽略，typecheck/build/
      clippy/fmt/bridge check 全绿；NSIS 安装包成功生成。

以下项目必须在真实 Tauri + 真实 DSH 或 GitHub runner 上完成，不能用浏览器 Mock/单测代替：

- [ ] ⭐ **安装包冷启动**：在干净 Windows 用户环境安装本轮 `PHL_0.1.0_x64-setup.exe`，确认首帧、
      自绘标题栏、关闭确认、文件对话框和卸载流程正常。
- [ ] ⭐ **R1 + R7 真实导出/取消**：真实实例含嵌套 `.env`/`token.json`，导出后解包核对排除；
      另用大包在压缩中点「取消导出」，确认 UI 可响应且目标路径无残缺 `.phlpack`。
- [ ] ⭐ **R3 真实插件加载**：安装仅含 `package.json` + 源码、无 `phl-plugin.json` 的 embedded 插件；
      确认插件页可见、Cordis 登记存在，真实启动 DSH 能加载，并回归启用/停用/卸载。
- [ ] **R4 失败返回视觉流程**：制造缺必需依赖的 Pack 安装失败，点「返回修改」，确认包路径/实例名保留，
      焦点与错误提示正常，再次安装成功。
- [ ] **R5 真实网络状态**：可更新页断网显示「检查失败 · 可重新检查」，恢复网络后成功；
      非 npm 来源显示「来源无法检查」。请求去重已由单测覆盖，真机只验 UI 与真实 registry。
- [ ] ⭐ **M2 Tauri IPC 冒烟**：逐域操作窗口控制、版本/Runtime/插件进度 Channel、launch/stop/adopt、
      快照、Bundle/Pack、API 同步与模型补全、诊断、系统文件对话框，确认参数与权限在真实壳内工作。
- [ ] **M1/M4 + R6 真机性能**：用 React Profiler 检查 Settings/Plugins 不因无关 toast/confirm/进度而整树
      重渲染；大包导出/解包时窗口可拖动、可取消；记录桌面冷启动与峰值内存。
- [x] **R2 GitHub Windows CI / Release**：先收口本地与远端分支，再触发 Windows runner，确认三 crate
      测试与短路径环境通过；之后做受控 tag 演练。未获明确授权前不提交、不推送、不创建 PR/tag。
      **已关闭（2026-09-11）**：公开提交 `6ba04c7` CI 两 job 全绿（run `34550242283`）；受控 tag `v0.1.0-alpha.4` 的 Release
      run `34550553053` 成功，安装哈希、minisign 验签与更新清单核验见 [docs/alpha-release-acceptance-2026-09-11.md](docs/alpha-release-acceptance-2026-09-11.md)。

## 16. 统一启动焦点与内嵌 WebUI 窗口（2026-09-08）

前置：`npm run app:dev` 真实桌面模式；至少两个已创建实例；真实 DSH 数据目录可启动。

### 焦点模型（launcher 点击语义）

- [x] 单击实例卡片 = 设为启动目标：卡片出现 accent 焦点环 + 色条点亮 + 「启动/详情」常驻；**不**跳转详情
- [x] dock target 与卡片焦点始终一致；启动/详情页切换实例时 dock 跟随（`InstanceDetailPage` 进入即 `setFocus`）
- [x] 双击卡片 = 启动（启动中再双击 = 停止；失败态双击 = 清错重试）
- [x] 卡片 hover/焦点态出现「详情」按钮（↗）进入详情页
- [x] 运行中的实例在列表自动置顶（原有排序规则保持）

### 内嵌 WebUI 窗口（`src-tauri/src/webui.rs`）

- [x] 启动成功自动打开内嵌窗口，直达带 token 的 `dsh web` URL（非登录页/白屏）
- [x] 关闭 WebUI 窗口**不停止实例**：dock 仍显示运行中，「打开 WebUI」重建同 label 窗口
- [x] 窗口已开时再点「打开 WebUI」= 仅聚焦，不新建（按 label 幂等）
- [x] 双实例双窗口并存互不串（11/22 各开一窗，标题为实例名）
- [x] 停止实例 → 对应窗口自动关闭，其他实例窗口不受影响（watcher 联动）
- [x] 非 loopback / 带凭据 / 非法 scheme 的 URL 被 `ensure_loopback` 拒绝（单测 ×3）
- [x] WebUI 窗口 × 正常关闭，不触发 PHL 退出确认（`CloseRequested` 仅 main 拦截）
- [ ] 进程崩溃（外部 kill node）→ 窗口随 watcher 关闭 + 崩溃 toast（机制同 stop 路径，未单独真机复验）
- [ ] 浏览器模式（`npm run dev`）回归：无内嵌窗口能力，「打开 WebUI」降级新标签页，不弹错误

### 回归结果登记（2026-09-08 真机）

- Windows 11 / dev 构建 / 实例 11（0.1.2-rc.1）+ 22（0.1.2-alpha.5），焦点、双击启动、自动开窗、
  关窗存活性、重建、停止联动关窗、双窗口隔离全部通过；`cargo test --lib webui` 3/3、vitest 75/75、typecheck 通过。
- **实现约束（tauri#3597）**：Windows 上创建 webview 窗口的 command 必须是 `async`——
  同步 command 会与事件循环死锁，表现为「窗口出现但永久白屏、devtools 也打不开」。新增窗口类 command 时注意。

## 17. Alpha 自动验收体系（2026-09-08）

### 命令

- `npm run test:alpha` — 确定性 gate：version check · typecheck · bridge:check · vitest · desktop runner tests · cargo fmt/clippy(-D)/workspace tests。
  一次跑完全部步骤并汇总 PASS/FAIL（fail 不中断 sweep）。CI 可完全复用。
- `npm run test:desktop` — GUI 层：以 `PHL_ROOT=<temp>/phl-alpha-desktop-<ts>` 隔离 root 启动 `tauri dev`，
  场景清单见 `scripts/alpha-desktop-scenarios.json`，结果用 `scripts/alpha-desktop-record.mjs` 落
  report JSON（生成在 root 旁，正常清理后仍保留；KEEP 用 `PHL_ALPHA_KEEP=1`；手动指定的
  `PHL_ALPHA_ROOT` 始终保留）。**真实 `root.json` 指针全程不被触碰**
  （paths.rs：`PHL_ROOT` 优先且无 pointer → 一切持久化文件都进不了 `%APPDATA%\PHL`）。

### 层一新增链测试（本次补齐的缺口）

| 链 | 测试 | 位置 |
|---|---|---|
| Root 隔离 | `phl_root_env_overrides_and_persists_nothing` | `src-tauri/src/paths.rs` |
| A 克隆字节不变量 | `clone_diverges_from_source_at_the_byte_level`（tree fingerprint 全链） | `instances/mod.rs` |
| B 运行守卫 | `snapshot_operations_refuse_a_running_instance`（三入口 + 非粘滞） | `instances/mod.rs` |
| C external 写保护 | `external_instances_survive_every_gated_write_attempt_untouched`（home+instance 双字节一致） | `instances/mod.rs` |
| D 插件 id 白名单 | `registry_ids_are_whitelisted_not_normalized` | `plugins/resolve.rs` |
| E pack 全链 e2e | `export_scan_install_roundtrip_keeps_secrets_out_and_residue_clean`（整包解包逐文件扫 secret 值 + B root 安装 + 半包损坏零残留） | `pack/install/tests.rs` |
| G 启动 drift | `launch_observes_managed_drift_without_touching_settings`（三态：无 drift/drift/短路） | `api_config/tests.rs` |
| H 陈旧 watcher | `a_stale_pid_never_removes_the_fresh_process_entry` | `launch/mod.rs` |

### 本次修复（发现→回归测试→最小修复→重验）

1. **pack 安装凭据台账丢失**（`pack/install.rs`）：install 只按目标 root 的全局 config 识别凭据名，
   新机安装报 `credential_names: []`，前端「需重新配置密钥」提示消失。修复：union pack 自带
   `bundle.credentials`。回归 = E e2e 断言。
2. **崩溃零留痕**（`instanceStore` + `InstanceCard` + `InstanceDetailPage`）：ready 后进程退出只有一条
   会过期的 toast，UI 无声回到「已停止」。修复：runtime state 记 `lastExit{code,at,ranFor}`，卡片与
   详情页常驻「上次退出 N」badge，下次启动自动清除。回归 = vitest ×2；真机（desktop alpha pass）验证
   badge 在 fs-ext 崩溃后正确出现。
3. **克隆失败全静默**（`instanceStore.cloneInstance`）：后端拒绝时错误以 unhandled rejection 蒸发，
   列表数字不变、无任何提示（桌面 alpha pass 实锤）。修复：catch + error toast 透传后端原因。
   回归 = vitest clone-failure 用例。

### 开放缺陷（需产品决策，未在本次擅自改）

- ~~**#4 符号链接结构性缺口**~~ **已修复（2026-09-08 收口轮）**：
  - 调查（实测）：链接由版本安装的 npm/pnpm 布局产生（PHL 自身从不创建）；Windows 上是
    **Junction**、绝对路径、指向 `<root>/versions/<v>/node_modules/*`（真实实例 966/483 条）；
    Rust 对 junction 的 `is_symlink()` 三入口均为 true，检测无漏。
  - 修复：`instances/copy.rs` 引入 `LinkPolicy`（`Preserve` / `Rewrite { new_root, match_on }`），
    managed 判据 = 目标落在 `<root>/versions/` 内（copy 路径 canonicalize，迁移撤销读原始文本）；
    逃逸、跨实例、断链一律拒绝。clone / snapshot create / restore / relocation 四个调用点分别接
    Preserve·Rewrite。同盘迁移 rename 快速路径原样平移会留下悬空绝对链接——补
    `repoint_managed_links` 翻译 + 失败回滚（本轮 e2e 新发现的第二个真实缺陷，一并修复）。
  - 整合包 / Bundle：pack 是字节容器，链接一律不打包且现在**可见**（`TreeAdd.skipped_links`
    → 导出报告 `linksSkipped` → 导出完成 toast；CLI build 同样打印）。managed 依赖链接由目标机
    安装时重建，语义正确。
  - 回归：copy.rs ×6（managed 重建 + 断链消息 / 逃逸拒绝 / rewrite 翻译 / junction 树整体复制 /
    拒绝时零改写 / 原始文本匹配翻译悬空目标）、instances ×2（真实 junction 走
    run_clone + snapshot create + restore 全链；删除不穿透链接）、
    storage ×3（同盘迁移重写 / 逃逸链接拒绝零改写 / 撤销把链接译回源根）、pack-core ×1（不打包且报告）。
- **#5 名称长度不拦截（低，观察项）**：创建/克隆的实例名超 64 字符 UI 无提示（id slug 已安全截断，
  无路径风险）。行为安全，但「超长名称」的用户预期是可见拒绝还是静默截断，需要定义。

### 收口轮（同日第二阶段）：#4 落地 + 新发现

1. **#4 修复落地**：`LinkPolicy`（Preserve / Rewrite）接管 copy engine；
   clone/snapshot/restore 用 Preserve，relocation 用 Rewrite；`dir_size` 对链接计零字节
   （快照/克隆不再复制共享版本树，staging 校验两侧口径一致）。
2. **新缺陷（同轮修复）——同盘迁移 rename 绕过链接策略**：rename 原样平移绝对 junction，
   源根删除后全部悬空。新增 `repoint_managed_links`（迁移后遍历翻译，非 managed 链接
   拒绝并把 rename 回滚、journal 置回 Pending）；e2e ×2（重写成功 + 逃逸拒绝零残留）。
3. **删除爆炸半径已验证**：带 junction 的实例删除不会跟进共享 `versions/`
   （`deleting_an_instance_never_reaches_through_its_links`）——MSYS `rm -rf` 会跟进，
   人工清理测试目录必须用 `rd /s /q`。
4. **pack 链接可见性**：`TreeAdd.skipped_links` → 导出报告 `linksSkipped` → 导出完成
   info toast；CLI build 打印跳过清单。整合包从不携带链接（目标机安装时重建）。
5. **观察（P2，未改）**：`recreate_link` 每个 junction 起一次 `cmd /C mklink /J`，
   497 链接的快照/克隆/恢复耗时数十秒；批量或原生 API 可优化。
6. **验收发现的第三个真实缺陷（本轮修复）——repoint 不是原子的**：`repoint_managed_links`
   边遍历边改写，遇到逃逸链接返回 Err 时，**已改写的链接不回滚**，而 `storage.rs` 的失败回滚
   只把目录 rename 回去 → 源树恢复后链接指向 `to/versions`，而 `versions` 按 `DATA_DIRS` 顺序
   后搬、此刻不存在 → 悬空环境。修复：两阶段（先全量分类，全部通过再改）+ 阶段二自带还原；
   分类统一走 `LinkPolicy::Rewrite`（此前 repoint 另写了一份判据，与 `action` 的 canonical 语义
   不一致，正是 e2e 失败的原因）。回归：`refusing_a_link_leaves_every_earlier_link_untouched`
   （copy 层）+ `a_same_drive_move_refuses_escaping_links_and_leaves_the_source_whole` 加 managed 链接
   （迁移层，改前必失败）。
7. **验收发现的第四个缺陷（本轮修复）——撤销不翻译链接**：`undo_migration` 只把目录 rename 回源根，
   链接仍指向 `to/versions`（可能从未存在）。修复：撤销后按 `LinkMatch::Raw` 译回源根
   （原始文本匹配，因为 `to/versions` 不存在、canonicalize 无法分类）；失败则把目录放回目标根并报错。
   回归：`raw_matching_translates_a_link_whose_target_does_not_exist_yet`（copy 层）+
   `undo_translates_managed_links_back_to_the_source_root`（storage 层）。
8. **#4 的第二个链接来源（本轮修复）——pnpm 插件依赖链接**：实测 pnpm 9.15.9 / Windows，
   `plugins/install.rs` 的 `pnpm install --ignore-workspace --prod` 把 `node_modules/<dep>`
   建成指向 `node_modules/.pnpm/<dep>@<v>/node_modules/<dep>` 的 **Junction**，目标落在**实例树内**。
   原 `Preserve` 只接受 `<root>/versions/` 下的目标 → 装了带依赖插件的实例仍不可克隆/快照，
   报"指向受管版本之外位置的链接"。#4 此前只对 DSH 自身的版本链接成立，低一层没关。
   修复（`f7fef82`）：`Preserve { source_root, dest_root }`——共享版本链接照旧，树内链接随副本重建，
   调用方传**最终**目录而非 staging（staging 会被 rename 到位，指向 staging 的绝对链接落地即悬空）；
   `Rewrite` 扩大到"正在迁移的子树"，同盘 rename 用 `Raw` 匹配（子树旧路径已不存在，无法 canonicalize）
   并拒绝带 `..` 的原始目标。回归：copy ×2 新增（树内链接随副本 / Rewrite 翻译）、
   instances e2e 扩展（clone + snapshot create + restore 全链）、storage 15/15。

### 手测覆盖映射（§3/§7/§11/§13 压缩结果）

| 原手测项 | 自动覆盖（deterministic） | Desktop 覆盖 | 仍需人工 | 最近结果 |
|---|---|---|---|---|
| §3 创建实例 | Rust `instance_lifecycle_on_disk` 等 | 本轮 GUI-1 | 否 | PASS |
| §3 克隆（含隔离） | `clone_diverges_from_source_at_the_byte_level`、`clone_and_snapshot_survive_a_managed_node_modules_link` | 本轮 GUI-2（真实 497 junction） | 否 | PASS |
| §5 启动/停止/接管 | launch/registry 12+7 tests + webui 3 | 上一轮 §16 + 本轮崩溃链 | 冷启动安装包（§15 清单保留） | PASS |
| §6 Bundle | 8 项 roundtrip tests | — | 否 | PASS |
| §7 快照/回滚 | `snapshot_create_restore_roundtrip`、running 守卫、链接回环 | 本轮 GUI-2 create→rollback | 否 | PASS |
| §11 外部实例写保护 | `external_instances_survive_every_gated_write_attempt_untouched`（字节级） | 否（需真实外部 home 语义） | 接入向导体验项 | PASS（自动化） |
| §13 Pack 导出/安装/secret | pack-core 32 + desktop `export_scan_install_roundtrip…`（含整包扫描） | 上轮冒烟 + 链接可见性本轮新增 | 否 | PASS |
| §16 焦点/WebUI 窗口 | vitest 77 + webui tests | 上一轮 §16 全项 | 否 | PASS |
| §15 R1/R3/R7 冷启动/加载 | 无法自动化（真机安装包 + 真实插件加载） | 部分 | **是** | OPEN |

### 桌面验收结果（2026-09-08 收口轮，Computer Use · 全新隔离 root）

**8 PASS / 1 SKIP / 0 FAIL**（fresh report，`.scratch/alpha-20260908/fresh-report.json`）。
SKIP：pack 导出 GUI 未驱动（层一 e2e 已全链覆盖，本轮聚焦 #4）。旧报告的 3 fail 对应项
本轮全部转 PASS（崩溃留痕、克隆静默、符号链接阻断）。


7 pass / 5 skip / 3 fail（3 个 fail 全部对应上面已修复或已登记的缺陷）。报告存档
`.scratch/alpha-20260908/desktop-report.json`。要点：空名内联拒绝、创建→后台自动装版本→
校验→事务提交全链正常、停止时 WebUI 菜单项正确禁用、AX 元素路径可后台驱动 Tauri 窗口。
工具链注意：computer-use 对 React 受控输入框走 AX set_value 会假成功（fail-open receipt），
需 event 策略键入——记录于 report TOOLING，属验收工具问题非产品问题。

## 已知的刻意外

- Bundle 不携带插件**文件**（只带记录，重装走插件页）——设计如此
- 快照不含 workspace / logs——快照管环境复现，不管数据备份
- ~~PHL 重启后，之前拉起的 DSH 进程仍在运行但不受管理~~ → 已由持久进程登记解决：身份可确认的进程重启后自动接管，无法确认身份的报告为外部进程且不终止（§10）
- profile 非 `web` 的实例拒绝启动（`--port` 只在 `dsh web` 上存在），报错里给了修改指引
