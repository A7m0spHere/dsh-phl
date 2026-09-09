## 安装提示：首次下载可能被浏览器拦截

这个安装包**尚未做代码签名**。浏览器和 Windows SmartScreen 会对任何没有下载信誉的可执行文件给出
「通常不会下载 PHL_0.1.0-alpha.1_x64-setup.exe」这类提示——**这不是检测到病毒，而是缺少签名与信誉**。

**继续下载**

- Edge：下载项右侧 `⋯` → 「保留」
- Chrome：下载栏 → `⋯` → 「保留」

**先校验再运行（推荐）**

下载后核对 SHA-256，与同一 Release 的 `PHL_0.1.0-alpha.1_x64-setup.exe.sha256` 一致再运行：

```powershell
Get-FileHash .\PHL_0.1.0-alpha.1_x64-setup.exe -Algorithm SHA256
```

**运行时若出现「Windows 已保护你的电脑」**：点「更多信息」→「仍要运行」。
这是 SmartScreen 对未知发布者的默认行为，与文件本身无关。

签名方案与进度见仓库的 [docs/code-signing.md](https://github.com/A7m0spHere/dsh-phl/blob/main/docs/code-signing.md)；
拿到证书之前，上述提示不会消失。
