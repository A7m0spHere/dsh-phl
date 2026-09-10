# dsh-phl 代码审查报告（Agent Team · 2026-09-10）

> 方式：Lead + 4 名只读审计员分四个互不重叠的范围并行审查，Lead 对高危结论逐条独立复核/复现。
> 全过程未修改任何产品源码。明细报告：`.scratch/review/{rust-core,rust-runtime,pack-core,frontend}.md`。

## 0. 基线（本工作树实测，全部为绿）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 类型检查 | `npm run typecheck` | 通过 |
| 前端单测 | `npx vitest run` | 20 文件 / 121 项全通过（12s） |
| Rust 单测 | `cargo test --workspace` | 全通过（含 phl-pack-core 33 项、CLI 3 项） |
| Clippy | `cargo clippy --all-targets -- -D warnings` | 通过 |
| IPC 桥 | `npm run bridge:check` | 73 invoke / 76 command，无缺失、无重复注册 |
| 版本一致性 | `node scripts/check-versions.mjs` | 通过 |
| TODO/FIXME | `git grep` | 0 处；`as any` 0 处 |

结论：**下列问题全部是现有测试与 CI 抓不到的潜在缺陷**，不是"构建坏了"。工作树另有 7 个未提交文件（属正常开发状态，已纳入审查）。

---

## 1. 严重问题（P0/P1）

### 1.1 [P0 · 已由 Lead 复现] `sanitize_version` 允许 `"..."`：Windows 上会把整个 `versions/`（或 `runtimes/`）目录删掉
- **位置**：`src-tauri/src/versions/install.rs:84-100`（白名单）、`versions/install.rs:386-451`（删除）、`src-tauri/src/runtimes/mod.rs:242`（同一函数用于 runtime 删除）、`src-tauri/src/paths.rs:81-107`（`ensure_under_root` 拦不住）
- **现象**：白名单允许 `.`、`-`、`_`、`+` 与字母数字，只显式拒绝 `"."` 和 `".."`（第 90-94 行的注释说明作者已意识到 `versions/..` 会解析到数据根）。但 Windows 的 Win32 路径归一化会**剥掉每个路径分量的尾部点号**：`<root>/versions/...` 解析结果就是 `<root>/versions` 本身。
- **Lead 复现**（`rustc` 单文件探针，只在 %TEMP% 的临时树上执行，见 `.scratch/dotdotdot-probe.rs`）：
  ```
  lexical components of '<versions>/...': [... Normal("versions"), Normal("...")]   // 不是 ParentDir
  exists = true     is_dir = true
  remove_dir_all('<versions>/...') => Ok
  after: '<versions>' still exists = false        // 整个 versions 目录（含 dsh-1.0.0/）被删除
  ```
- **为什么 `ensure_under_root` 无效**：`Path` 的分量是 `Normal("...")`，不含 `ParentDir`，词法检查通过；`canonicalize` 把路径折叠成父目录后仍然 `starts_with(root)`，词法+真实路径两层检查都放行。
- **影响**：`remove_version_dir` / `remove_runtime_dir` 拿到 `"..."`（或 `"...."`）即删除 **`<root>/versions` / `<root>/runtimes` 整棵树**——所有已安装 DSH 版本 / Runtime 一次性丢失，且是 `remove_dir_all`，无法恢复。
- **可达性（如实说明）**：正常 UI 只对"已安装"行显示删除，而 `"..."` 目录会被 `is_hidden_tree_name`（以 `.` 开头）过滤，列表里根本看不到；因此这不是"点一下按钮就删库"，而是 **IPC 边界不设防 + 灾难性爆炸半径**：任何能构造一次 `invoke('remove_version_dir', { versionName: '...' })` 的路径（前端缺陷、被注入的 WebView、日后新增的调用方）都会造成不可逆损失。这与 `paths.rs` 模块文档的承诺（"destructive commands … confine every one of them under the root"）直接冲突。
- **建议**：白名单加"名称不能只由点号组成"（如 `!name.trim_matches('.').is_empty()` 或要求首字符为字母/数字），并顺带拒绝 Windows 保留名与尾部 `.`/` `；删除路径在 `join` 之后再加一次真实路径复核。runtime 删除同时补齐下面 1.4 的守卫。

