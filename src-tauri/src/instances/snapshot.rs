//! Point-in-time copies of an instance's `dsh-home`: create under a staging
//! name, restore by copy-swap with the previous tree as backup, delete.
//!
//! A restore replaces the home's plugins and configuration and nothing else.
//! The snapshot records which DSH version and Runtime the instance referenced,
//! but the instance keeps its current binding: re-pointing a live instance at
//! a version that may since have been deleted would trade a recoverable state
//! for an unrunnable one. The UI says so instead of implying a full rollback.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use super::copy::SkipRule;
use super::copy::{copy_tree_with_progress, LinkPolicy};
use super::{
    build_record, home_of, instance_dir, instances_root, load_manifest, profile_root_of,
    reject_external_write, sanitize_segment, scan_plugins, CloneProgress, InstanceManifest,
    InstanceRecord,
};
use crate::launch::Processes;
use crate::paths::{ensure_under_root, PhlState};
use crate::versions::{now_iso, Transfers};

/* ------------------------------ snapshots ----------------------------- */

/// A recorded point-in-time copy of the instance's `dsh-home`. The workspace
/// and logs are deliberately not part of it: a snapshot exists to make the
/// *environment* (plugins, config) reproducible, not to back up user data.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    pub id: String,
    pub label: String,
    pub created_at: String,
    /// The environment at snapshot time, recorded so a snapshot can be judged
    /// against the instance it came from. A restore deliberately does NOT
    /// re-point these: it replaces the dsh-home only, and the instance keeps
    /// the DSH and Runtime it currently references (see the module doc).
    pub version_id: String,
    pub runtime_id: String,
    pub plugin_count: usize,
    /// Bytes measured when the snapshot was taken; listing never re-walks.
    pub size: u64,
}

fn snapshots_root(dir: &Path) -> PathBuf {
    dir.join("snapshots")
}

