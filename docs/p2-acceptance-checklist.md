# 验收清单 — 本机 DSH 接入 / Session / Pack（收尾轮 + Review）

> 日期：2026-09-07 · 分支：main（未提交工作树）· 对应《PHL 下一阶段开发规格》§28–§30、§32–§33
> 本清单分「✅ 已完成（附验收方法）」与「⏳ 未完成（需真机/决策）」。
> 实现细节见 `docs/p2-pack-core-skill.md`；进度状态入口由维护者在本地工作区维护（不随本仓库发布）。

## ✅ 已完成 — 可验收

| # | 任务 | 验收方法 |
|---|------|----------|
| 1 | **接入向导「按对话选择迁移」闭环**（P0 遗留） | 后端 `src-tauri/src/instances/adoption.rs` 的 `Selected` 策略不再拒绝；`cd src-tauri && cargo test --lib adoption`（含 3 个新用例：仅选中的迁入 / 空选拒 / 缺失目录拒+回滚）。前端：实例页 → 接入本机 DSH → 选「复制到 PHL」→ 勾「按对话选择迁移」出现可勾选对话列表。 |
| 2 | **复制会话后计数即时刷新** | `SessionCopyPanel` 成功后经 `onCopied` 触发实例详情重测（`src/pages/InstanceDetailPage.tsx`）。`npm test`。真机：复制对话后「历史对话 N 条」当场变，无需重开页面。 |
| 3 | **导入 Bundle 移入「更多导入方式」** | 实例页头部只剩三主操作（新建/接入/安装整合包），Bundle 收进 `Menu`。看 `src/pages/InstancesPage.tsx`。 |
| 4 | **Discovery macOS/Linux 平台路径** | 新增 `discovery/{windows,macos,linux}.rs`；`inspect::executable_roots()` 补 GUI 启动看不到的 Homebrew/npm-global/nvm/Volta/Bun 根。`cd src-tauri && cargo test --lib discovery`（非宿主平台文件经 `#[cfg(test)] #[path]` 也在本机编译+测）。⚠️ 仅代码+单测，真机验证未完成（见下）。 |
| 5 | **P2-1：抽 `phl-pack-core` crate + `phl-pack` CLI** | 格式层在 `src-tauri/crates/phl-pack-core/`；CLI `src-tauri/crates/phl-pack-cli/`（bin `phl-pack`，命令 inspect/validate/unpack/build）。验收：`cd src-tauri && cargo test --workspace` + `cargo build -p phl-pack-cli` 后 `./target/debug/phl-pack help`。桌面端 `pack/{export,install}.rs` 已变薄壳复用同一 core。 |
| 6 | **P2-2：`phl-export` DSH Skill 文档** | `docs/skills/phl-export/SKILL.md`（侦察→分类→敏感逐项确认→组布局→CLI；铁律不发明格式）。 |
| 7 | **§32/§33 手测矩阵登记** | 维护者本地手测清单新增 §11–§14（不随本仓库发布）。⚠️ 仅登记，执行未完成（见下）。 |
| 8 | **【Review 抓到并修复】凭据文件层泄漏** | 见下方专门条目。 |
| 9 | **全量门禁** | Rust workspace 268 通过 / 5 忽略；clippy `-D warnings`、fmt 干净；前端 typecheck 0 错、13 文件 / 73 测试通过、build 成功。 |
| 10 | **M3 catalog store composition** | `catalogStore.ts` 只保留加载、组合和 lookup（91 行）；version/runtime/plugin actions、通知策略和共享 transfer queue 分文件。原 `useCatalogStore` API 不变，catalog/update 相关 11 项测试通过。 |
| 11 | **M5 API config 模块边界** | `api_config/mod.rs` 从 1200+ 行降为 147 行编排壳；types/validation/launch_keys/tests 独立，API config 41 项测试通过。 |
| 12 | **R7 阻塞内协作取消** | Pack export/install 使用唯一 transfer id；扫描、压缩、完整性哈希与解包每 64 KiB 检查取消；三个大文件取消测试通过，解包取消删除半成品；UI 有「取消导出/安装」。 |
| 13 | **R6 前后基准** | `scripts/pack-benchmark.ps1` 对比 `f17030e` 与当前 release：64 MiB + 1000 文件、3 次中位数，build/validate/unpack 峰值工作集分别从 133.59/130.57/128.29 MiB 降至 6.20/5.33/5.33 MiB。详见 `PERF_BASELINE.md`。 |
| 14 | **bridge / 浏览器 Mock 回归** | `npm run bridge:check`：17 个 bridge 文件、71 个 invoke 无缺失 handler/重复注册。Chromium 回归设置切换、实例创建、插件搜索/详情/安装/启停/更新页，修复后控制台零 warning/error、`button button = 0`。 |
| 15 | **Windows 本机安装包构建** | `npm run app:build` 成功，生成 `PHL_0.1.0_x64-setup.exe`（2,880,367 B）；这证明可构建，不等于安装后真机链路已经验收。 |