### 1.2 [P1 · 已由 Lead 复现] `.phlpack` 可以写入 NTFS 备用数据流（ADS）：完整性校验全绿，Explorer 里看不见
- **位置**：`src-tauri/crates/phl-pack-core/src/lib.rs:147-172`（`normalize_entry`）、`unpack.rs:124`（`File::create`）
- **现象**：`normalize_entry` 只拒绝 `ParentDir`/`RootDir`/`Prefix`。条目名 `payload/bar:baz` 的分量是 `Normal("bar:baz")`，校验通过；落地时 `File::create(<dir>/bar:baz)` 被 Win32 解释成"文件 `bar` 的隐藏流 `:baz`"。
- **Lead 复现**（`.scratch/ads-probe.rs`）：
  ```
  visible directory entries: ["bar"]                 // 目录里只有一个可见文件
  read back 'bar:baz' = "SMUGGLED PAYLOAD"
  Get-Item -Stream *  ->  :$DATA 15   baz 16         // baz 是看不见的流
  ```
  pack-core 另用 .NET `ZipArchive` 构造同名 entry 跑真实 `phl-pack validate|unpack`：`validate exit=0`，解包后 ADS 出现——即**被校验通过、被 integrity 覆盖的包，仍能在磁盘上留下校验面之外的内容**。
- **建议**：`normalize_entry` 对每个 `Normal` 分量额外拒绝含 `:` 的名字；同一处补 Windows 保留设备名（`NUL/CON/AUX/COM1…`）与尾部 `.`/` `（后者见 3.6，会让"已解包 3 个文件"与磁盘实际不符）。

### 1.3 [P1 · 已复核代码] `save_instance` 不校验 `profile`：IPC 可让后续写操作落到数据根之外
- **位置**：`src-tauri/src/instances/mod.rs:289-299`（写路径）、`instances/manifest.rs:266-309`（`write_manifest` 只强制 `schema_version`）、`instances/manifest.rs:144-148`（`profile_root_of` 裸拼）、`repair.rs:292-299`、`plugins/install.rs`（经 `writable_profile_dir`）
- **现象**：`create_instance`(mod.rs:658)、`adopt_inner`(adoption.rs:385)、`pack::install`(pack/install.rs:357) 都调了 `sanitize_segment(profile)`，**唯独 `save_instance` 漏了**：它只做 `load_manifest` + `canonicalize_version_binding` 就把前端传来的整个 manifest 落盘。之后 `profile_root_of` 直接把 `manifest.profile` 拼进 `<home>/profiles/<profile>`，`repair_instance([recreate-skeleton])` 会据此 `create_dir_all`，插件装卸会在该目录下写 `node_modules`/`package.json`/`cordis.patch.yml`。
- **影响**：`profile = "..\\..\\..\\..\\Users\\me\\.dsh\\profiles\\web"` 这类值可在数据根之外建立**写文件原语**（Windows 保留名同样可致后续操作失败）。同一写路径上 `runtime_id` 也从未白名单，而 `verify.rs:234-276` 会据此拼路径并执行该目录下的 `node.exe --version`（纵深防御缺口）。
- **建议**：`write_manifest` 落盘前统一 `sanitize_segment(&profile)` 与 runtime id 白名单；`profile_root_of` 改为返回 `Result`，让所有读取方天然拿到已校验值。

