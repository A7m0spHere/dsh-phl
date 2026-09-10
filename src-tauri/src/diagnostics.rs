//! Diagnostics: one command that answers "is my PHL install healthy?".
//!
//! Checks are cheap filesystem probes — existence, writability, marker
//! presence. Nothing here walks a `node_modules` tree (the cache summary only
//! counts top-level entries and sizes) and nothing touches the network, so a
//! full report is safe to run on demand from the settings page.

use std::path::Path;

use serde::Serialize;

use tauri::State;

use crate::instances::{self};
use crate::paths::PhlState;
use crate::versions::now_iso;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticItem {
    pub id: String,
    /// `ok` | `warn` | `fail` — mirrors the frontend tone set.
    pub level: String,
    pub label: String,
    /// Human-readable detail; empty when there is nothing to add.
    pub detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub root: String,
    pub items: Vec<DiagnosticItem>,
    /// Download-cache summary, offered for cleanup on the same page.
    pub cache_bytes: u64,
    pub cache_files: usize,
    pub generated_at: String,
}

#[tauri::command]
pub async fn run_diagnostics(phl: State<'_, PhlState>) -> Result<DiagnosticReport, String> {
    run_diagnostics_inner(&phl.root()).await
}

/// The plain report, callable from tests with a temp root.
async fn run_diagnostics_inner(root_path: &Path) -> Result<DiagnosticReport, String> {
    let mut items = Vec::new();

    // 1. 数据目录存在且可写 — 没有这一条，其它一切都会以诡异的方式失败。
    items.push(writability_probe(root_path).await);

    // 2. 已安装 DSH 版本的完整性：清单标记 + lib/bin.js。
    items.push(versions_check(root_path).await);

    // 3. 已安装 Runtime 的完整性：清单标记 + node 可执行文件。
    items.push(runtimes_check(root_path).await);

    // 4. 实例引用完整性：实例指向的 version / runtime 是否真的在磁盘上。
    items.push(instance_refs_check(root_path).await);

    // 5. 孤立目录：无清单的 instances/* 目录，存储页提供回收入口。
    let orphans = instances::scan_orphan_instances_inner(root_path)
        .await
        .unwrap_or_default();
    items.push(DiagnosticItem {
        id: "orphan-dirs".into(),
        level: if orphans.is_empty() { "ok" } else { "warn" }.into(),
        label: "孤立实例目录".into(),
        detail: if orphans.is_empty() {
            String::new()
        } else {
            let names: Vec<&str> = orphans.iter().map(|o| o.name.as_str()).collect();
            format!(
                "{} 个（{}），可在「存储」分区回收",
                orphans.len(),
                names.join("、")
            )
        },
    });

    // 6. 安装残留：中断的安装事务、旧版暂存目录、下载分片。只报告，不删除 ——
    //    清理入口在各分区（版本/运行时的删除、缓存清理），这里让用户知道它们存在。
    let residue = crate::repair::scan_residue_inner(root_path)
        .await
        .unwrap_or_default();
    let stale: Vec<_> = residue.iter().filter(|r| r.stale).collect();
    items.push(DiagnosticItem {
        id: "residue".into(),
        level: if residue.is_empty() { "ok" } else { "warn" }.into(),
        label: "安装残留".into(),
        detail: if residue.is_empty() {
            String::new()
        } else {
            format!(
                "{} 处（其中 {} 处已过期）：{}",
                residue.len(),
                stale.len(),
                residue
                    .iter()
                    .take(3)
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            )
        },
    });

    // 7. 下载缓存：可安全清理的 .part 残留与保留的压缩包。
    let (cache_bytes, cache_files) = cache_summary(root_path).await;
    items.push(DiagnosticItem {
        id: "cache".into(),
        level: if cache_bytes > 0 { "warn" } else { "ok" }.into(),
        label: "下载缓存".into(),
        detail: if cache_bytes > 0 {
            format!("占用 {} 字节，可在本页清理", cache_bytes)
        } else {
            String::new()
        },
    });

    Ok(DiagnosticReport {
        root: root_path.to_string_lossy().into_owned(),
        items,
        cache_bytes,
        cache_files,
        generated_at: now_iso(),
    })
}