## 🔎 Review 抓到并已修的缺陷（建议重点验收）

规格 §12 要求「`.env` 中明显凭据不得进包」，但此前导出**只剥离 env 键值**，
插件目录里的 `.env`/SSH key 会随 `add_tree` 原样进包，而 manifest 仍撒谎标
`secretsExcluded: true`。已在 core 加**文件名级、零误伤**过滤
（`is_secret_entry_name`：`.env` 排除 `.example/.sample/.template/.dist`、
`id_*` 私钥、`credentials.json`/`token.json`、`.npmrc/.netrc/.pypirc`），
预览与导出都会点名扣下了什么；`.env.example` 等模板仍照常进包。

- **最快验收（命令行，无需桌面）**：造一个布局目录，`embedded/plugins/mine/` 下放
  `.env`（含一行 `OPENAI_API_KEY=sk-...`）与 `.env.example`，跑
  `phl-pack build <目录> out.phlpack` → 输出应含「已按规格 §12 扣下 1 个疑似凭据文件：…/.env」，
  且 `.env.example` 仍在；再 `phl-pack unpack out.phlpack dump/` 后 `grep -r sk- dump/`
  应搜不到。
- **桌面端**：给一个将被嵌入的本地插件目录放 `.env` → 导出预览 warning 点名
  「插件 X 含疑似凭据文件，无论是否嵌入都不会打包」→ 导出后 toast 列「已扣下」。
- 单测：`cargo test -p phl-pack-core -- secret`（`secret_names_are_exact_families_not_substrings`、
  `build_from_dir_withholds_secret_shaped_files_and_reports_them`）。

## ⏳ 未完成 — 需真机 / 决策

| 任务 | 状态 | 说明 |
|------|------|------|
| §11–§14 手测矩阵**执行** | 未开始 | 已登记进清单，但需你在 Windows 桌面 `npm run app:dev` 实跑，尤其「真实 DSH 启动后能看到迁移/安装的会话」只有真机能证。 |
| macOS/Linux **真机**发现验证 | 未开始 | 代码+单测就绪，但 Finder/桌面启动 PATH 行为要在真实 mac/linux 机器跑——属 L-12 专项。 |
| `phl-pack` 发布通道 | 未做 | 本轮已构建本地 release CLI，但仍未随安装包分发；Skill 用前需自取二进制。归 O-01 CI 后续。 |
| 子代理血缘断链 | 有意留白 | Selected 只选了子代理对话却没选其父 → 迁过去 `parentSession` 指向不存在 id；当前允许，真机反馈若要求再收紧。 |
| 非典型文件名的硬编码密钥 | 设计上不覆盖 | 如 `config.js` 里写死 token——三层都不识别，靠「含会话必须提示隐私」兜底（§33 明确不假装能扫正文）。 |
| **git 提交** | 未做 | 所有改动仍在工作树（叠加上一轮未提交的 P0/P1 改动），等待你决定是否整理成一次提交。 |

## 一条命令自证全绿

```bash
# Rust
cd "D:\AI项目\dsh-phl\src-tauri"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 前端
cd "D:\AI项目\dsh-phl"
npm run typecheck && npm test && npm run build
npm run bridge:check
```

## 本轮改动文件索引

- **后端**：`instances/adoption.rs`（Selected 闭环 + `list_adoption_sessions`）、`sessions/mod.rs`、
  `discovery/{mod,windows,macos,linux,inspect}.rs`、`lib.rs`（命令注册）、
  `pack/{mod,export,install}.rs`（接回 core + 凭据过滤 + `[state]` 边界）、删除 `pack/{format,write,tests}.rs`。
- **新 crate**：`src-tauri/crates/phl-pack-core/`（`lib.rs`+`format.rs`+`write.rs`+`unpack.rs`）、
  `src-tauri/crates/phl-pack-cli/`（`main.rs`）。
- **workspace**：`src-tauri/Cargo.toml`（加 `[workspace]` + 成员 + `phl-pack-core` path 依赖）。
- **前端**：`lib/desktopAdoption.ts`、`lib/desktopSessions.ts`、`lib/desktop.ts`、`lib/desktopPack.ts`、
  `stores/adoptionStore.ts`、`stores/adoptionStore.test.ts`、`pages/AdoptDshPage.tsx`、
  `pages/InstanceDetailPage.tsx`、`pages/InstancesPage.tsx`、`pages/PackExportPage.tsx`、
  `components/instance/SessionCopyPanel.tsx`。
- **文档**：`docs/p0-local-dsh-adoption.md`、`docs/p1-session-pack.md`、`docs/p2-pack-core-skill.md`（新）、
  `docs/skills/phl-export/SKILL.md`（新）；进度清单与手测矩阵（§11–§14）由维护者在本地工作区维护，不随本仓库发布。