### 1.4 [P1 · 已复核代码] 删除 Runtime 既无"实例在用"守卫，也不先改名再删
- **位置**：`src-tauri/src/runtimes/mod.rs:237-251`，对照 `versions/install.rs:398-451`
- **现象**：版本删除有两道保护——遍历 `Processes` 找正在用该版本的实例并拒绝（`versions/install.rs:402-432`）、先把目录 rename 成隐藏的 `.phl-REMOVE-*` 再删；runtime 删除**两道都没有**，直接 `remove_tree_progress` 原地递归删。
- **影响**：删除正在被实例使用的 `node-22` 时，Windows 上 `node.exe` 被占用 → 递归删到一半失败 → `phl-runtime.json` 已删、列表显示"未安装"、磁盘留半截目录；所有绑该 runtime 的实例下次启动报"Runtime 未安装"，只能重装。前端只做软确认（`RuntimesPage.tsx`），失败后把行还原成"已安装"，与磁盘真实状态不符。
- **建议**：照抄版本删除的守卫并统一"先改隐藏名再删"。

---

## 2. 确定的功能性缺陷（P2）

| # | 位置 | 问题 | 建议 |
| --- | --- | --- | --- |
| 2.1 | `launch/process.rs:207-251` + `launch/mod.rs:787/858/872` | 端口分配是 check-then-use：`allocate_port` 只读 `Processes` 不写占位，登记发生在 spawn 之后；两个实例并发启动可拿到同一端口，就绪探测 `TcpStream::connect` 连到**对方的** DSH 并判 ready | 分配时写 `pid:0` 保留位、spawn 后覆盖；ready 判定要求拿到本 boot 的 token 行 |
| 2.2 | `launch/registry.rs:144-270` + `lib.rs:313` | 登记表是"进程内 Mutex + 整表覆盖写 `processes.json`"，而应用**没有单实例保护**（无 `tauri-plugin-single-instance`）；双开 PHL 时后写者用自己 boot 的快照覆盖，丢掉另一进程刚登记的运行实例行 | 引入 single-instance 插件；或落盘前在文件锁内 read-modify-write 合并 |
| 2.3 | `instances/mod.rs:330-355` | `delete_instance_inner` 完全不读 manifest：schema 过新的实例在 list/orphan/save 三处都被保护，唯独这里 `remove_dir_all` 全树删除 | 删除前 `load_manifest(&dir, id).await?` |
| 2.4 | `instances/manifest.rs:180-183` | 读取 manifest 的**任何** IO 错误都被折叠成 `Missing`（权限拒绝、共享冲突、非 UTF-8）→ 有效实例从列表消失并出现在"可回收孤儿"里，可被整树删除 | 按 `ErrorKind` 分流，新增 `Unreadable` 且禁止其进入回收路径 |
| 2.5 | `diagnostics.rs:142-156` | `clear_download_cache` 以 `Vec::new()` 注册资源，等于不持锁（含不与 `DataRoot` 冲突）→ 可在下载中途 `remove_dir_all(<root>/cache)`，让进行中的版本安装以 NotFound 失败 | 新增 `Resource::Cache`，下载与清缓存共同持有 |
| 2.6 | `storage.rs:463-518` | `move_root_data` 完全信任 IPC 传来的 `from`，从不与 `phl.root()` 比对；传错的 `from` 会把无关目录当数据搬走并把根指针切到 `to`，journal 随即被删、撤销入口失效 | 入口校验 `same_location(from, phl.root())`，或让前端不再传 `from` |
| 2.7 | `versions/install.rs:150-162` | `promote_staged` 回滚时 `remove_dir_all(dest)` 与 `rename(backup,dest)` 的结果都被 `let _` 丢弃，却无条件返回"最终校验失败，已恢复原版本"→ 坏树留在原地、好树卡在隐藏的 `.phl-txn/backup-*`，用户被文案误导 | 回滚改为返回 `Result`，失败时报出 backup 路径；删 dest 改为先 rename 到 reject 名 |
| 2.8 | `api_config/library.rs:125-133`、`api_config/sync.rs:183-198` | `save_api_config`（写 `<root>/config/api.json`）与 `sync_instance_api`（写实例 `settings.yaml`）都不取资源锁 → 与"数据根迁移"（独占 `DataRoot`）、"快照恢复"（持 `Instance`）互不阻塞，迁移期间保存的配置会写进旧根而看似丢失 | 分别取 `DataRoot` / `Instance` |
| 2.9 | `launch/mod.rs:674-704` | 启动自愈直接 `npm install --prefix versions/<name>`，只持 `Instance` 锁，而该目录写入在别处都受 `Resource::Version` 保护；且自愈发生在 spawn 登记之前，连删除时的"实例在用"守卫都绕过 | 自愈前临时取 `Resource::Version(name)` |
| 2.10 | `instances/copy.rs:333-353` | `cmd /C mklink /J` 拼未加引号的路径：数据根含 `&`/`^`/`(`/`|` 时 cmd 重新解析参数 → 克隆/快照/迁移必然失败，且构成命令注入面 | 不经过 cmd.exe（`symlink_dir` 或 junction crate） |
| 2.11 | `plugins/security.rs:15-23` + `plugins/resolve.rs:88-100` | `verified` 信任级自证：下载 URL（`dist.tarball`，无 host/scheme 约束）与期望摘要出自同一份 packument，任意被配置的 npm 镜像都能让攻击者代码以"verified"装上并被 DSH 加载 | `dist.tarball` 必须 https 且 host 等于注册源，否则降级 unverified；文案改为"与注册源摘要一致" |
| 2.12 | `api_config/models.rs:107-166` | `fetch_provider_models` 是混淆代理：调用方传 `base_url`+`provider_id`，PHL 去系统凭据库取 key 发往任意 host（`models_url` 不限 host） | 用库内该 provider 的 `baseURL` 校验目的 host，或移除 `provider_id` 参数 |
| 2.13 | `sessions/mod.rs:117`、`sessions/copy.rs:122` | 为读一个 header 就把整份 `session.jsonl` 读入并**全帧解压**（列会对每个会话都这么做），复制路径峰值约 3× 日志体积 | header 只读首帧/首行；复制加体积上限或流式 |
| 2.14 | `launch/mod.rs:809-814` + `process.rs:45-60` | 启动日志名只有秒级精度且 `.append`：同秒重启时文件头仍是上一次 boot 的 token，`web_url`/重启接管取到失效 token | 打开时 `truncate(true)`，或文件名加毫秒/序号 |
| 2.15 | `launch/mod.rs:450-466` | `stop_instance` 不取资源锁，在 spawn(858)→登记(872) 窗口内静默 no-op 返回成功，实例随后照常启动 | 取 `Instance` 锁，拿不到即返回 `[busy]`；启动中统一走 `cancel_launch` |

