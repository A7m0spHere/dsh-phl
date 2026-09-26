# e2e/ — Windows 安装器冒烟车道

无头核心门禁（J1–J5、F1–F5）住在 `src-tauri/src/release_e2e/`，由
`cargo test --workspace -- --ignored release_e2e --test-threads=1` 运行。
本目录是第二条车道：装真包、开真窗、走真 OS。

| 文件 | 用途 |
|---|---|
| `cdp-spike.mjs` | 可行性验证（CI 前手动跑一次）：确认 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port` + raw CDP 能起窗、过桥、真实 IPC 往返。**2026-09-11 本机已验证 PASS。** |
| `install-smoke.mjs` | 完整冒烟：静默安装 `/S` → exe 版本元数据 → CDP 首启（窗口可见即 `app_ready` 握手完成的证据）→ `list_instances` 真实往返 → 3 秒 console 异常捕获 → updates 清单一致性 → 覆盖升级 → 静默卸载 → 用户数据保留承诺。`--installer` 全模式 / `--exe` 仅运行模式。 |
| `lib/tauri-cdp.mjs` | 两条 GUI 车道共用的 raw-CDP 客户端、按文本/状态等待的页面助手、IPC 录制器（`POST http://ipc.localhost/<cmd>` 的原始请求体）。 |
| `dependency-retry-cdp.mjs` | **依赖重试失败路径证据**（R4-01）：真实窗口打开待补依赖实例 → 记录里的历史原因可见 → 点「重试依赖安装」→ 断言出现失败提示与真实原因、**绝不出现成功**、实例保持原状态、新原因已持久化；`--scenario=corrupt` 另跑损坏记录：卡片明示不可本地重建 + 「重新导入整合包」真的到达整合包安装页。 |
| `trial-copy-cdp.mjs` | **试升级页面链路证据**（R1·M3 / R3-01）：在隔离 `PHL_ROOT` 里预置两个版本 + Runtime + 源实例，起窗后点真实 UI（实例菜单 → 复制并试用新版 → 创建副本 → 打开副本），从 IPC 传输（`POST http://ipc.localhost/<cmd>`）**原样记录**页面发出的 `preview_trial`/`create_trial` 请求，最后落证据 JSON + 截图。 |

统一入口：`npm run test:e2e:smoke -- --installer=<setup.exe> --expect-version=<V>`；
页面链路证据：`npm run test:trial-cdp` 与 `npm run test:dep-retry`（均支持
`--keep` 保留隔离根、`--exe=<path>` 换 exe、`--label=<名>` 决定证据文件名；
`--no-vite` 用于 release exe 的内嵌页面，`--scenario=pending|corrupt` 选失败路径场景）。

## 真实上游车道（Rust 侧，opt-in）

`src-tauri/src/acceptance/` 里两条车道会真的联网下载并启动服务，**刻意不在 CI 门禁内**，
必须用 `PHL_REAL_DSH=1` 显式开启（未设置时打印 SKIP）：

```bash
PHL_REAL_DSH=1 cargo test --lib real_dsh_upstream -- --ignored --nocapture --test-threads=1
PHL_REAL_DSH=1 cargo test --lib real_runtime    -- --ignored --nocapture --test-threads=1
```

前者：真实 npm 安装 `0.1.7-rc.2` / `0.1.5-rc.3` → 启动 → 真实认证握手 → 双实例并存 → 停止。
后者：真实安装两个 Node Runtime，同一 DSH 的两个实例各绑一个 Runtime 同时运行。
真实 DSH 的认证流程（303 + `dsh-auth-*` cookie → 200；无 cookie 401）见
`internal/acceptance/next-release/rework-report-round5.md` §3.1。

## 为什么是 raw CDP 而不是 WinAppDriver/tauri-driver

仓库此前没有任何 GUI 驱动。WebView2 的 DevTools 端口允许用 Node 内建
`WebSocket` + `fetch` 直接 `Runtime.evaluate`，在页面上下文调用
`window.__TAURI_INTERNALS__.invoke`——真实后端、真实窗口、零新依赖、
无需屏幕可读。安装器行为（注册表、文件、卸载）走 PowerShell/reg 断言，
同样不需要 UI 自动化。

`trial-copy-cdp.mjs` 补两点：命令行走真实点击（按可见文本/aria-label 选元素），
请求体走 **CDP Network 事件**——不是补丁 `invoke`，因为
`window.__TAURI_INTERNALS__` 的成员是不可写不可配置的，而 `chrome.webview.postMessage`
也不是这套构建的 IPC 传输；实测真正的传输是 `POST http://ipc.localhost/<command>`。

## 注意事项

- **只在一次性环境跑 `--installer` 全模式**：它会真实安装并**静默卸载**
  PHL。在开发机上跑会卸掉你已安装的版本（run-only 的 `--exe` 模式无此副作用）。
- debug 构建的 exe 会加载 `devUrl`（Tauri 的 debug 行为），冒烟必须用
  `tauri build --no-bundle`/release 产物或安装包。`trial-copy-cdp.mjs` 反过来
  利用这一点：debug exe 需要 Vite，脚本自己拉起/收掉 dev server（`--no-vite` 可关）。
- 退出时序：脚本用 `node:https` + `Connection: close` 取清单（undici 的
  keep-alive socket 与 Windows 上的 `process.exit` 竞争会在退出时报
  `uv async.c` 断言，已规避）。
- 报告落在 `--out`（CI 里是 `e2e-report/`，作为 artifact 上传）。
- I-6 与核心车道 J5 的失败分类：**传输失败（断网/超时）→ SKIP**，**契约失败**
  （端点答复非 200、JSON 不合法、版本倒退、keyid 不匹配）→ FAIL。发布时若真断网，
  Release 后续步骤自然失败，门禁无需抢跑报错。