/// Removes everything under `<root>/cache` — leftover `.part` files and any
/// archives the user chose to keep. Both are safe to delete; they only ever
/// speed up a reinstall. Returns the bytes freed. Registered as a task
/// (audit C): the cache holds multi-GB `.part` downloads and archives, and
/// this `remove_dir_all` used to be the one user-sized sweep invisible to
/// the task centre.
#[tauri::command]
pub async fn clear_download_cache(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
) -> Result<u64, String> {
    crate::resources::guarded(
        crate::resources::next_task_id("cache-clear"),
        "cache-clear",
        "清理下载缓存".to_string(),
        Vec::<crate::resources::Resource>::new(),
        None,
        &locks,
        &tasks,
        move |task| async move {
            task.set_phase("removing");
            clear_download_cache_inner(&phl.root()).await
        },
    )
    .await
}

async fn clear_download_cache_inner(root: &Path) -> Result<u64, String> {
    let cache = root.join("cache");
    let (bytes, _) = cache_summary(root).await;
    if cache.exists() {
        tokio::fs::remove_dir_all(&cache)
            .await
            .map_err(|e| format!("无法清理缓存目录: {e}"))?;
    }
    Ok(bytes)
}

async fn writability_probe(root: &Path) -> DiagnosticItem {
    let probe = root.join(".phl-probe");
    let fail = |detail: String| DiagnosticItem {
        id: "root-writable".into(),
        level: "fail".into(),
        label: "数据目录可写".into(),
        detail,
    };
    if tokio::fs::create_dir_all(root).await.is_err() {
        return fail(format!("目录不存在且无法创建：{}", root.display()));
    }
    if tokio::fs::write(&probe, b"probe").await.is_err() {
        return fail(format!("无法写入 {}", root.display()));
    }
    let _ = tokio::fs::remove_file(&probe).await;
    DiagnosticItem {
        id: "root-writable".into(),
        level: "ok".into(),
        label: "数据目录可写".into(),
        detail: root.display().to_string(),
    }
}

async fn versions_check(root: &Path) -> DiagnosticItem {
    let dir = root.join("versions");
    let mut total = 0usize;
    let mut broken: Vec<String> = Vec::new();
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return DiagnosticItem {
                id: "versions".into(),
                level: "ok".into(),
                label: "DSH 版本".into(),
                detail: "尚未安装任何版本".into(),
            };
        }
        Err(e) => {
            return DiagnosticItem {
                id: "versions".into(),
                level: "fail".into(),
                label: "DSH 版本".into(),
                detail: format!("无法读取版本目录: {e}"),
            };
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() || path.file_name().is_none() {
            continue;
        }
        // Same hidden-name rule as the runtimes check below: `.phl-txn`
        // (and any other staging or OS junk) is not an installed version,
        // and counting it flagged a broken install that does not exist.
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::paths::is_hidden_tree_name(&name) {
            continue;
        }
        total += 1;
        if !path.join("phl-install.json").exists() {
            broken.push(format!("{name}（缺少安装标记）"));
        } else if !path.join("lib").join("bin.js").exists() {
            broken.push(format!("{name}（缺少 lib/bin.js，安装不完整）"));
        }
    }
    DiagnosticItem {
        id: "versions".into(),
        level: if broken.is_empty() { "ok" } else { "warn" }.into(),
        label: "DSH 版本".into(),
        detail: if broken.is_empty() {
            format!("{total} 个已安装，全部完整")
        } else {
            format!("发现问题：{}", broken.join("；"))
        },
    }
}