### 2.x `.phlpack` / CLI（pack-core，均为实测）

| # | 位置 | 问题 |
| --- | --- | --- |
| 2.16 | `phl-pack-core/src/lib.rs:207` | 去重键是字节级 `BTreeSet<PathBuf>`，`a/README.md` 与 `a/readme.md` 双双通过校验，Windows/macOS 解包时后者静默覆盖前者，而校验与 integrity 全绿 |
| 2.17 | `phl-pack-core/src/lib.rs:261`、`unpack.rs:519` | `sessionsIncluded:false` 但包里实际带 `sessions/` 时无人拦截（core 算了 `has_sessions` 却不判断），安装会写入外部对话记录 → 隐私同意被绕过；CLI 反而正确（`main.rs:127`），只差 core 一条一致性规则 |
| 2.18 | `phl-pack-core/src/write.rs:409-419` | `build` 失败后残留非法产物且挡住重试（GUI 路径 `export.rs:688` 有清理，core 没有）；实测失败后留下 384B 的 `.phlpack`，重跑被"输出文件已存在"拒绝 |
| 2.19 | `phl-pack-cli/src/main.rs:176-181` | CLI `unpack` 不检查目标目录、无 `--force`、无回滚；实测已存在的 `payload/real.txt` 被静默截断，而同一工具的 `build` 明确拒绝覆盖（行为不对称） |
| 2.20 | `pack-core` lib.rs:430-440 | scoped 插件计数少报（只取第三段，`@scope/name` 折叠成一个），安装页数字比实际少 |

