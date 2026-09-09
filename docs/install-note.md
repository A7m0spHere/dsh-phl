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

## 反馈

- 问题与建议：<https://github.com/A7m0spHere/dsh-phl/issues>
- 安装包不签名、SmartScreen 提示等已知情况：见上。
- 这个版本是 alpha，界面与数据格式仍可能变化。