async fn runtimes_check(root: &Path) -> DiagnosticItem {
    let dir = root.join("runtimes");
    let node_binary = if cfg!(windows) {
        "node.exe"
    } else {
        "bin/node"
    };
    let mut total = 0usize;
    let mut broken: Vec<String> = Vec::new();
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return DiagnosticItem {
                id: "runtimes".into(),
                level: "ok".into(),
                label: "Node Runtime".into(),
                detail: "尚未安装任何 Runtime".into(),
            };
        }
        Err(e) => {
            return DiagnosticItem {
                id: "runtimes".into(),
                level: "fail".into(),
                label: "Node Runtime".into(),
                detail: format!("无法读取 Runtime 目录: {e}"),
            };
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // `.phl-*` 是安装暂存目录，不是已完成的 Runtime（规则见 `paths::is_hidden_tree_name`）。
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::paths::is_hidden_tree_name(&name) {
            continue;
        }
        total += 1;
        if !path.join("phl-runtime.json").exists() {
            broken.push(format!("{name}（缺少安装标记）"));
        } else if !path.join(node_binary).exists() {
            broken.push(format!("{name}（缺少 node 可执行文件）"));
        }
    }
    DiagnosticItem {
        id: "runtimes".into(),
        level: if broken.is_empty() { "ok" } else { "warn" }.into(),
        label: "Node Runtime".into(),
        detail: if broken.is_empty() {
            format!("{total} 个已安装，全部完整")
        } else {
            format!("发现问题：{}", broken.join("；"))
        },
    }
}

/// An instance whose pinned version or runtime has been uninstalled shows up
/// here — the launch guard would refuse it, this check explains it up front.
async fn instance_refs_check(root: &Path) -> DiagnosticItem {
    let records = match instances::list_instances_inner(root).await {
        Ok(records) => records,
        Err(e) => {
            return DiagnosticItem {
                id: "instance-refs".into(),
                level: "fail".into(),
                label: "实例引用".into(),
                detail: format!("无法读取实例列表: {e}"),
            };
        }
    };
    let mut dangling: Vec<String> = Vec::new();
    for record in &records {
        // Version ids are `dsh-<semver>` but install directories are named by
        // the bare version — the same strip the version service and the
        // launcher both apply. Joining the raw id made every healthy instance
        // report its version as missing.
        let version_name = record
            .manifest
            .version_id
            .strip_prefix("dsh-")
            .unwrap_or(&record.manifest.version_id);
        let version_dir = root.join("versions").join(version_name);
        if !version_dir.exists() {
            dangling.push(format!(
                "{}（版本 {} 未安装）",
                record.manifest.name, record.manifest.version_id
            ));
        }
        let runtime_ok = record.manifest.runtime_id == "node-system"
            || root
                .join("runtimes")
                .join(&record.manifest.runtime_id)
                .exists();
        if !runtime_ok {
            dangling.push(format!(
                "{}（Runtime {} 未安装）",
                record.manifest.name, record.manifest.runtime_id
            ));
        }
    }
    DiagnosticItem {
        id: "instance-refs".into(),
        level: if dangling.is_empty() { "ok" } else { "warn" }.into(),
        label: "实例引用".into(),
        detail: if dangling.is_empty() {
            format!("{} 个实例的版本与 Runtime 引用全部有效", records.len())
        } else {
            format!("引用缺失：{}", dangling.join("；"))
        },
    }
}