pub(crate) async fn scan_snapshots(dir: &Path) -> Vec<SnapshotFile> {
    let mut out = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(snapshots_root(dir)).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // `.phl-new-*` is the create's staging name. It carries a committed
        // `snapshot.json` *before* the promote rename, so a crash between the
        // two would otherwise surface the half-published tree as a real
        // snapshot — one whose restore/delete could never find it.
        if crate::paths::is_hidden_tree_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(raw) = tokio::fs::read_to_string(path.join("snapshot.json")).await else {
            continue; // no metadata → not a completed snapshot
        };
        let Ok(snap) = serde_json::from_str::<SnapshotFile>(&raw) else {
            continue;
        };
        out.push(snap);
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

/// Refuses snapshot ids that are not plain segments, so a frontend-supplied
/// id can never escape the instance's `snapshots/` directory.
fn snapshot_dir(dir: &Path, snapshot_id: &str) -> Result<PathBuf, String> {
    let safe = sanitize_segment(snapshot_id, "快照 id")?;
    Ok(snapshots_root(dir).join(safe))
}

pub(crate) fn ensure_not_running(processes: &Processes, id: &str) -> Result<(), String> {
    if processes.0.lock().expect("processes lock").contains_key(id) {
        return Err("实例正在运行，请先停止再进行快照操作".into());
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_instance_snapshot(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    transfer_id: String,
    id: String,
    on_progress: Channel<CloneProgress>,
) -> Result<SnapshotFile, String> {
    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "snapshot-create",
        format!("创建快照 {id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
            task.set_phase("copying");
            let r = run_snapshot_create(&flag, &processes, &state.root(), &id, &|p| {
                let _ = on_progress.send(p);
            })
            .await;
            if crate::versions::cancelled(&flag) {
                return Err("cancelled".into());
            }
            r
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

/// Undo the debris of an interrupted restore.
///
/// The swap renames the live home aside before the restored copy is placed, so
/// a crash between the two leaves the instance with *no* home and the only
/// copy under a hidden name; a crash after the swap leaves a stale backup. Both
/// are repaired here rather than refused, because the alternative is telling
/// the user their data is gone. Safe to call when there is nothing to do.
pub(crate) async fn recover_interrupted_restore(
    dir: &Path,
    manifest: &InstanceManifest,
) -> Result<(), String> {
    let current = home_of(dir, manifest);
    let backup = dir.join(".phl-old-dsh-home");
    let staging = dir.join(".phl-restore");
    if !backup.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Ok(());
    }
    if current.exists() {
        // The swap finished; only the cleanup was interrupted.
        let _ = tokio::fs::remove_dir_all(&backup).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Ok(());
    }
    tokio::fs::rename(&backup, &current).await.map_err(|e| {
        format!(
            "检测到中断的快照还原，但无法把 {} 放回 {}：{e}",
            backup.display(),
            current.display()
        )
    })?;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    eprintln!(
        "[phl] 已回滚中断的快照还原，{} 已放回原位",
        current.display()
    );
    Ok(())
}

pub(crate) async fn run_snapshot_create<F: Fn(CloneProgress) + Send + Sync>(
    flag: &Arc<AtomicBool>,
    processes: &Processes,
    root: &Path,
    id: &str,
    on_progress: &F,
) -> Result<SnapshotFile, String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    let manifest = load_manifest(&dir, &id).await?;
    ensure_not_running(processes, &id)?;
    recover_interrupted_restore(&dir, &manifest).await?;

    // Snapshotting reads the home; for an external instance that is the
    // user's own directory, and copying it *out* into the instance tree is
    // safe. The danger is only ever the restore (below).
    let dsh_home = home_of(&dir, &manifest);
    if !dsh_home.exists() {
        return Err("实例缺少 dsh-home，无法创建快照".into());
    }

    let snap_id = format!(
        "snap-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let staging = snapshots_root(&dir).join(format!(".phl-new-{snap_id}"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| e.to_string())?;

    // Same copy-with-progress shape as cloning; the snapshot lands under a
    // staging name so a cancelled copy cannot look like a real snapshot.
    let dest = staging.join("dsh-home");
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| e.to_string())?;
    let copy_result = copy_tree_with_progress(
        dsh_home.clone(),
        dest.clone(),
        Arc::clone(flag),
        SkipRule::RunStateAtRoot,
        // The home's `node_modules` holds junctions into the shared version
        // tree and into the home's own `.pnpm`; a snapshot keeps both as
        // links (see `LinkPolicy::Preserve`).
        // The staging directory is renamed to the snapshot's final name below,
        // so in-tree links must already name that final home.
        LinkPolicy::Preserve {
            source_root: dsh_home.clone(),
            dest_root: snapshots_root(&dir).join(&snap_id).join("dsh-home"),
        },
        root.to_path_buf(),
        on_progress,
    )
    .await;
    // Cancellation is reported by `copy_tree` as Err("cancelled"), so the
    // cancel branch has to come *first*: propagating the error before it left
    // the staging tree — a full copy of dsh-home, potentially gigabytes —
    // behind forever, invisible to both the snapshot list (no snapshot.json)
    // and the orphan scanner (it only inspects children of `instances/`).
    if flag.load(Ordering::SeqCst) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err("cancelled".into());
    }
    let bytes_total = match copy_result {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(e);
        }
    };

    // Counted before the struct: `manifest.version_id` is moved into the
    // literal, and `profile_root_of` borrows the manifest — ordering them in
    // one literal partial-moves before the borrow.
    let plugin_count = scan_plugins(&profile_root_of(&dir, &manifest)).await.len();
    let snapshot = SnapshotFile {
        id: snap_id,
        label: format!("快照 {}", now_iso().replace('T', " ").trim_end_matches('Z')),
        created_at: now_iso(),
        version_id: manifest.version_id,
        runtime_id: manifest.runtime_id,
        plugin_count,
        size: bytes_total,
    };
    let body = serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?;
    tokio::fs::write(staging.join("snapshot.json"), body)
        .await
        .map_err(|e| format!("无法写入快照元数据: {e}"))?;

    let dest = snapshots_root(&dir).join(&snapshot.id);
    tokio::fs::rename(&staging, &dest)
        .await
        .map_err(|e| format!("无法放置快照目录: {e}"))?;
    Ok(snapshot)
}

/// Restores a snapshot by copying its `dsh-home` back over the live one. The
/// copy (not move) keeps the snapshot intact so it can be restored again —
/// and rolling back to the same point twice must not be a trap.
#[tauri::command]
pub async fn restore_instance_snapshot(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
    snapshot_id: String,
) -> Result<InstanceRecord, String> {
    crate::resources::guarded(
        crate::resources::next_task_id("snapshot-restore"),
        "snapshot-restore",
        format!("恢复快照 {snapshot_id} → {id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move {
            let r = restore_snapshot_inner(&state.root(), &id, &snapshot_id, &processes).await;
            if r.is_ok() {
                if let Ok(dir) = instance_dir(&state.root(), &id) {
                    crate::instances::invalidate_disk_usage(&dir);
                }
            }
            r
        },
    )
    .await
}

pub(crate) async fn restore_snapshot_inner(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
) -> Result<InstanceRecord, String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    let manifest = load_manifest(&dir, &id).await?;
    ensure_not_running(processes, &id)?;
    // Restore overwrites the live home in place. For an external instance
    // that would stamp PHL's copy over the user's own directory — the one
    // operation adoption explicitly forbids.
    reject_external_write(&manifest, &id, "还原快照并覆盖")?;
    // An interrupted earlier restore is repaired before this one reasons about
    // the live home: without it, a half-swapped instance looks like one with
    // no dsh-home at all and this command would refuse to help.
    recover_interrupted_restore(&dir, &manifest).await?;

    let snap_dir = snapshot_dir(&dir, snapshot_id)?;
    let raw = tokio::fs::read_to_string(snap_dir.join("snapshot.json"))
        .await
        .map_err(|_| format!("快照不存在: {snapshot_id}"))?;
    serde_json::from_str::<SnapshotFile>(&raw).map_err(|e| format!("快照元数据解析失败: {e}"))?;
    let snap_home = snap_dir.join("dsh-home");
    if !snap_home.exists() {
        return Err(format!("快照 {snapshot_id} 缺少 dsh-home，无法还原"));
    }
    let current = home_of(&dir, &manifest);
    if !current.exists() {
        return Err("实例缺少 dsh-home，无法还原".into());
    }

    // Copy back under a staging name, then swap. If the swap fails, the
    // current tree steps back in; the snapshot itself is never touched.
    let staging = dir.join(".phl-restore");
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| e.to_string())?;
    // Same guarded copier as create: it runs on a blocking thread (the old
    // `copy_tree_sync` blocked the async runtime for the whole restore),
    // classifies links instead of following them, and reports the worker's
    // real result rather than a half-finished tree.
    let restore_flag = Arc::new(AtomicBool::new(false));
    let dest_home = staging.join("dsh-home");
    if let Err(e) = copy_tree_with_progress(
        snap_home.clone(),
        dest_home.clone(),
        Arc::clone(&restore_flag),
        SkipRule::Nothing,
        // `current` is where this tree is renamed to, so in-tree links must
        // name the live home rather than the `.phl-restore` staging path.
        LinkPolicy::Preserve {
            source_root: snap_home.clone(),
            dest_root: current.clone(),
        },
        root.to_path_buf(),
        &|_| {},
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("还原快照失败: {e}"));
    }

    let backup = dir.join(".phl-old-dsh-home");
    let _ = tokio::fs::remove_dir_all(&backup).await;
    tokio::fs::rename(&current, &backup)
        .await
        .map_err(|e| format!("无法备份当前 dsh-home: {e}"))?;
    if let Err(e) = tokio::fs::rename(staging.join("dsh-home"), &current).await {
        let _ = tokio::fs::rename(&backup, &current).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("无法放置还原内容: {e}"));
    }
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;

    let manifest = load_manifest(&dir, &id).await?;
    Ok(build_record(&dir, manifest).await)
}

