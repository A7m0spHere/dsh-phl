# Instance Manager 开发计划

> 下一阶段：把 Instance 从内存对象变成磁盘上真实存在的目录。
> 对应 `dsh-phl-development-roadmap.md` §9.3 / §14 的 Instance Manager。

---

## 1. 为什么是这个模块

### 两份文档的顺序不一致

| 文档 | Version Manager 之后 |
|---|---|
| `dsh-phl-development-roadmap.md` §14 | **Instance Manager** → Runtime Isolation → Process/Port → Launch → Plugin Manager |
| `dsh-phl-project-master-plan.md` §15 | Runtime Manager → Instance Manager → … → Plugin Manager |

采用路线图的顺序。理由不是文档权威性，而是下面这些**现在就存在的损坏**。

### Plugin Manager 提前落地造成的实际问题

两份文档都把 Plugin Manager 排在 Instance Manager **之后**，但它已经先实现了。于是插件安装管线正在往一个从未真正被创建过的实例目录里写文件：

```text
真实发生的事                                        实例的真实状态
─────────────────────────────────────────────────  ──────────────
install_plugin 创建 <root>/instances/<slug>/         该目录没有 instance.json
  dsh-home/profiles/default/node_modules/<pkg>/      没有归属信息
  并写入 cordis.patch.yml                            重启后实例本身消失
```

由此产生四个具体缺陷：

1. **重启丢实例，但不丢文件。** `instanceSeed = []` 且实例从不持久化，重启后实例列表为空 —— 而它的插件文件仍在磁盘上，UI 里再也看不到、也删不掉。
2. **删除实例不删文件。** `deleteInstance` 只是从数组里移除，磁盘上的 `node_modules` 永久残留，没有任何回收入口。
3. **占用统计是编的。** `diskUsage: 24_000_000 + 插件数 * 8_400_000` 是写死的公式，而真实字节已经躺在磁盘上。设置页「存储」整个视图建立在这个假数字上。
4. **记录与磁盘会漂移。** 实例的 `plugins[]` 是内存记录，`node_modules` 与 `cordis.patch.yml` 是磁盘事实，两者没有任何对账机制。

Runtime Manager 不解决上述任何一条 —— 它只解锁「启动」。**Instance Manager 是唯一能止血的模块**，同时它也是产品命题本身（Instance 是第一公民）。

---

## 2. 目标与非目标

**本阶段做到：**

- 实例在磁盘上真实存在，重启后仍在
- 创建 / 克隆 / 删除 / 改名与配置修改都落到磁盘
- 实例的插件列表由磁盘反推，而不是内存记录
- 占用统计用真实字节

**本阶段不做：**

- 真实启动 / 停止进程（Phase Process/Port + Launch）
- 端口探测与自动分配（只做「与其他实例的 instance.json 冲突检测」，不碰 OS）
- Runtime 的真实安装（实例仍可引用一个 mock runtime id）
- 快照（Snapshot 属于更后面的阶段，UI 保持空态）

---

## 3. 目录结构

沿用 `dsh-phl-development-roadmap.md` §7，并与已落地的插件路径保持一致：

```text
<root>/instances/<id>/
├── instance.json          # 清单，唯一的实例身份来源
├── dsh-home/              # DSH_HOME
│   └── profiles/<profile>/
│       ├── node_modules/  # 插件（已由 Plugin Manager 使用）
│       └── cordis.patch.yml
├── workspace/
└── logs/
```

两点说明：

- **目录名用 `id` 而不是 `name` 的 slug。** 同名实例必须允许存在，slug 会撞车。现有 `mockRepository` 用的是 slug，改为 id 是一次破坏性变更 —— 但由于实例从不持久化，磁盘上不存在需要迁移的合法数据。
- **`profiles/` 放在 `dsh-home/` 内部**，与 `tauriPlugins.pluginProfileRoot()` 现有实现一致（路线图 §7 画的是同级，但已落地的是内嵌，且更贴近 DSH 自身的 `~/.dsh/profiles/` 语义）。不要为了对齐一张示意图去改动已经在工作的插件路径。

**DSH 版本与 Runtime 不复制进实例目录。** 它们共享存放在 `<root>/versions/` 与 `<root>/runtimes/`，实例只按 id 引用 —— 多版本共存却不重复占用磁盘正是本项目的核心命题。当前 mock 的进度文案「解压 DSH 与 Runtime 到实例目录」描述的是错误模型，实现时需一并改掉。

---

## 4. Rust 侧：`src-tauri/src/instances.rs`

复用 `versions.rs` 已有的基础设施（`http_client` 无关，但 `now_iso`、`safe_join` 思路、`sanitize_*` 模式、`Channel` 进度、`Transfers` 取消标志都可直接沿用）。

### 命令

| 命令 | 职责 |
|---|---|
| `list_instances(root)` | 扫描 `<root>/instances/*/instance.json`，跳过无清单的目录 |
| `create_instance(root, manifest, template_plugins, on_progress)` | 建目录树 → 写 `instance.json` → 初始化 profile → 安装模板插件 |
| `clone_instance(root, source_id, manifest, on_progress)` | 复制源实例树（**排除** `logs/`、`.phl-cache/`），改写清单 |
| `delete_instance(root, id)` | 删除整棵树 |
| `save_instance(root, manifest)` | 覆写 `instance.json`（改名、端口、env、args、favorite） |
| `instance_disk_usage(root, id)` | 递归求和，供设置页「存储」使用 |
| `scan_orphan_instances(root)` | 列出 `<root>/instances/` 下缺少 `instance.json` 的目录 |

