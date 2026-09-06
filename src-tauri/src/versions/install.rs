//! The transactional version install (T-003): stage → verify → health gate →
//! backup swap → final verify → rollback, plus the installed-version listing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::State;

use super::{
    cancelled, download, extract, install_version_deps, now_iso, package_requires_deps,
    pick_npm_capable_node, read_deps_marker, verify_integrity, InstalledVersionInfo, ProgressEvent,
};
use crate::launch::Processes;
use crate::paths::{ensure_under_root, PhlState};
use std::sync::atomic::AtomicBool;

#[tauri::command]
pub async fn list_installed_versions(
    phl: State<'_, PhlState>,
) -> Result<Vec<InstalledVersionInfo>, String> {
    let dir = phl.root().join("versions");
    let mut out = Vec::new();
    // A missing directory just means nothing has been installed yet — the
    // very first launch always lands here.
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.to_string()),
    };
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let installed_at = match read_marker(&path).await {
            Some(marker) => marker.installed_at,
            None => continue, // no marker → not a completed install
        };
        let (install_health, skipped_dependencies) = read_deps_marker(&path);
        out.push(InstalledVersionInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            installed_at,
            install_health,
            skipped_dependencies,
        });
    }
    out.sort_by(|a, b| a.installed_at.cmp(&b.installed_at));
    Ok(out)
}

/* ------------------------------ install ------------------------------ */

#[derive(Deserialize)]
// The marker on disk is written as `{"installedAt": …}` — without the rename,
// deserialising a *completed install* failed and every version looked
// uninstalled after a restart.
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallMarker {
    pub(crate) installed_at: String,
    /// Written since the transactional installer; `default` keeps markers
    /// from before that era readable.
    #[serde(default)]
    version: String,
}

pub(crate) async fn read_marker(version_dir: &Path) -> Option<InstallMarker> {
    let raw = tokio::fs::read_to_string(version_dir.join("phl-install.json"))
        .await
        .ok()?;
    serde_json::from_str::<InstallMarker>(&raw).ok()
}

pub(crate) fn sanitize_version(name: &str) -> Result<String, String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
        // `.` is an allowed character, so both of these otherwise pass the
        // whitelist — and `<root>/versions/..` resolves to the data root,
        // which `remove_version_dir` would then `remove_dir_all`.
        && name != "."
        && name != "..";
    if ok {
        Ok(name.to_string())
    } else {
        Err(format!("非法的版本号: {name}"))
    }
}

/// The transactional swap both installers share: the fully staged tree
/// replaces `dest`, the previous tree waits in `backup` until the final
/// check passes, and any failure puts the previous tree back. `staging` and
/// `dest` must be on the same drive — both installers stage under the
/// parent of `dest` to guarantee that.
///
/// This is the whole transaction vocabulary in one place: stage, backup,
/// commit, verify, rollback, cleanup — deliberately not a generic framework.
pub(crate) async fn promote_staged(
    staging: &Path,
    dest: &Path,
    backup: &Path,
    final_check: &(dyn Fn(&Path) -> Result<(), String> + Send + Sync),
) -> Result<(), String> {
    let had_previous = dest.exists();
    if had_previous {
        let _ = tokio::fs::remove_dir_all(backup).await;
        if let Err(e) = tokio::fs::rename(dest, backup).await {
            let _ = tokio::fs::remove_dir_all(staging).await;
            return Err(format!("无法备份现有安装，版本未更新: {e}"));
        }
    }
    if let Err(e) = tokio::fs::rename(staging, dest).await {
        if had_previous {
            let _ = tokio::fs::rename(backup, dest).await;
        }
        let _ = tokio::fs::remove_dir_all(staging).await;
        return Err(format!("无法放置新版本: {e}"));
    }
    if let Err(e) = final_check(dest) {
        // The new tree is in place but wrong; the backup is the only copy of
        // what worked before, so it goes back before anything else happens.
        let _ = tokio::fs::remove_dir_all(dest).await;
        if had_previous {
            let _ = tokio::fs::rename(backup, dest).await;
        }
        return Err(format!("最终校验失败，已恢复原版本: {e}"));
    }
    // Success: the backup is now redundant space, not a rollback point —
    // the staging dir is gone (renamed), so nothing references it.
    let _ = tokio::fs::remove_dir_all(backup).await;
    Ok(())
}