#[tauri::command]
pub async fn delete_instance_snapshot(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
    snapshot_id: String,
) -> Result<(), String> {
    crate::resources::guarded(
        crate::resources::next_task_id("snapshot-delete"),
        "snapshot-delete",
        format!("删除快照 {snapshot_id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move {
            delete_snapshot_inner(&state.root(), &id, &snapshot_id, &processes).await
        },
    )
    .await
}

pub(crate) async fn delete_snapshot_inner(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
) -> Result<(), String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    ensure_not_running(processes, &id)?;
    let snap_dir = snapshot_dir(&dir, snapshot_id)?;
    ensure_under_root(&instances_root(root), &snap_dir)?;
    if snap_dir.exists() {
        tokio::fs::remove_dir_all(&snap_dir)
            .await
            .map_err(|e| format!("无法删除快照: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-snap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_processes() -> Processes {
        Processes(std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    fn manifest(id: &str) -> InstanceManifest {
        InstanceManifest {
            schema_version: 0,
            id: id.into(),
            name: "Snap".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            port: 34480,
            auto_port: true,
            profile: "web".into(),
            created_at: crate::versions::now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: std::collections::HashMap::new(),
            args: Vec::new(),
            api: None,
            management_mode: Default::default(),
            source: Default::default(),
            external_home: None,
            adopted_from: None,
        }
    }

    fn snapshot_file(id: &str) -> SnapshotFile {
        SnapshotFile {
            id: id.into(),
            label: format!("测试快照 {id}"),
            created_at: crate::versions::now_iso(),
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            plugin_count: 0,
            size: 0,
        }
    }

    fn write_snapshot_meta(dir: &Path, snap: &SnapshotFile) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("snapshot.json"),
            serde_json::to_string_pretty(snap).unwrap(),
        )
        .unwrap();
    }

    /// A plugin package carrying PHL's marker, laid out like a real
    /// `dsh-home/profiles/<profile>/node_modules/<pkg>`.
    fn write_marker_pkg(home: &Path, name: &str) {
        let pkg = home
            .join("dsh-home")
            .join("profiles")
            .join("web")
            .join("node_modules")
            .join(name);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("phl-plugin.json"),
            format!(r#"{{"pluginId":"t/{name}","version":"1.0.0","registryId":"{name}"}}"#),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn staging_dirs_never_surface_as_snapshots() {
        let root = temp_root("staging");
        let dir = root.join("inst-1");
        let snaps = dir.join("snapshots");
        std::fs::create_dir_all(&snaps).unwrap();

        // A real, promoted snapshot.
        write_snapshot_meta(&snaps.join("snap-111"), &snapshot_file("snap-111"));
        // A crash between the metadata write and the promote rename leaves a
        // `.phl-new-*` staging tree that carries a *valid* snapshot.json.
        let mut ghost = snapshot_file("snap-222");
        ghost.plugin_count = 1;
        write_snapshot_meta(&snaps.join(".phl-new-snap-222"), &ghost);
        write_marker_pkg(&snaps.join(".phl-new-snap-222"), "solo");

        let listed = scan_snapshots(&dir).await;
        assert_eq!(
            listed.len(),
            1,
            "the half-published staging tree must not be listed: {listed:?}"
        );
        assert_eq!(listed[0].id, "snap-111");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn deleting_a_snapshot_id_resolves_through_the_final_name_only() {
        let root = temp_root("delete");
        let dir = root.join("instances").join("inst-2");
        std::fs::create_dir_all(dir.join("snapshots").join(".phl-new-snap-9")).unwrap();
        std::fs::create_dir_all(dir.join("snapshots").join("snap-9")).unwrap();
        // The staging id is refused outright (sanitize_segment rule), so a
        // "delete" can never reach into a tree a running create still owns.
        let err = delete_snapshot_inner(&root, "inst-2", ".phl-new-snap-9", &fake_processes())
            .await
            .unwrap_err();
        assert!(err.contains("非法"), "{err}");
        assert!(dir.join("snapshots").join(".phl-new-snap-9").exists());
        // The real id deletes only the final directory.
        delete_snapshot_inner(&root, "inst-2", "snap-9", &fake_processes())
            .await
            .unwrap();
        assert!(!dir.join("snapshots").join("snap-9").exists());
        assert!(dir.join("snapshots").join(".phl-new-snap-9").exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
