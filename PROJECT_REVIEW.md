# 项目审查与重构记录

## 2026-09-08：Git 整理与当前健康复查

结论：实例、版本、Runtime、插件、Pack 与快照主体均已实现，现有检查通过；当前仍有进程接管与 WebUI 生命周期缺口，迁移的极端中断恢复也未完全闭环。应先补稳定性和安装包验收，再扩展功能。本次检查包含 Git 历史、现有未提交变更、自动化检查和关键失败路径静态复查；没有操作正在运行的用户实例，也没有把过去的桌面验收当成本轮验证。

### Git 整理

- 初始：`main` 领先 `origin/main` 69、落后 1；31 个已跟踪文件显示修改（其中一个仅工作区换行差异），另有 6 个未跟踪文件。
- `git fetch origin` 后确认远端 `4317475` 为仅代码发布记录。对比原 `7442240`，代码一致，远端省略了 15 个本地文档文件。
- `e5f38c8` 保存原有 36 个实际变更文件（WebUI、Alpha 验收、链接策略等），与本轮新增修复分开留痕；`7176ad7` 用保留本地树的合并衔接该发布记录。
- 删除已被 `main` 完整包含的本地分支 `feat/github-agent-install`、`fix/link-policy-and-ipc-contract`；原提交仍可从 `main` 历史访问。
- 保留原来禁用的 push URL；本轮未推送、未创建 PR/tag、未触发发布。领先提交数包含本地历史和文档，不能当作尚未发布的功能数量。

### 本轮修复

| 优先级 | 触发条件与影响 | 修复及验证 |
|---|---|---|
| P1 | 迁移已经 rename 到目标，但 journal 仍为 `moving` 时退出；恢复会先删除目标，随后发现源不存在，导致唯一副本丢失。跨盘删除源到一半时也可能用残缺源覆盖完整目标。 | `storage.rs` 在任何目录变动前检查歧义状态，保留双方和 journal 并报错；跨盘在删除源之前写入已验证的 `moved` 状态。回归模拟源不存在和源残缺两种窗口，检查数据与 journal 保留。 |
| P1 | 撤销遍历 `pending` 行，可能删除原本造成目标冲突的其他数据；源删除不完整、两侧目录都存在时也会误删完整目标。 | 跳过未执行行，对存在两份目录的已移动行拒绝自动撤销；两个文件级回归覆盖冲突数据和残缺源。 |
| P1 | `test:desktop` 退出时递归删除手动指定的 `PHL_ALPHA_ROOT`，即使目录原先就有数据。 | 只有本次 `mkdtemp` 分配的目录才自动清理；指定目录及中断目录保留。真实脚本集成测试用假 npm 代替桌面启动，验证 sentinel 文件保留。 |
| P2 | WebUI 用 `127.` 前缀及长度判断本机地址，`http://127.example.com/` 也能通过。 | 使用解析后的 `url::Host` 和 IP `is_loopback()`；拒绝伪装域名的回归加入现有 URL 测试。 |
| P2 | 验收报告存放在自动删除的目录内，正常退出会一起丢失。 | 报告改放在 root 旁，以运行时间区分；集成测试确认自动清理后报告保留。Windows 中断改为终止子进程树，异常退出保留数据目录。 |

新增 runner 测试同时接入 Alpha gate 与 CI 前端 job。

### 仍需修复或验收

| 优先级 | 问题及证据 | 下一步 |
|---|---|---|
| P1 | `launch/mod.rs:265` 的 `adopt_processes` 只登记存活进程，没有建立后续退出监听；唯一的 `child.wait()` watcher 在本次启动路径。重启后接管的进程随后退出，前端可能仍显示运行中，WebUI 也不会自动关闭。静态确认，未在用户进程上做故障注入。 | 为接管进程建立身份校验与存活监听，覆盖退出、停止、PID 复用和二次接管。 |
| P2 | `launch/mod.rs:629` 的旧 watcher 虽按 PID 清理进程表，但 `close_for_instance` 只按 instance id 关闭窗口；`webui.rs` 的窗口标签也不包含进程代次。快速重启时延迟到达的旧退出可能关闭新窗口，重新聚焦也不会校正旧 URL/token。静态确认，未做真实 GUI 竞态复现。 | 将窗口所有权绑定当前进程或启动代次，补旧退出与重新启动交错测试。 |
| P1 | 迁移歧义状态当前以保留数据并报错收口，需要人工核对，尚不是完整自动恢复。跨盘撤销仍使用 rename，不能保证跨盘成功；链接改写中断也需要更细的日志。 | 增加可区分的落位/链接改写/源清理阶段与恢复方案，并补真实跨盘、进程中断验收。 |
| P2 | 主 JS 506.05 kB（gzip 159.41 kB），构建仍提示超过 500 kB。 | 按首屏性能测量决定后续分包，不以增大警告阈值代替优化。 |
| 发布验收 | 本地新增内容尚未经过远端 CI 与干净 Windows 安装包冷启动；6 个 Rust 忽略测试未启用。 | 按手测清单执行安装包、真实插件加载和外部接入验收。 |