/// Top-level summary only: file count + bytes of the cache dir. Deliberately
/// not recursive — the cache holds archives, not extracted trees.
async fn cache_summary(root: &Path) -> (u64, usize) {
    let dir = root.join("cache");
    let mut bytes = 0u64;
    let mut files = 0usize;
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return (0, 0);
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-diag-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item<'a>(report: &'a DiagnosticReport, id: &str) -> &'a DiagnosticItem {
        report.items.iter().find(|i| i.id == id).unwrap()
    }

    #[tokio::test]
    async fn healthy_install_reports_all_ok() {
        let root = temp_root("healthy");
        // 一个完整的版本 + 一个完整的 Runtime + 一个无引用的实例目录结构。
        let version = root.join("versions/0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();
        std::fs::write(version.join("lib/bin.js"), "// bin").unwrap();
        let runtime = root.join("runtimes/node-22");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("phl-runtime.json"), "{}").unwrap();
        let node = runtime.join(if cfg!(windows) {
            "node.exe"
        } else {
            "bin/node"
        });
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "bin").unwrap();

        let report = run_diagnostics_inner(&root).await.unwrap();
        assert_eq!(item(&report, "root-writable").level, "ok");
        assert_eq!(item(&report, "versions").level, "ok");
        assert_eq!(item(&report, "runtimes").level, "ok");
        assert_eq!(item(&report, "instance-refs").level, "ok");
        assert_eq!(item(&report, "orphan-dirs").level, "ok");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn broken_installs_and_dangling_refs_are_reported() {
        let root = temp_root("broken");
        // 版本目录有标记但没有 bin.js —— 安装不完整。
        let version = root.join("versions/0.1.0");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();

        let report = run_diagnostics_inner(&root).await.unwrap();
        assert_eq!(item(&report, "versions").level, "warn");
        assert!(item(&report, "versions").detail.contains("lib/bin.js"));

        // 指向未安装版本的实例 —— 引用缺失。
        let instances = root.join("instances/inst-0001");
        std::fs::create_dir_all(instances.join("dsh-home/profiles/web")).unwrap();
        let manifest = r#"{"id":"inst-0001","name":"Demo","kind":"sandbox","hue":0,
            "versionId":"0.1.0","runtimeId":"node-22","port":3080,"autoPort":true,
            "profile":"web","createdAt":"2026-01-01T00:00:00Z"}"#;
        std::fs::write(instances.join("instance.json"), manifest.replace('\n', "")).unwrap();

        let report = run_diagnostics_inner(&root).await.unwrap();
        assert_eq!(item(&report, "instance-refs").level, "warn");
        assert!(item(&report, "instance-refs").detail.contains("Demo"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn transaction_staging_is_never_counted_as_installed() {
        let root = temp_root("staging");
        // A real, complete version.
        let version = root.join("versions/0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();
        std::fs::write(version.join("lib/bin.js"), "// bin").unwrap();
        // `.phl-txn` still holding staging trees from a running or crashed
        // install, and a detached removal still carrying the *old* marker —
        // neither may count as an installed version.
        std::fs::create_dir_all(root.join("versions/.phl-txn/0.1.5.staging-1")).unwrap();
        let stale = root.join("versions/.phl-REMOVE-0.1.2");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("phl-install.json"), "{}").unwrap();
        // An empty hidden parent in runtimes must not look broken either.
        std::fs::create_dir_all(root.join("runtimes/.phl-txn")).unwrap();

        let report = run_diagnostics_inner(&root).await.unwrap();
        assert_eq!(item(&report, "versions").level, "ok");
        assert!(
            item(&report, "versions").detail.contains("1 个已安装"),
            "only the real version counts: {}",
            item(&report, "versions").detail
        );
        assert_eq!(item(&report, "runtimes").level, "ok");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cache_summary_counts_and_clear_frees() {
        let root = temp_root("cache");
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("dsh-0.1.0.tgz.part"), vec![0u8; 128]).unwrap();
        std::fs::write(cache.join("dsh-0.1.0.tgz"), vec![0u8; 256]).unwrap();

        let report = run_diagnostics_inner(&root).await.unwrap();
        assert_eq!(report.cache_files, 2);
        assert_eq!(report.cache_bytes, 384);
        assert_eq!(item(&report, "cache").level, "warn");

        let freed = clear_download_cache_inner(&root).await.unwrap();
        assert_eq!(freed, 384);
        assert!(!cache.exists(), "cache dir removed wholesale");

        let _ = std::fs::remove_dir_all(&root);
    }
}
