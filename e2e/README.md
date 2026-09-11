# e2e/ — Windows 安装器冒烟车道

无头核心门禁（J1–J5、F1–F5）住在 `src-tauri/src/release_e2e/`，由
`cargo test --workspace -- --ignored release_e2e --test-threads=1` 运行。
本目录是第二条车道：装真包、开真窗、走真 OS。

| 文件 | 用途 |
|---|---|
| `cdp-spike.mjs` | 可行性验证（CI 前手动跑一次）：确认 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port` + raw CDP 能起窗、过桥、真实 IPC 往返。**2026-09-11 本机已验证 PASS。** |
| `install-smoke.mjs` | 完整冒烟：静默安装 `/S` → exe 版本元数据 → CDP 首启（窗口可见即 `app_ready` 握手完成的证据）→ `list_instances` 真实往返 → 3 秒 console 异常捕获 → updates 清单一致性 → 覆盖升级 → 静默卸载 → 用户数据保留承诺。`--installer` 全模式 / `--exe` 仅运行模式。 |

统一入口：`npm run test:e2e:smoke -- --installer=<setup.exe> --expect-version=<V>`。

## 为什么是 raw CDP 而不是 WinAppDriver/tauri-driver

仓库此前没有任何 GUI 驱动。WebView2 的 DevTools 端口允许用 Node 内建
`WebSocket` + `fetch` 直接 `Runtime.evaluate`，在页面上下文调用
`window.__TAURI_INTERNALS__.invoke`——真实后端、真实窗口、零新依赖、
无需屏幕可读。安装器行为（注册表、文件、卸载）走 PowerShell/reg 断言，
同样不需要 UI 自动化。

## 注意事项

- **只在一次性环境跑 `--installer` 全模式**：它会真实安装并**静默卸载**
  PHL。在开发机上跑会卸掉你已安装的版本（run-only 的 `--exe` 模式无此副作用）。
- debug 构建的 exe 会加载 `devUrl`（Tauri 的 debug 行为），冒烟必须用
  `tauri build --no-bundle`/release 产物或安装包。
- 退出时序：脚本用 `node:https` + `Connection: close` 取清单（undici 的
  keep-alive socket 与 Windows 上的 `process.exit` 竞争会在退出时报
  `uv async.c` 断言，已规避）。
- 报告落在 `--out`（CI 里是 `e2e-report/`，作为 artifact 上传）。
- I-6 与核心车道 J5 的失败分类：**传输失败（断网/超时）→ SKIP**，**契约失败**
  （端点答复非 200、JSON 不合法、版本倒退、keyid 不匹配）→ FAIL。发布时若真断网，
  Release 后续步骤自然失败，门禁无需抢跑报错。