### 本轮验证

- `npm run typecheck`、`npm test`：通过，13 个文件 / 78 项前端测试。
- `npm run bridge:check`：17 个 bridge，72 个调用 / 78 个注册，无缺失或重复。
- `npm run build`：通过，保留上述分包提示。
- `cargo test --manifest-path src-tauri/Cargo.toml --workspace --offline --quiet`：306 项通过（桌面库 270、CLI 3、Pack Core 33），6 项忽略。
- `cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --offline --all-targets --all-features -- -D warnings`：通过。
- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`、`git diff --check`：通过。
- `node --test scripts/test-alpha-desktop.mjs`：1 项脚本集成测试通过；未启动真实 Tauri。脚本使用 Node test runner，命名与 Vitest 的自动发现规则分离。
- `npm audit --omit=dev --audit-level=high`：生产依赖 0 项已知漏洞；没有审计开发依赖或 Rust 依赖漏洞。
- [远端最近 CI](https://github.com/A7m0spHere/dsh-phl/actions/runs/34176772394) 成功，对应 `4317475`；不包含本轮本地变更。前一条 `7442240` 的 CI 也已成功，较早的 Windows 路径失败不能再视为当前阻塞。

---

## 2026-09-08：GitHub 更新前审核

范围：本地 `663cbe0` 与远端 `f17030e` 的树差异（89 个文件），重点检查插件写入、Pack 安装/导出、取消与异步状态，以及 bridge/store 拆分。两条历史没有共同祖先；发布时保留双方历史，以本次审查修复后的代码树整合，不强制覆盖远端引用。

本次确认并修复：

| 优先级 | 触发与影响 | 修复与验证 |
|---|---|---|
| P1 | Pack 内置插件把 Cordis 注册写到实例根目录，DSH 读取 profile 时找不到挂载 | `register_embedded_plugins` 只接收 profile；回归直接读取 `dsh-home/profiles/web/cordis.patch.yml`，并确认根目录没有误写 |
| P1 | 插件启停/卸载使用普通 `profile_dir`，绕过 external 实例写入限制 | 恢复 `writable_profile_dir`，与插件安装及外部实例只读契约一致 |
| P1 | profile 级 `pnpm add` 改写共享依赖，插件失败回滚无法恢复这些文件；超时后子进程还可能继续写盘 | 在当前插件目录安装，忽略上层 workspace、禁用 lifecycle scripts；超时终止并等待子进程；本地依赖集成测试验证 profile 不变、依赖可由 Node 加载、脚本未运行 |
| P1 | scoped 包名的未引用 `@` 产生无效 YAML；共享 insert 中扫描、停用、卸载可能混淆兄弟插件或嵌套 config | 引用 scoped id；统一按 mount row 读取和编辑；保留其他插件；支持 name 开头的行、注释和 scoped 包扫描；以 YAML 解析与真实文件操作验证 |
| P2 | Pack reset 后旧请求的进度、失败或 finally 回写新流程，第二次安装丢失取消句柄；返回后旧 preview 重新填充 | 请求代次与独立 AbortController 身份检查；阻止重复提交；异步交错回归覆盖旧失败、旧进度、新取消与返回 |

验证：

- `npm run typecheck`、`npm test`：75 项通过。
- `npm run build`：通过；主 JS 504.01 kB，gzip 158.87 kB；仍有已有的 500 kB 分包提示。
- `npm run bridge:check`：17 个 bridge、71 个调用、75 个注册，无缺失或重复；已加入 CI。
- `cargo test --manifest-path src-tauri/Cargo.toml --workspace --offline`：277 项通过，6 项默认忽略（包含新增 pnpm 环境依赖测试）。
- 显式执行 `dependencies_stay_inside_the_plugin_and_scripts_do_not_run -- --ignored`：1 项通过，使用本机 pnpm/Node 和本地测试包。
- `cargo clippy --manifest-path src-tauri/Cargo.toml --offline --workspace --all-targets --all-features -- -D warnings`：通过。
- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`、`git diff --check`：通过。

远端前一轮 CI `34035139199` 的失败原因是 Windows 8.3 短路径与长路径直接比较。本地待发布更新已改为 canonicalize 后比较目录身份，本轮回归通过。未执行真实用户实例启停、NSIS 安装或跨机器 Pack 搬迁；上述自动化结果不等同于这些真机验收。

---

日期：2026-09-05。工作分支：`codex/project-review-refactor`，基于 `feat/api-config-ui-polish`。

本轮深入检查了实例状态、配置持久化、进程生命周期和目录迁移，并对版本、插件、运行时模块进行了静态检查及既有测试回归。总体的 React → service → Rust 分层可以继续使用，主要问题集中在异步操作交错、失败处理，以及多个复制流程的重复实现。

## 已修复

