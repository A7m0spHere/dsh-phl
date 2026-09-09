# 公开预览发布就绪记录 · 2026-09-09

## 判定

**代码与供应链检查已就绪；真机验收由维护者自测通过（有问题的走 issue 跟踪）。**

此前放行清单里的每一项要么已关闭，要么已明确移交：

| 放行条件 | 状态 |
|---|---|
| R1–R4 代码缺陷 | 已关闭（含回归） |
| V1–V4 复验遗留 | 已关闭（含失败时序回归） |
| 四项 P2（凭据 / 快照语义 / 快照中断 / WebUI 代次） | 已关闭（含回归，P2-2 按收紧承诺处理） |
| `npm run test:alpha` | 全绿 |
| `cargo audit` | **0 个漏洞**（7 条警告见下） |
| 干净 Windows 安装 / 首启 / 卸载 | 维护者自测通过；后续问题走 issue |
| 真实断电 / 硬退出注入 | 未做，同上按 issue 跟踪 |
| 安装包 | 已在本轮修复之上重建（见下） |

## 验证数字

- Rust workspace：**352 项通过 / 0 失败 / 6 忽略**（桌面库 316、Pack CLI 3、Pack Core 33）。
- 前端：**92 项 / 15 个文件**。
- Bridge：无缺失 handler、无重复注册。
- `npm run test:alpha` 八步全绿。
- 发布副本内另跑：`npm test`、`npm run build`、`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test`。

## 供应链：cargo audit

`cargo audit --file src-tauri/Cargo.lock`（513 个 crate 依赖）：**exit 0，0 个漏洞**。

7 条警告均为「无人维护 / unsound」类别，不构成已知可利用漏洞：

| crate | 类型 | 公告 |
|---|---|---|
| proc-macro-error 1.0.4 | unmaintained | RUSTSEC-2024-0370 |
| unic-char-property 0.9.0 | unmaintained | RUSTSEC-2025-0081 |
| unic-char-range 0.9.0 | unmaintained | RUSTSEC-2025-0075 |
| unic-common 0.9.0 | unmaintained | RUSTSEC-2025-0080 |
| unic-ucd-ident 0.9.0 | unmaintained | RUSTSEC-2025-0100 |
| unic-ucd-version 0.9.0 | unmaintained | RUSTSEC-2025-0098 |
| glib 0.18.5 | unsound | RUSTSEC-2024-0429（`VariantStrIter` 的 Iterator 实现） |

全部为传递依赖：`proc-macro-error` / `unic-*` 来自构建期或解析链路；`glib` 只出现在 Linux/GTK 目标，
Windows 构建不编译它。**不引入 `audit.toml` 豁免**——保持每次审计都完整可见。

## CI

- `96546fc`（本轮 P2 修复）：CI 全绿。
- `cb04137`（许可证）：CI 全绿。
- `b8a8c91`（大同步）：Rust job 的 `cargo test --workspace` 一次失败（exit 101），
  但**同一份 Rust 代码**在下一个提交上通过。本地连跑 10 轮 `cargo test --workspace` 全绿，无法复现。
  判定为 runner 侧偶发（历史上 runner 的短/长路径问题也出现过一次）；失败用例名需要仓库鉴权才能取日志，
  若再次出现应优先取日志定位。

## 产物

安装包在本轮修复之上重建（构建时工作树等于 `acdb74e`，仅多了本记录文档）：

```text
src-tauri/target/release/bundle/nsis/PHL_0.1.0-alpha.1_x64-setup.exe
  2,978,500 字节   2026-09-09 20:48
  SHA-256 5B72B49F2202A56554B30894FC60E2E060B082E656F61938CE04F0A11CE07837
```

正式发布走标签触发的 `.github/workflows/release.yml`：推 `v0.1.0-alpha.1` 后由 runner 重新构建、