---

## 3. 前端（P2/P3，`src/**`）

| # | 位置 | 问题 | 建议 |
| --- | --- | --- | --- |
| 3.1 | `App.tsx:238-248` + `Menu.tsx:136` / `Dropdown.tsx:54` / `plugins/discovery.tsx:49` | 弹出层打开时按 Esc **同时**关弹层并返回上一页（全局 `go-back` 只屏蔽了 palette/dialog）——从列表进详情页后点"⋯"再按 Esc 会退回列表 | 给弹层加共享 `popupOpenCount` 并纳入快捷键 `enabled` |
| 3.2 | `components/ui/Card.tsx:87-96`（Extra 容器 `:110` 只 `stopPropagation` 了 click） | 键盘用户在可折叠卡片 Extra 里的按钮上按 Enter/Space，默认激活被 header 的 `preventDefault` 掐掉且卡片被折叠（`InstanceDetailPage` 打开目录/管理插件/创建快照、`ApiBindingCard` 编辑、`EnvironmentHealthCard` 修复都受影响） | header keydown 加 `if (e.target !== e.currentTarget) return` |
| 3.3 | `stores/catalogRuntimeActions.ts:54-60` + `pages/RuntimesPage.tsx:92-163` | Runtime 安装失败把原因硬编码成"下载失败"（丢弃 `parseThrownError`），toast 无 message/无重试，且页面**从不渲染** `state.reason` → 行回到普通"未安装"外观 | 对齐版本路径（reason+hint+重试），行内渲染 reason |
| 3.4 | `stores/catalogVersionActions.ts:59-64`、`catalogRuntimeActions.ts:15-20` | `installVersion`/`installRuntime` 无重入保护，`controllers.set` 无条件覆盖：点"取消"可能取消错对象，同一版本被串行下载两遍（toast 的 action 按钮在 `mode="popLayout"` 退出动画期间仍可点） | 加 `controllers.has(key)` 互斥 + `finally` 按身份 delete |
| 3.5 | `stores/apiConfigStore.ts:342-363` | `refreshSnapshots` 无请求代次校验，过期响应覆盖刚同步成功的状态，把实例重新标成"已改/已删/本地新增" | 加 per-instance generation（同仓 `catalogPluginActions`/`packStore` 已有范式） |
| 3.6 | `components/instance/useInstanceActions.tsx:37-38` | 裸订阅 `useInstanceStore()`/`useUIStore()`，把 `InstanceCard` 的细粒度 selector 与 `memo` 全部作废：进度每次 patch（约 120ms）或任意 toast 都重渲染全部卡片，并重建 `menuItems` 触发 `Menu` 重定位 | 逐 action 订阅 + `getState()`；`ui` 拆 5 个稳定引用 |
| 3.7 | `stores/adoptionStore.ts:304/326/368` | 预览与提交各生成一次实例 id（带随机后缀），"预览"描述的对象永不等于落盘对象 | configure 阶段生成一次 `draftId`，预览与提交共用 |
| 3.8 | `lib/errorCodes.ts:32,50` | `ParsedError.retryable` 解析出来却**零消费**（仅测试引用），导致重试入口时有时无（版本有、Runtime 没有） | 收一个 `toastError(err, {retry})` 封装按 `retryable` 决定，或删掉该字段 |

---

## 4. 值得优化的地方（非 bug）