### 必须注意

- **`id` 必须经过与 `sanitize_version` 同规格的白名单校验**再拼进路径。它来自前端，是路径注入面。
- **创建走「暂存 + 重命名」**：先建 `.phl-new-<id>/`，成功后再 rename 成 `<id>/`。中途失败不留下半个没有清单的实例目录 —— 那正是当前孤立目录的成因。
- **删除前先确认目标在 `<root>/instances/` 之内**（规范化后比对前缀），绝不接受前端传入的任意路径。
- 克隆用递归复制；`node_modules` 可能很大，进度通过 `Channel` 汇报，并支持取消。

---

## 5. 插件列表由磁盘反推

这是本阶段最有价值的一处设计变更。

现状：实例的 `plugins[]` 是内存记录，与磁盘无对账。代码评审中修掉的「并发安装互相覆盖」「取消竞态产生幽灵安装」都只是在内存层面缓解，根因仍在。

改为：

```text
node_modules/<pkg>/phl-plugin.json   → 装了什么、什么版本（已由 install_plugin 写入）
cordis.patch.yml                     → 启用 / 停用
                    ↓
              InstalledPlugin[]
```

新增命令 `scan_instance_plugins(instance_root)`，`listInstances` 时逐实例调用。收益：

- 磁盘是唯一事实来源，记录不可能与实际漂移
- 外部手动装的插件会被自动发现
- 幽灵安装（记录没了、文件还在）自动消失

`instance.json` 中不再保存 `plugins[]`。

---

## 6. 前端改动

### 新增 `src/services/tauriInstances.ts`

覆盖 `listInstances` / `createInstance` / `cloneInstance` / `deleteInstance`，并在 `tauriVersions.ts` 的 `tauriRepository` 里展开，与 `tauriPluginOverrides` 同样的方式。

### `PhlRepository` 接口需要扩一个方法

```ts
/** 持久化实例配置的修改（改名、端口、env、args、收藏）。 */
saveInstance(instance: Instance): Promise<void>
```

当前 `instanceStore.updateInstance` 只改内存，重启即丢。加上这个方法后，`updateInstance` 需要在写内存后调用它落盘（失败要回滚并提示，不能静默丢弃用户的修改）。

### 需要一并修掉的既有问题

- **`templateSeed` 引用的是虚构插件 id**（`dsh-core/routing-suite`、`lumen/trace-export` 等），而 `createInstance` 是拿 `pluginSeed` 查版本号的。真实创建会把不存在的包写成「已安装」。必须改为真实注册表中存在的包，或先清空模板的插件列表。
- **`CreateProgress` 的 `environment` 步骤文案**要改，不能再说把 DSH 和 Runtime 解压进实例目录。
- **`diskUsage`** 改为调用 `instance_disk_usage`，不要再用那个写死的公式。
- **孤立目录**：在设置页「存储」里给 `scan_orphan_instances` 的结果一个入口，让用户能清掉此前遗留的插件目录。

### 权限

新增的所有命令都要在 `src-tauri/capabilities/default.json` 中声明，保持最小授权（`CLAUDE.md` 的硬性约定）。

---

## 7. 实施顺序

1. `instances.rs` 的清单读写 + `list_instances` + `sanitize`/路径守卫 → 先让「读」跑通
2. `create_instance`（含暂存重命名）+ 前端 `tauriInstances.ts` → 创建的实例重启后还在
3. `delete_instance` + `save_instance` → 生命周期闭合
4. `scan_instance_plugins`，切换插件列表来源 → 记录与磁盘对账
5. `clone_instance`（带进度与取消）
6. `instance_disk_usage` + 设置页接线 + 孤立目录清理入口
7. 修 `templateSeed` 与进度文案

前四步完成即可止血，5–7 是补齐。

---

## 8. 验证

- `npm run typecheck`、`npm run build`、`cargo test --lib`、`cargo clippy --all-targets` 全绿
- Rust 单元测试（沿用 `versions.rs` / `plugins.rs` 已有的 `#[cfg(test)]` 风格）：
  - `id` 白名单拒绝 `..`、绝对路径、分隔符
  - 创建失败不留下无清单的目录
  - `scan_instance_plugins` 能正确合并 `phl-plugin.json` 与 `cordis.patch.yml` 的启用状态
  - 删除拒绝 `<root>/instances/` 之外的路径
- 端到端手测（`npm run app:dev`）：
  1. 建实例 → 确认磁盘上出现完整目录树与 `instance.json`
  2. 装一个插件 → 关掉应用 → 重开：实例与插件都还在
  3. 改名、改端口 → 重开：修改仍在
  4. 克隆 → 两个实例的 `node_modules` 相互独立
  5. 删除 → 磁盘上整棵树消失
  6. 设置页「存储」显示的数字与资源管理器看到的实际占用一致

---

## 9. 完成后的状态

`PhlRepository` 18 个方法中，真实接入的从 8 个增加到 13 个：

```text
已真实      版本(3) + 插件(5) + 实例(5，含新增 saveInstance)
仍为 mock   listRuntimes / installRuntime / removeRuntime / listTemplates
            launch / stop
```

届时 README、设置页「关于」与首次引导浮层中「纯前端原型 / 不会真的改动磁盘」的表述**必须同步更新** —— 那时它已经与事实相差甚远（其实现在就已经不准确了）。

下一个模块建议 **Runtime Manager**：它与 Version Manager 几乎同构，可直接复用 `download` / `verify_integrity` / `extract` 管线，且是真实启动的最后一块前置依赖。
