//! Point-in-time copies of an instance's `dsh-home`: create under a staging
//! name, restore by copy-swap with the previous tree as backup, delete.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use super::copy::SkipRule;
use super::copy::{copy_tree_sync, copy_tree_with_progress};
use super::{
    build_record, instance_dir, instances_root, load_manifest, profile_root, sanitize_segment,
    scan_plugins, CloneProgress, InstanceRecord,
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
    /// The environment at snapshot time — a restore brings these back.
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
pub async fn create_instance_snapshot(
    transfers: State<'_, Transfers>,
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    transfer_id: String,
    id: String,
    on_progress: Channel<CloneProgress>,
) -> Result<SnapshotFile, String> {
    let flag = transfers.take(&transfer_id);
    let result = run_snapshot_create(&flag, &processes, &state.root(), &id, &|p| {
        let _ = on_progress.send(p);
    })
    .await;
    transfers.release(&transfer_id);
    result
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

    let dsh_home = dir.join("dsh-home");
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
        dest,
        Arc::clone(flag),
        SkipRule::RunStateAtRoot,
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

    let snapshot = SnapshotFile {
        id: snap_id,
        label: format!("快照 {}", now_iso().replace('T', " ").trim_end_matches('Z')),
        created_at: now_iso(),
        version_id: manifest.version_id,
        runtime_id: manifest.runtime_id,
        plugin_count: scan_plugins(&profile_root(&dir, &manifest.profile))
            .await
            .len(),
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
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
    snapshot_id: String,
) -> Result<InstanceRecord, String> {
    restore_snapshot_inner(&state.root(), &id, &snapshot_id, &processes).await
}

pub(crate) async fn restore_snapshot_inner(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
) -> Result<InstanceRecord, String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    ensure_not_running(processes, &id)?;

    let snap_dir = snapshot_dir(&dir, snapshot_id)?;
    let raw = tokio::fs::read_to_string(snap_dir.join("snapshot.json"))
        .await
        .map_err(|_| format!("快照不存在: {snapshot_id}"))?;
    serde_json::from_str::<SnapshotFile>(&raw).map_err(|e| format!("快照元数据解析失败: {e}"))?;
    let snap_home = snap_dir.join("dsh-home");
    if !snap_home.exists() {
        return Err(format!("快照 {snapshot_id} 缺少 dsh-home，无法还原"));
    }
    let current = dir.join("dsh-home");
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
    if let Err(e) = copy_tree_sync(&snap_home, &staging.join("dsh-home")) {
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
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
    snapshot_id: String,
) -> Result<(), String> {
    delete_snapshot_inner(&state.root(), &id, &snapshot_id, &processes).await
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