| 优先级 | 问题与触发条件 | 修改 |
|---|---|---|
| P1 | 迁移数据目录时只搬实例、版本、Runtime、缓存，漏掉 API 配置库 | 将 `config/` 纳入目录检测和迁移，切换根目录后重新加载 API 配置并清空旧快照 |
| P1 | 后面的目标目录存在冲突时，前面的目录已经搬走；带 `..` 的路径可能绕过父子目录检查 | 操作前检查所有目标目录，并按规范化后的真实路径验证目录关系 |
| P1 | 复制流程忽略目录枚举、文件属性或线程错误；克隆取消后清理可能与复制线程竞争 | 克隆、快照和迁移统一使用 `copy_tree_with_progress`，等待 worker 的明确结果后再清理；遇到不可读条目和符号链接明确失败 |
| P1 | `taskkill` 返回非零仍被视作成功，前端又吞掉停止错误 | 检查命令退出码，停止失败时保留 PID、URL、开始时间和端口占用；退出应用时遇到停止失败保留窗口 |
| P1 | 旧进程退出事件误清新进程状态，或退出事件早于启动结果导致“已死但显示运行中” | 事件增加 PID；匹配进程身份；缓存启动阶段的退出事件；停止阶段统一结算运行时长 |
| P1 | 连续修改实例会并发写同一清单；旧请求失败可能回滚新编辑，连续失败又可能回滚到未保存的值 | 提取可测试的乐观更新队列，按实例串行持久化，并基于最后一次成功保存的值重放剩余编辑 |
| P2 | 插件、快照和磁盘占用这些派生数据也触发清单写入 | 派生字段仅更新内存；删除快照完成后读取当前列表，避免覆盖同时创建的快照 |
| P2 | API 配置读取失败回退到浏览器数据；导入保存失败仍返回成功 | 桌面读取错误向上传递；导入失败返回 null；旧根目录的加载结果不能覆盖新根；拒绝同时启动第二次 API 保存 |
| P2 | 启动失败后的“重试”没有复用异常处理 | 重试完整初始化流程，持续保留错误提示与再次重试入口 |
| P2 | 更改数据目录只拦截 running，漏掉 starting、stopping、快照和保存中状态 | 扩展切换前的忙碌检查；退出前等待实例配置写入完成 |
| P2 | `C:\` 被去掉分隔符后成为相对路径 `C:`；“1 小时前”错误显示为“1 分钟前” | 提取路径归一化函数并保留根路径；修正相对时间的单位和除数 |
| P2 | 所有页面一次性进入主脚本，首屏加载不需要的页面代码 | 首页保留直接加载，其他页面及其面板通过 React.lazy 按需加载，并提供加载状态 |
| P2 | 开发工具链依赖审计有已知漏洞 | Vite 升至 6.4.3，加入 Vitest 3.2.7，更新锁文件；当前 npm audit 为 0 项漏洞 |

## 验证

- `npm test`：25 项前端测试通过，覆盖保存顺序、失败回滚、派生数据、进程事件顺序、API 错误处理、时间单位和根路径。
- `npm run typecheck` 和 `npm run build`：通过。
- 主脚本从本轮按需加载优化前的 637.36 KB 降至 474.88 KB，约减少 25.5%；gzip 从 193.72 KB 降至 149.37 KB。页面分包另外按需加载，这不是总产物体积减少 25.5%。
- `cargo test --manifest-path src-tauri/Cargo.toml`：76 项通过，4 项按原设置忽略；新增迁移配置库、路径别名、目标冲突、取消、复制失败及停止命令失败的测试。
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`：通过。
- `npm audit --audit-level=low`：0 项漏洞。
- Playwright 浏览器检查：6 个主导航、创建页面、实例不存在时的详情页均可加载，无页面异常或控制台错误。脚本为 `scripts/browser-smoke.py`，需本机安装 Python Playwright 并启动 `npm run preview -- --host 127.0.0.1 --port 5189`。
- `git diff --check`：通过。

## 范围与后续注意事项

本轮没有对真实用户实例执行删除、迁移或启停。浏览器检查使用 Mock Repository；真实跨盘迁移、大型 node_modules、权限不足时的进程终止、外部发布源下载，以及 macOS/Linux 进程行为仍需在对应环境手动验证。4 项原有忽略测试涉及网络或本机外部配置，没有强制启用。

迁移仍沿用项目原有的“按目录完成、取消后可续传”语义，并非整个数据根目录的事务；中途取消会使已完成目录留在新位置。复制路径遇到符号链接会报错并保留源数据，需要处理链接后重试；同盘直接重命名不需要复制文件。

API、插件和创建向导页面仍然较大。下一轮可按供应商编辑、模型列表、插件发现、向导步骤拆分组件，但应结合交互测试逐步做，不能仅凭文件行数拆分。本轮优先改动已经确认存在的故障路径。

用户原有的 `src/data/vendorPresets.ts`、`src/pages/ApiConfigPage.tsx` 和 `src/types/apiConfig.ts` 三个未提交改动完整保留，不纳入本轮提交。没有合并原分支，也没有推送远端。
