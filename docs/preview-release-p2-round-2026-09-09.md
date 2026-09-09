# P2 收口 · 2026-09-09

## 判定

首轮 review 列为「建议公开发包前一并修复」的四项 P2 已全部处理：P2-1、P2-3、P2-4 修复并配回归；
P2-2 按 review 给出的第二条路径**把承诺收紧到与实现一致**（回滚不重新绑定 DSH/Runtime）。

公开预览仍为 **NO-GO**，但剩余条件全部是验收类（真机安装、故障注入、供应链审计），不是已知代码缺陷。

## 证据口径

- 门禁 `npm run test:alpha` 全绿：版本、typecheck、bridge、前端、runner、fmt、clippy、Rust workspace。
- Rust workspace **352 项通过 / 0 失败 / 6 忽略**（桌面库 316）；前端 **92 项 / 15 文件**。
- 新增 3 项回归都在产品测试集中，不依赖临时脚本。

## 逐项

### P2-1 · 修改已有密钥后保存失败会丢旧密钥 —— 已修

- 问题：`save_api_config_at` 先把新密钥写进凭据库，再提交 `api.json`；提交失败时旧配置指向的已是新密钥，
  而错误文案仍说「旧配置与凭据保持不变」（`api_config/library.rs`）。
- 修法：写库之前记录每个将被替换的密钥；写库失败或文件提交失败时把它们放回（无旧值则删除），
  放不回时错误里点名供应商并提示重新输入。
- 回归：`a_failed_commit_puts_the_previous_key_back`；既有 `store_failure_aborts_before_any_file_change` 继续成立。

### P2-2 · 快照不恢复 DSH/Runtime 绑定 —— 按 review 第二条路径收口

- 决定：**不重新绑定**。快照记录的版本/运行时可能已被删除，把实例指向不存在的安装会把「可恢复」换成「跑不起来」。
- 改动：`snapshot.rs` 的模块文档与 `SnapshotFile` 注释不再声称「a restore brings these back」；
  前端空状态与回滚确认框写明「回滚只替换 dsh-home（插件与配置），不改变实例引用的 DSH 与 Runtime」。
- 影响：界面承诺与实现一致；不再有「回滚后版本也回到快照时刻」的误解。

### P2-3 · 快照交换中断无自动恢复 —— 已修

- 问题：还原先 `rename current → .phl-old-dsh-home` 再放回快照；两步之间崩溃后实例没有 dsh-home，
  下一次还原以「实例缺少 dsh-home」拒绝，唯一副本留在隐藏目录（`instances/snapshot.rs`）。
- 修法：新增 `recover_interrupted_restore`——备份在而 home 不在就把备份放回；home 在则退役备份；顺带清 `.phl-restore`。
  在快照创建与还原路径调用，且排在外部实例写入拒绝之后。
- 回归：`an_interrupted_restore_is_rolled_back_instead_of_refused`。

### P2-4 · 快速重启复用旧 WebUI URL/token —— 已修

- 问题：窗口已存在时只 unminimize/show/focus，新进程的端口与 token 被忽略，窗口仍显示上一次的地址（`webui.rs`）。
- 修法：先比较窗口当前 URL 与请求 URL，不同则 `navigate` 到新地址再聚焦。
- 回归：`a_restarted_instance_reuses_the_window_but_not_the_url`。

## 未关闭（放行前仍需完成）

- 干净 Windows 环境的安装 / 首启 / 卸载与完整业务链路（含非管理员安装与 WebView2 前提）。
- 真实断电 / 硬退出故障注入。
- `cargo audit`（工具未安装）。
- 安装包需在当前提交上重建——上一份产物早于本轮修复。
