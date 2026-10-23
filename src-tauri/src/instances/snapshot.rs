//! Point-in-time copies of an instance's `dsh-home`: create under a staging
//! name, restore by copy-swap with the previous tree as backup, delete.
//!
//! A restore replaces the home's plugins and configuration. Conversation
//! data — the live home's `sessions/` and `attachments/` — is kept: the
//! staged copy leaves the snapshot's own copies out (`SkipRule::Restore`)
//! and copies of the live directories join it before the swap. The snapshot itself
//! still holds the full home, so its sessions stay reachable by hand for
//! disaster recovery. The snapshot records which DSH version and Runtime
//! the instance referenced, but the instance keeps its current binding:
//! re-pointing a live instance at a version that may since have been deleted
//! would trade a recoverable state for an unrunnable one. The UI says so
//! instead of implying a full rollback.

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

mod restore;

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

/// Whether any snapshot of this instance records `runtime_id` (CR-09):
/// snapshots remember the runtime they were taken under, so a restore
/// after a runtime deletion must not strand on a phantom reference.
pub(crate) async fn snapshots_reference_runtime(dir: &Path, runtime_id: &str) -> bool {
    scan_snapshots(dir)
        .await
        .iter()
        .any(|snap| snap.runtime_id == runtime_id)
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

/// Recover only under the instance lock, before startup listing or launch
/// can observe a half-swapped home. Ambiguous state is preserved and reported.
pub(crate) async fn recover_interrupted_restore(
    dir: &Path,
    manifest: &InstanceManifest,
) -> Result<(), String> {
    restore::recover(dir, manifest).await
}

pub(crate) async fn has_pending_restore(dir: &Path) -> Result<bool, String> {
    restore::pending(dir).await
}

pub(crate) async fn recover_pending_restores(
    root: &Path,
    processes: &Processes,
    registry: &crate::launch::Registry,
    locks: &crate::resources::ResourceLocks,
) -> Result<(), String> {
    let mut entries = match tokio::fs::read_dir(instances_root(root)).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::paths::is_hidden_tree_name(&name)
            || !entry.file_type().await.map_err(|e| e.to_string())?.is_dir()
        {
            continue;
        }
        let Ok(id) = sanitize_segment(&name, "实例 id") else {
            continue; // Unrelated/orphan directories are not restore targets.
        };
        let Ok(_held) = locks.acquire(&[crate::resources::Resource::Instance(id.clone())]) else {
            continue; // An active restore owns its staging directory.
        };
        if !has_pending_restore(&entry.path()).await? {
            continue;
        }
        if crate::launch::ensure_launch_available(
            processes,
            registry,
            &id,
            crate::launch::probe_process,
        )
        .is_err()
        {
            continue; // Keep a running/uncertain process and its home untouched.
        }
        let manifest = load_manifest(&entry.path(), &id).await?;
        recover_interrupted_restore(&entry.path(), &manifest).await?;
    }
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
    let _restore_lock = restore::lock(&dir).await?;
    restore::recover_locked(&dir, &manifest).await?;

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
///
/// Shares the create operation's model: a transfer id for the cancel flag,
/// a progress Channel for the copy, and a registered guarded task (phase
/// `copying` + ratio) so the task center and the instance page tell the same
/// story. The swap is deliberately NOT cancellable — once the live home is
/// renamed aside, the only safe way out is to finish putting the restored
/// copy in place.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn restore_instance_snapshot(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, Processes>,
    registry: State<'_, crate::launch::Registry>,
    state: State<'_, PhlState>,
    transfer_id: String,
    id: String,
    snapshot_id: String,
    on_progress: Channel<CloneProgress>,
) -> Result<InstanceRecord, String> {
    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "snapshot-restore",
        format!("恢复快照 {snapshot_id} → {id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        move |task| async move {
            crate::launch::ensure_launch_available(
                &processes,
                &registry,
                &id,
                crate::launch::probe_process,
            )?;
            task.set_phase("copying");
            let r = restore_snapshot_with(
                &state.root(),
                &id,
                &snapshot_id,
                &processes,
                &flag,
                &|p: CloneProgress| {
                    task.set_progress(Some(p.progress));
                    let _ = on_progress.send(p);
                },
            )
            .await;
            if r.is_ok() {
                if let Ok(dir) = instance_dir(&state.root(), &id) {
                    crate::instances::invalidate_disk_usage(&dir);
                }
            }
            r
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

/// Test-only wrapper: no cancel channel, progress discarded. Production
/// goes through the command above, which wires the real flag and channel.
#[cfg(test)]
pub(crate) async fn restore_snapshot_inner(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
) -> Result<InstanceRecord, String> {
    restore_snapshot_with(
        root,
        id,
        snapshot_id,
        processes,
        &Arc::new(AtomicBool::new(false)),
        &|_| {},
    )
    .await
}

pub(crate) async fn restore_snapshot_with<F: Fn(CloneProgress) + Send + Sync>(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
    flag: &Arc<AtomicBool>,
    on_progress: &F,
) -> Result<InstanceRecord, String> {
    restore::run(root, id, snapshot_id, processes, flag, on_progress, &|_| {
        Ok(())
    })
    .await
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
    let _restore_lock = restore::lock(&dir).await?;
    if has_pending_restore(&dir).await? {
        return Err("实例有未完成的快照还原，请先完成恢复；暂不删除快照以保留恢复资料".into());
    }
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

    #[tokio::test]
    async fn a_cancelled_restore_never_touches_the_live_home() {
        let root = temp_root("restore-cancel");
        let id = "inst-r1";
        let dir = root.join("instances").join(id);
        std::fs::create_dir_all(dir.join("dsh-home").join("profiles").join("web")).unwrap();
        crate::instances::manifest::write_manifest(&dir, &manifest(id))
            .await
            .unwrap();
        write_snapshot_meta(
            &dir.join("snapshots").join("snap-1"),
            &snapshot_file("snap-1"),
        );
        std::fs::create_dir_all(dir.join("snapshots").join("snap-1").join("dsh-home")).unwrap();

        // The transfer is already cancelled before the call: the copier's
        // first act is checking the flag, so the restore must abort before
        // the swap and leave the live home exactly as it was.
        let flag = Arc::new(AtomicBool::new(true));
        let err = restore_snapshot_with(&root, id, "snap-1", &fake_processes(), &flag, &|_| {})
            .await
            .unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
        assert!(dir.join("dsh-home").join("profiles").join("web").exists());
        assert!(!dir.join(".phl-restore").exists(), "staging cleaned up");
        assert!(
            !dir.join(".phl-old-dsh-home").exists(),
            "the swap never started"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The 2026-09-29 review's P1, pinned: a restore replaces plugins and
    /// configuration, NOT conversation data. The live home's sessions and
    /// attachments survive — including sessions created *after* the snapshot
    /// — the snapshot's own copies never surface, and everything else rolls
    /// back to the snapshot's content.
    #[tokio::test]
    async fn a_restore_keeps_the_live_conversation_data() {
        let root = temp_root("restore-sessions");
        let id = "inst-s1";
        let dir = root.join("instances").join(id);
        let home = dir.join("dsh-home");
        // Live home at snapshot time: one session, one attachment, one config.
        std::fs::create_dir_all(home.join("sessions").join("proj").join("s-old")).unwrap();
        std::fs::write(
            home.join("sessions")
                .join("proj")
                .join("s-old")
                .join("meta.json"),
            "{}",
        )
        .unwrap();
        std::fs::create_dir_all(home.join("attachments")).unwrap();
        std::fs::write(home.join("attachments").join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(home.join("profiles").join("web")).unwrap();
        std::fs::write(
            home.join("profiles").join("web").join("config.json"),
            "old-config",
        )
        .unwrap();
        crate::instances::manifest::write_manifest(&dir, &manifest(id))
            .await
            .unwrap();

        let snap = run_snapshot_create(
            &Arc::new(AtomicBool::new(false)),
            &fake_processes(),
            &root,
            id,
            &|_| {},
        )
        .await
        .unwrap();

        // After the snapshot the conversation moves on: the old session and
        // attachment are deleted, new ones appear, and the config drifts.
        // Under the pre-fix behavior the restore resurrected `s-old` and
        // `old.txt` out of the snapshot and deleted the newer data.
        std::fs::remove_dir_all(home.join("sessions").join("proj").join("s-old")).unwrap();
        std::fs::create_dir_all(home.join("sessions").join("proj").join("s-new")).unwrap();
        std::fs::write(
            home.join("sessions")
                .join("proj")
                .join("s-new")
                .join("meta.json"),
            "{}",
        )
        .unwrap();
        std::fs::remove_file(home.join("attachments").join("old.txt")).unwrap();
        std::fs::write(home.join("attachments").join("new.txt"), "new").unwrap();
        std::fs::write(
            home.join("profiles").join("web").join("config.json"),
            "new-config",
        )
        .unwrap();

        restore_snapshot_inner(&root, id, &snap.id, &fake_processes())
            .await
            .unwrap();

        // Conversation data kept — including what the snapshot predates.
        assert!(
            home.join("sessions")
                .join("proj")
                .join("s-new")
                .join("meta.json")
                .exists(),
            "sessions newer than the snapshot must survive the restore"
        );
        assert!(
            !home.join("sessions").join("proj").join("s-old").exists(),
            "the snapshot's own session copy must not surface"
        );
        assert!(home.join("attachments").join("new.txt").exists());
        assert!(!home.join("attachments").join("old.txt").exists());
        // Everything else rolls back to the snapshot.
        assert_eq!(
            std::fs::read_to_string(home.join("profiles").join("web").join("config.json")).unwrap(),
            "old-config"
        );
        // No staging leftovers.
        assert!(!dir.join(".phl-restore").exists());
        assert!(!dir.join(".phl-old-dsh-home").exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