1. **大页面拆分未完成且出现回退**：`SettingsPage.tsx` 1050 行、`InstanceDetailPage.tsx` 937 行、`features/create/sections.tsx` 871 行。路线图 O-13 记录的是"SettingsPage 约 931 / InstanceDetailPage 约 878 行"，两个文件都比记录时更大了——按仓库自己的验收口径（"每次只迁移一个完整交互，不以行数当指标"）应继续推进。
2. **列表性能**：`InstancesPage` 无虚拟化且每个卡片都被整 store 订阅拖着重渲染（见 3.6）；`QuickSwitcher` 常驻挂载却整数组订阅 `instances`/`states`，每次进度变化重建 2N 条命令对象。
3. **死代码/死契约**：`packStore.ts:148`、`adoptionStore.ts:331` 调 `instanceStore.load()`，而后者在 `loaded || loadStarted` 时立即返回——注释承诺的"重新读取实例列表"不会发生（应改 `reload()` 或删除）；`errors.rs` 的 `retryable()` 被标 `#[allow(dead_code)]`，前端同名字段也无人消费。
4. **可访问性**：`Card` 的 header 用 `role="button"` 包住真实按钮（3.2 的根因），建议把折叠交互收敛到标题/箭头本身的 `<button>`。
5. **路线图仍开放的三项**（O-01 干净检出 CI/tag 演练、O-13 页面拆分、O-14 真机性能基线）在本机无法闭环，需要一次干净检出与真机采集。
6. **真机验证缺口**：`dsh-phl-manual-test-checklist.md` 覆盖的 125%/150% 缩放、50/200 实例、500 插件、长时运行内存等项目仍缺数据；本报告的并发类结论（端口竞态、双开丢登记行）也建议按"并发启动两个实例""双开 PHL"两个场景做一次故障注入验收。

---

## 5. 建议修复顺序

1. **立刻**：1.1（`sanitize_version` 点号名）、1.2（ADS）、1.3（profile/runtime_id 白名单）——都是"一处收窄输入"就能关掉的不可逆风险。
2. **本迭代**：1.4、2.1（端口保留位）、2.5（cache 锁）、2.7（回滚文案）、2.3（删除前读 manifest）——影响用户可见的数据安全与状态可信。
3. **发布前**：2.2（单实例）、2.6（from 校验）、2.4、2.8、2.10、2.11、2.16-2.19（pack 一致性/CLI 覆盖保护）。
4. **随后**：第 3 节前端项与第 4 节优化项，配合 O-13/O-14 一起排期。

---

## 6. 已核对但判定"不是 bug"（避免重复排查）

- 子进程 stdout/stderr 接的是**文件句柄**不是管道，不存在缓冲区写满死锁。
- `DSH_HOME` 无法被实例 env 绕过（先按大小写不敏感过滤，再 `set_isolated_home` 覆写，有单测）。
- 版本/Runtime 下载解压有 `safe_join`（逐组件拒绝 `ParentDir/RootDir/Prefix`），tar 的 link 条目不会写出链接。
- 下载取消/断点续传绑定 URL+ETag/Last-Modified、只在 206 追加、收尾重算整文件摘要，均有本地模拟服务器单测。
- 插件装卸有 journal+backup+RollingBack 事务恢复；外部命令带 `--ignore-scripts` 且无 shell 拼接。
- 凭据只落系统凭据库，`api.json` 为 tmp+rename 且失败回滚；`settings.yaml` 只写 `apiKeyEnv`。
- `capabilities/default.json` 未过度授权：`dsh-web-*` 虽在同一 capability 的 `windows` 里，但该条**无 `remote` 授权**，远程页面拿不到 IPC；WebUI 地址门禁只放行 loopback 且拒绝 userinfo。
- `bridge:check` 只校验命令名不校验参数形状（`ipc_contract.rs` 已声明该边界）；抽查三个代表性命令的 JS 参数与 Rust 签名一致。
- 会话列表刻意"count, never parse"仅适用于计数路径；列表路径的问题记为 2.13 而非重复项。
