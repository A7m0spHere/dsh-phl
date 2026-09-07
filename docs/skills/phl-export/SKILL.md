---
name: phl-export
description: >-
  Export the current DSH environment into a shareable `.phlpack` PHL 整合包.
  Use when the user asks to 导出整合包 / 打包当前 DSH 环境 / share their DSH
  setup with another PHL user. Detection-based: inspects this DSH's own files,
  classifies its plugins, asks the user about privacy-sensitive content, and
  calls the `phl-pack` CLI to build and self-validate the archive.
---

# phl-export — 把当前 DSH 导出为 `.phlpack`

面向「没有使用 PHL、但已经有自己的 DSH」的用户(开发规格第 14 部分)。
你是运行在 DSH 里的 Agent;本 Skill 指导你把自己所在的环境打成一个
可以发给朋友的 `.phlpack`。

**铁律(规格 §14.2):你不发明 Pack 格式,也不手写 ZIP。**
归档的生成与校验只有一个出口:`phl-pack` CLI(它与 PHL 桌面端共用同一个
`phl-pack-core` crate)。你负责**侦察、组装布局目录、征得同意、调用 CLI**;
CLI 负责 schema、integrity、打包与自校验。CLI 不可用时,告诉用户安装
`phl-pack` 并停止,不要退化为手工 zip。

## 第 0 步 — 确认 `phl-pack` 可用

```bash
phl-pack help
```

不存在 → 终止并说明(整合包必须由共享 Pack Builder 产出)。

## 第 1 步 — 定位并侦察本环境(只读)

DSH_HOME 解析优先级(与 DSH 自身一致,规格 §2.2;**不要全盘搜索**):

1. 会话环境里显式的 `DSH_HOME`(空值视为未设置);
2. `~/.dsh`。

只读取以下事实,读不到就如实标注未知,不要猜:

| 信息 | 来源 |
|---|---|
| DSH 版本 | `<home>/profiles/<profile>/node_modules/@deepseek-ai/dsh-base/package.json` 的 `version`(profile 目录下 store 优先,其次共享 `profiles/node_modules` store) |
| Profile | `web` 存在则用 `web`,否则第一个普通 profile 目录(跳过 `node_modules` 与点开头目录) |
| 已装插件 | profile `package.json` 的 `dsh.profile.bundles` ∪ `cordis.patch.yml` 顶层 `- id:` 行;排除 `@deepseek-ai/` 核心作用域;只统计 `node_modules/<name>` 真实存在的 |
| Node 版本 | `node --version` |
| 历史对话数 | `<home>/sessions/<project>/<session-*>/session.jsonl[.zstd]` 的文件名遍历;**不读取会话内容** |

## 第 2 步 — 插件分类(规格 §8–§10)

对每个已装插件判断来源:

- **有 registryId / 包名是公开 npm 包** → `registry` 条目(包内不带文件,
  安装方重新下载)。
- **本地写/改的、私有、无公开下载渠道** → 建议 `embedded`(把目录整个打进
  包里)。判断依据:`node_modules/<name>/package.json` 有 `"private": true`、
  名字不像已发布包、或存在 `file:`/link 安装痕迹。

**许可证(规格 §10)**:读 `package.json` 的 `license`/`licenses` 与 `LICENSE`
文件。读不到时,向用户展示:

> PHL 无法确认该插件是否允许重新分发。请确认你拥有分享该文件的权利。

不要自动判定「合法」或「非法」。

## 第 3 步 — 敏感项询问(规格 §9/§11/§16;**必须让用户确认,不能默认放行**)

逐项询问用户:

1. **嵌入哪些本地插件**(默认:推荐的全部,列清单让用户勾选)。
2. **是否包含历史对话**:

   - 默认**不包含**。
   - 勾选包含前,原样展示隐私警告(规格 §11):

     > 历史对话可能包含:用户输入、文件内容、项目路径、Tool 调用结果、
     > 私有代码、Token 或其他敏感信息。
     > 分享整合包前请确认你了解相关隐私风险。

   - 用户确认后**再次确认**(两拍;规格 §11「然后再次确认」)。
   - 只从 `<home>/sessions` 整目录复制会话文件,不编辑内容;
     **不要假装能可靠识别对话正文里的秘密** —— 提示责任归用户(规格 §33)。