/// One transaction-scoped directory name: `<parent>/.phl-txn/<name>.<role>-<token>`.
/// A failed or cancelled attempt can only ever leave a `.phl-txn` child
/// behind, never something that reads as an installed version or runtime.
pub(crate) fn txn_dir(parent: &Path, name: &str, role: &str, token: u128) -> PathBuf {
    parent
        .join(".phl-txn")
        .join(format!("{name}.{role}-{token}"))
}

pub(crate) fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Minimal health check a staged version must pass before it may replace a
/// good install: manifest present, DSH entrypoint present, dependencies
/// materialised when the manifest requires them, and the marker naming this
/// exact version.
pub(crate) fn check_version_health(dir: &Path, version_name: &str) -> Result<(), String> {
    if !dir.join("package.json").exists() {
        return Err("package.json 缺失".into());
    }
    if !dir.join("lib").join("bin.js").exists() {
        return Err("DSH 入口 lib/bin.js 缺失".into());
    }
    if package_requires_deps(dir) && !dir.join("node_modules").exists() {
        return Err("依赖目录 node_modules 缺失".into());
    }
    let raw = std::fs::read_to_string(dir.join("phl-install.json"))
        .map_err(|e| format!("安装标记读取失败: {e}"))?;
    let marker: InstallMarker =
        serde_json::from_str(&raw).map_err(|e| format!("安装标记解析失败: {e}"))?;
    if marker.version != version_name {
        let found = if marker.version.is_empty() {
            "<空>".to_string()
        } else {
            marker.version
        };
        return Err(format!("安装标记版本 {found} 与目标 {version_name} 不一致"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_install(
    flag: &Arc<AtomicBool>,
    tarball_url: &str,
    integrity: Option<&str>,
    version_name: &str,
    root: &Path,
    registry_base: &str,
    keep_archive: bool,
    total_hint: Option<u64>,
    on_progress: &Channel<ProgressEvent>,
) -> Result<(), String> {
    let version_name = sanitize_version(version_name)?;
    if cancelled(flag) {
        return Err("cancelled".into());
    }

    let versions_dir = root.join("versions");
    let cache_dir = root.join("cache");
    let dest = versions_dir.join(&version_name);
    let archive_path = cache_dir.join(format!("dsh-{version_name}.tgz"));
    let part_path = cache_dir.join(format!("dsh-{version_name}.tgz.part"));
    let token = now_millis();
    let staging = txn_dir(&versions_dir, &version_name, "staging", token);
    let backup = txn_dir(&versions_dir, &version_name, "backup", token);
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(staging.parent().expect("txn parent"))
        .await
        .map_err(|e| e.to_string())?;

    // The existing install is never touched until a verified replacement is
    // fully staged: a download, integrity, extraction or dependency failure
    // must leave the old version exactly where it was.
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let downloaded = download(
        flag,
        tarball_url,
        &part_path,
        total_hint,
        &|progress, bytes_done, bytes_per_sec| {
            let _ = on_progress.send(ProgressEvent::Downloading {
                progress,
                bytes_done,
                bytes_per_sec,
            });
        },
    )
    .await?;

    on_progress
        .send(ProgressEvent::Verifying)
        .map_err(|e| e.to_string())?;
    if let Err(e) = verify_integrity(&downloaded.sha512, integrity) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    on_progress
        .send(ProgressEvent::Extracting { progress: 0.0 })
        .map_err(|e| e.to_string())?;
    if let Err(e) = extract(
        &part_path,
        &staging,
        &|progress| {
            let _ = on_progress.send(ProgressEvent::Extracting { progress });
        },
        flag,
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    // The tarball is only the package itself — pull its declared deps into
    // the staging tree's own node_modules now, while the user is watching an
    // install. A version that boots into a surprise 30-second npm run would
    // be the worse failure mode.
    if package_requires_deps(&staging) {
        on_progress
            .send(ProgressEvent::InstallingDeps { progress: 0.0 })
            .map_err(|e| e.to_string())?;
        let node = pick_npm_capable_node(root).unwrap_or_else(|| PathBuf::from("node"));
        if let Err(e) = install_version_deps(&node, &staging, registry_base, flag, &|p| {
            let _ = on_progress.send(ProgressEvent::InstallingDeps { progress: p });
        })
        .await
        {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(if e == "cancelled" {
                e
            } else {
                format!("已下载但依赖安装失败，版本未安装: {e}")
            });
        }
    }

    // The marker is written last into staging: its presence is what makes a
    // directory read as a completed install anywhere on disk.
    let marker = serde_json::json!({
        "installedAt": now_iso(),
        "version": version_name,
        "tarball": tarball_url,
        "integrity": integrity.unwrap_or(""),
        "bytes": downloaded.bytes,
    });
    if let Err(e) = tokio::fs::write(staging.join("phl-install.json"), marker.to_string()).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e.to_string());
    }

    on_progress
        .send(ProgressEvent::Verifying)
        .map_err(|e| e.to_string())?;
    if let Err(e) = check_version_health(&staging, &version_name) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("安装校验未通过，版本未安装: {e}"));
    }

    promote_staged(&staging, &dest, &backup, &|installed: &Path| {
        check_version_health(installed, &version_name)
    })
    .await?;

    if keep_archive {
        tokio::fs::rename(&part_path, &archive_path)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        let _ = tokio::fs::remove_file(&part_path).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_version_dir(
    processes: State<'_, Processes>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    version_name: String,
) -> Result<(), String> {
    crate::resources::guarded(
        crate::resources::next_task_id("version-remove"),
        "version-remove",
        format!("删除版本 {version_name}"),
        vec![crate::resources::Resource::Version(version_name.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move {
            remove_version_dir_inner(&phl.root(), &processes, &version_name).await
        },
    )
    .await
}

pub(crate) async fn remove_version_dir_inner(
    root: &Path,
    processes: &Processes,
    version_name: &str,
) -> Result<(), String> {
    let safe = sanitize_version(version_name)?;
    let dir = root.join("versions").join(&safe);
    // Canonical containment: a junction planted at the version path must not
    // redirect `remove_dir_all` outside the data root.
    ensure_under_root(&root.join("versions"), &dir)?;

    // A running instance executes node.exe from inside this version's tree —
    // Windows locks running executables and their working directories, so the
    // delete would fail with an opaque "access denied". Refuse up front and
    // name the instances, instead of letting the user guess from os error 5.
    let version_id = format!("dsh-{safe}");
    let mut running_users: Vec<String> = Vec::new();
    let pinned: Vec<String> = processes
        .0
        .lock()
        .expect("processes lock")
        .keys()
        .cloned()
        .collect();
    for instance_id in pinned {
        let Ok(instance_dir_path) = crate::instances::instance_dir(root, &instance_id) else {
            continue;
        };
        if let Ok(manifest) =
            crate::instances::load_manifest(&instance_dir_path, &instance_id).await
        {
            if manifest.version_id == version_id {
                running_users.push(format!("「{}」", manifest.name));
            }
        }
    }
    if !running_users.is_empty() {
        return Err(format!(
            "以下实例正在运行此版本，请先停止后再删除：{}",
            running_users.join("、")
        ));
    }

    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await.map_err(|e| match e.raw_os_error() {
            // 5 = access denied, 32 = sharing violation — both mean Windows
            // found a handle this process cannot break. Name the usual
            // suspects; a bare os error teaches the user nothing.
            Some(5) | Some(32) => format!(
                "目录被占用，无法删除（{e}）。请检查：① 是否有实例正在运行此版本（含残留的 node 进程）；② 资源管理器或终端是否停在该目录内；③ 杀毒软件是否正在扫描。"
            ),
            _ => e.to_string(),
        })?;
    }
    Ok(())
}
