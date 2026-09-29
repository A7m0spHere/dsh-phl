## 安装与校验

从下方 Assets 下载安装包。**它尚未做代码签名**，浏览器与 Windows SmartScreen 会对任何没有下载信誉的
可执行文件给出「通常不会下载 …」提示——**这不是检测到病毒，而是缺少签名与信誉**。

**继续下载**：Edge 点下载项右侧 `⋯` → 「保留」；Chrome 点下载栏 `⋯` → 「保留」。

**先校验再运行（推荐）**：下载后核对 SHA-256，与同一 Release 的 `.sha256` 文件一致再运行。

```powershell
Get-FileHash .\PHL_*_x64-setup.exe -Algorithm SHA256
```

**运行时若出现「Windows 已保护你的电脑」**：点「更多信息」→「仍要运行」。
这是 SmartScreen 对未知发布者的默认行为，与文件本身无关。

签名方案与进度见 [docs/code-signing.md](https://github.com/A7m0spHere/dsh-phl/blob/main/docs/code-signing.md)；
拿到证书之前，上述提示不会消失。

## macOS 安装（Apple Silicon）

- 下载 `PHL_<version>_aarch64.dmg`，校验方式同上（macOS 用 `shasum -a 256 PHL_*_aarch64.dmg`）。
  当前仅提供 Apple Silicon 版本，Intel Mac 未做真机验收。
- 安装包**未签名未公证**：首次打开会被 Gatekeeper 拦下（「无法打开，因为无法验证开发者」）。
  **这不是检测到恶意软件，而是缺少签名与公证**。处理：在 Finder 中对 PHL **右键 →「打开」→ 再点「打开」**；
  若仍被拦，到「系统设置 → 隐私与安全性」底部点「仍要打开」。
- 挂载 DMG 后把 PHL 拖入「应用程序」即可。macOS 版**暂无应用内自动更新**，升级需手动下载新 DMG 覆盖。

## 反馈

- 问题与建议：<https://github.com/A7m0spHere/dsh-phl/issues>
- 安装包不签名、SmartScreen/Gatekeeper 提示等已知情况：见上。
- 这个版本是 rc 预览版，界面与数据格式仍可能变化。