3. **任何不认识的顶层文件**:不打包,除非用户点名包含(规格 §16「包含未知文件」)。
4. **输出文件已存在**:展示路径并请用户确认覆盖;`phl-pack build` 本身
   拒绝覆盖(见第 5 步:经确认后由你先移走旧文件再构建)。

## 第 4 步 — Secrets 边界(规格 §12;不可协商)

以下内容**无论如何不得进入包**:API Key、Registry Token、credential store、
SSH key、`.env` 中明显凭据。

因此布局目录**不放** `settings.yaml` 原文(里面通常有模型配置与凭据)。
`phlpack.json` 里也不写任何凭据值 —— 只可写凭据**名字**,放
`content.note` 或说明性字段,提示安装方「需重新配置凭据」。

**兜底(不是许可证)**:`phl-pack build` 会按名扣下 `.env`(非 `.example`)、
`id_rsa` 系、`credentials.json`/`token.json`、`.npmrc`/`.netrc` 等明显凭据文件,
并在输出里列出"已扣下"。这是纵深防御,**不能替代**你第 4 步的自觉清理 —— 凭据
若藏在非典型文件名里(如 `config.js` 里写死 key),CLI 不识别、也不承诺识别
(规格 §33:不假装能扫自然语言正文)。

## 第 5 步 — 组装布局目录并交给 CLI(规格 §6.1/§7)

在临时位置组装(以下都是可选段落,按需出现):

```text
<staging>/
├── phlpack.json              # 你按下方 schema 手写(唯一由 Skill 产出的文件)
└── embedded/plugins/<id>/    # 第 2 步勾选的本地插件整目录(排除其中的
                              #   .env/凭据文件 —— 打包前逐个检查)
└── sessions/                 # 用户双确认后,从 <home>/sessions 整目录复制
```

`phlpack.json` 最小必填(formatVersion 固定 1):

```json
{
  "formatVersion": 1,
  "pack":    { "id": "<kebab-id>", "name": "<展示名>", "version": "1.0.0",
               "author": "<可选>", "description": "<可选>", "createdAt": "<ISO>" },
  "dsh":     { "version": "<第 1 步读到的版本,必填>" },
  "runtime": { "kind": "node", "nodeVersion": "22", "arch": "x64" },
  "plugins": [
    { "id": "a", "version": "1.2.0",
      "source": { "type": "registry", "registryId": "@scope/a" } },
    { "id": "b", "version": "0.1.0",
      "source": { "type": "embedded", "path": "embedded/plugins/b" } }
  ],
  "content": { "sessionsIncluded": false, "secretsExcluded": true }
}
```

注意:`embedded` 插件的 `path` 必须是包内相对路径(不能绝对、不能 `..`);
声明了 path 就必须真实存在 `<path>/package.json`,否则 CLI 会以 consistency
错误拒绝。`content.sessionsIncluded` 必须与 `sessions/` 实际存在与否一致。

构建 + 自校验(CLI 会重读归档、校验穿越/软链/大小/重复/版本/integrity,
并为所有载荷文件自动补 sha256 —— 你不需要也不应该自己算哈希写进 manifest):

```bash
phl-pack build "<staging>" "<输出>.phlpack"
phl-pack validate "<输出>.phlpack"
```

覆盖已存在的输出:`mv` 旧文件(经用户确认)后重新 build,不使用静默覆盖。

## 第 6 步 — 向用户汇报

展示 Preview 式摘要(规格 §13.2 的口径):包名与版本、DSH 版本、Node、
在线/内置插件数量、历史对话包含与否、凭据已排除、以及
`phl-pack validate` 的通过信息。告诉用户文件在哪,可以如何分享
(对方在 PHL 实例页「安装整合包」导入)。

## 禁止事项汇总

- 不发明/扩展 Pack 格式,不手工打 ZIP,不计算 integrity 哈希(全部交给 CLI)。
- 不扫描整块磁盘;只认第 1 步的 DSH_HOME 解析规则。
- 不读取会话正文(计数是文件名遍历)。
- 不把凭据(值)写进任何包内文件;不确定就不带。
- 未经第 3 步逐项询问,不得包含会话、本地插件、未知文件或覆盖输出。
