# 应用内自动更新 · 2026-09-09

## 它怎么工作

PHL 用 Tauri 的 updater 插件检查更新，流程是：

1. 启动后 8 秒（可在 设置 → 关于 关闭）静默读取一份**清单**；
2. 清单里写了最新版本号、说明、安装包地址与**签名**；
3. 插件用内置的公钥（`tauri.conf.json` → `plugins.updater.pubkey`）验证签名，**验不过就不下载**；
4. 有更新时弹一条提示；真正的下载与安装只在你点「下载并安装」后发生；
5. Windows 上安装器会静默替换文件并重启应用。

签名用的是 minisign（不是 Authenticode）。两者用途不同：Authenticode 解决「用户下载时会不会被拦」，
minisign 解决「更新包是不是我们签的」——所以即使安装包仍未做 Authenticode 签名，自动更新依然是安全的。

## 清单地址

```text
https://raw.githubusercontent.com/A7m0spHere/dsh-phl/updates/latest.json
```

发布流水线在打 `v*` 标签时把 `latest.json` 推到 `updates` 分支——分支里只有这一个文件，
首次建立时会自动 `git init` 并配好 remote。用分支而不是 `releases/latest/download/latest.json`，
是因为后者只指向**最新正式版**，而 PHL 目前发的是 prerelease。

清单同时作为资产附在 Release 上（与 `.sig`、`.sha256` 并列），便于人工核对；应用实际读的是分支上那一份。

## 密钥

| 用途 | 值 |
|---|---|
| 公钥 | 写在 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`（可公开） |
| 私钥 | **只存于 GitHub Secrets**：`TAURI_SIGNING_PRIVATE_KEY`；有密码时再加 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |

私钥文件（本机）在 `%USERPROFILE%\.phl\updater.key`，**不要提交、不要外传**。丢了就无法再签发可被旧版本接受的更新，
用户只能手动下载新版；那时可以换新密钥，但**旧版本不会接受新密钥签的包**（公钥烧在旧版本里）。

重新生成（仅在确认要换密钥时）：

```powershell
npx tauri signer generate -w $env:USERPROFILE\.phl\updater.key --ci
# 把新的 .pub 内容填进 tauri.conf.json，并把 .key 内容加进 GitHub Secret
```

## 发版时要注意

1. `docs/release-notes.md` 会变成更新提示里显示的说明（`update.body`）；
2. 流水线需要 `TAURI_SIGNING_PRIVATE_KEY` 这个 Secret，否则**不会产生** `latest.json` 与 `.sig`；
3. 更新只能从**已带 updater 的版本**开始：装了 alpha.3 的用户能自动更新到 alpha.4，
   alpha.2 及更早的用户需要手动装一次 alpha.3。

## 已知限制

- 安装包仍未做 Authenticode 签名，首次下载与运行仍有 SmartScreen 提示（见 [code-signing.md](./code-signing.md)）。
- 更新包的下载走 GitHub Releases 直链；网络受限的环境可能很慢（版本下载已有镜像逻辑，更新包暂未接）。
- 目前只在 Windows 上验证；其它平台的更新需要各自的安装包形态。
