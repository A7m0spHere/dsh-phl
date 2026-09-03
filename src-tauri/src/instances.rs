//! Real Instance Manager backing: an instance is a directory on disk, not a
//! record in memory.
//!
//! Layout follows `dsh-phl-development-roadmap.md` §7:
//!
//! ```text
//! <root>/instances/<id>/
//! ├── instance.json
//! ├── dsh-home/profiles/<profile>/   ← node_modules + cordis.patch.yml
//! ├── workspace/
//! └── logs/
//! ```
//!
//! DSH versions and Node runtimes are deliberately *not* copied in here: they
//! live once under `<root>/versions` and `<root>/runtimes` and an instance
//! only references them by id. Sharing the bits while isolating the state is
//! the whole point of the product.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::api_config::ApiBinding;
use crate::launch::Processes;
use crate::plugins::disabled_plugin_ids;
use crate::versions::{now_iso, Transfers};

/* ----------------------------- wire types ----------------------------- */

/// What `instance.json` holds: everything that defines the instance and
/// nothing that can be observed from the filesystem. The plugin list is
/// absent on purpose — see `scan_plugins`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: Option<String>,
    pub kind: String,
    pub hue: u32,
    pub version_id: String,
    pub runtime_id: String,
    pub port: u32,
    pub auto_port: bool,
    pub profile: String,
    pub created_at: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub total_runtime: u64,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Binding to the global API provider library (`api_config`). `None` for
    /// manifests written before the feature existed — treated as unmanaged.
    #[serde(default)]
    pub api: Option<ApiBinding>,
}

/// A manifest plus everything derived from the directory it lives in.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRecord {
    #[serde(flatten)]
    pub manifest: InstanceManifest,
    pub dsh_home: String,
    pub workspace: String,
    pub plugins: Vec<InstalledPluginInfo>,
    pub snapshots: Vec<SnapshotFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginInfo {
    pub plugin_id: String,
    pub version: String,
    pub enabled: bool,
    /// The id used in `cordis.patch.yml`. Handed back so the frontend can
    /// enable or uninstall without consulting the (possibly offline) catalog.
    pub registry_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanDir {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneProgress {
    pub progress: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// What the marker `install_plugin` leaves inside each package directory.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginMarker {
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    version: String,
    #[serde(default)]
    registry_id: String,
}

/* ------------------------------- paths -------------------------------- */

/// Instance ids and profile names are pasted straight into a filesystem path,
/// so they get the same whitelist treatment as version names: anything that
/// is not a plain segment is rejected rather than normalised.
fn sanitize_segment(value: &str, label: &str) -> Result<String, String> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && value != "."
        && value != "..";
    if ok {
        Ok(value.to_string())
    } else {
        Err(format!("非法的{label}: {value}"))
    }
}

fn instances_root(root: &Path) -> PathBuf {
    root.join("instances")
}

pub(crate) fn instance_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(instances_root(root).join(sanitize_segment(id, "实例 id")?))
}

fn profile_root(dir: &Path, profile: &str) -> PathBuf {
    dir.join("dsh-home").join("profiles").join(profile)
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("instance.json")
}

/// Refuses any path that is not a direct child of `<root>/instances`.
///
/// `sanitize_segment` already rejects separators, so this is belt and braces
/// — but the operation on the other side is `remove_dir_all`, and a guard
/// that only exists in one place is one refactor away from being gone.
fn assert_inside_instances(root: &Path, dir: &Path) -> Result<(), String> {
    let base = instances_root(root);
    let is_child = dir.parent() == Some(base.as_path())
        && matches!(dir.components().next_back(), Some(Component::Normal(_)));
    if is_child {
        Ok(())
    } else {
        Err("拒绝操作 instances 目录之外的路径".into())
    }
}

/* ------------------------------ commands ------------------------------ */

#[tauri::command]
pub async fn list_instances(root: String) -> Result<Vec<InstanceRecord>, String> {
    let dir = instances_root(Path::new(&root));
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        // Nothing has been created yet — the very first launch lands here.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };

    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // A directory without a manifest is not an instance: it is either a
        // staging dir from an interrupted create, or a leftover from before
        // instances were real. `scan_orphan_instances` reports those.
        let Some(manifest) = read_manifest(&path).await else {
            continue;
        };
        out.push(build_record(&path, manifest).await);
    }
    out.sort_by(|a, b| a.manifest.created_at.cmp(&b.manifest.created_at));
    Ok(out)
}

#[tauri::command]
pub async fn create_instance(
    root: String,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    let root = Path::new(&root);
    let dir = build_instance_tree(root, &manifest).await?;
    let manifest = apply_api_at_create(root, &dir, manifest).await;
    Ok(build_record(&dir, manifest).await)
}

/// Materialize the API binding right after creation so a new instance boots
/// configured instead of empty — the whole point of the global library. A
/// failed sync does not fail the create: the binding stays (minus sync
/// metadata), the instance shows as "尚未同步" in the UI, and the user can
/// fix the library and hit sync.
async fn apply_api_at_create(
    root: &Path,
    dir: &Path,
    mut manifest: InstanceManifest,
) -> InstanceManifest {
    let Some(binding) = manifest.api.clone() else { return manifest };
    match crate::api_config::apply_create_binding(root, dir, &manifest.id, binding).await {
        Some(applied) => manifest.api = Some(applied),
        None => eprintln!("[phl] api sync at create skipped for {}", manifest.id),
    }
    manifest
}

#[tauri::command]
pub async fn save_instance(root: String, manifest: InstanceManifest) -> Result<(), String> {
    let dir = instance_dir(Path::new(&root), &manifest.id)?;
    if !dir.exists() {
        return Err(format!("实例不存在: {}", manifest.id));
    }
    write_manifest(&dir, &manifest).await
}

#[tauri::command]
pub async fn delete_instance(
    processes: State<'_, Processes>,
    root: String,
    id: String,
) -> Result<(), String> {
    delete_instance_inner(Path::new(&root), &id, &processes).await
}

async fn delete_instance_inner(
    root: &Path,
    id: &str,
    processes: &Processes,
) -> Result<(), String> {
    let dir = instance_dir(root, id)?;
    assert_inside_instances(root, &dir)?;
    // The UI blocks this too, but the guard belongs next to the irreversible
    // operation: a running instance's files are in use, and forcing the
    // delete would leave a half-deleted tree behind a live process.
    ensure_not_running(processes, id)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| format!("无法删除实例目录: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn clone_instance(
    transfers: State<'_, Transfers>,
    transfer_id: String,
    root: String,
    source_id: String,
    manifest: InstanceManifest,
    on_progress: Channel<CloneProgress>,
) -> Result<InstanceRecord, String> {
    let flag = transfers.take(&transfer_id);
    let result = run_clone(&flag, Path::new(&root), &source_id, manifest, &on_progress).await;
    transfers.release(&transfer_id);
    result
}

#[tauri::command]
pub async fn instance_disk_usage(root: String, id: String) -> Result<u64, String> {
    let dir = instance_dir(Path::new(&root), &id)?;
    tokio::task::spawn_blocking(move || dir_size(&dir))
        .await
        .map_err(|e| e.to_string())
}

/// Directories under `<root>/instances` that carry no manifest — interrupted
/// creates, and plugin trees written before instances were real. Surfacing
/// them gives the user a way to reclaim the space.
#[tauri::command]
pub async fn scan_orphan_instances(root: String) -> Result<Vec<OrphanDir>, String> {
    let dir = instances_root(Path::new(&root));
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if !path.is_dir() || manifest_path(&path).exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let probe = path.clone();
        let size = tokio::task::spawn_blocking(move || dir_size(&probe))
            .await
            .unwrap_or(0);
        out.push(OrphanDir { name, size });
    }
    Ok(out)
}

#[tauri::command]
pub async fn delete_orphan_instance(root: String, name: String) -> Result<(), String> {
    let root = Path::new(&root);
    let dir = instances_root(root).join(sanitize_segment(&name, "目录名")?);
    assert_inside_instances(root, &dir)?;
    // Refuse anything that turns out to be a real instance after all.
    if manifest_path(&dir).exists() {
        return Err("该目录是一个有效实例，请从实例页删除".into());
    }
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/* ------------------------------- bundle ------------------------------- */

/// The bundle format: a manifest plus the plugin records that were on disk at
/// export time. Deliberately not a package of `node_modules` — the manifest
/// is what makes an environment reproducible, and the plugin files come back
/// through the normal install pipeline, not a private archive.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceBundle {
    /// Format tag; anything other than 1 is refused on import.
    pub phl_bundle: u32,
    pub exported_at: String,
    pub instance: InstanceManifest,
    pub plugins: Vec<BundlePluginEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundlePluginEntry {
    pub plugin_id: String,
    pub version: String,
    pub registry_id: String,
}

/// What the import dialog shows before anything is created.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundlePreview {
    pub name: String,
    pub version_id: String,
    pub runtime_id: String,
    pub port: u32,
    pub plugin_count: usize,
    pub exported_at: String,
}

async fn read_bundle_file(path: &str) -> Result<InstanceBundle, String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("无法读取 Bundle 文件: {e}"))?;
    // The format tag is checked before the full parse, so an unknown version
    // reports itself instead of a pile of missing-field errors.
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("Bundle 文件解析失败: {e}"))?;
    let version = value.get("phlBundle").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != 1 {
        return Err(format!("不支持的 Bundle 版本: {version}"));
    }
    let bundle: InstanceBundle =
        serde_json::from_value(value).map_err(|e| format!("Bundle 文件解析失败: {e}"))?;
    Ok(bundle)
}

#[tauri::command]
pub async fn export_instance_bundle(root: String, id: String, dest: String) -> Result<(), String> {
    let root = Path::new(&root);
    let dir = instance_dir(root, &id)?;
    let manifest = read_manifest(&dir)
        .await
        .ok_or_else(|| format!("实例不存在或缺少清单: {id}"))?;
    let plugins = scan_plugins(&profile_root(&dir, &manifest.profile)).await;
    let bundle = InstanceBundle {
        phl_bundle: 1,
        exported_at: now_iso(),
        plugins: plugins
            .into_iter()
            .map(|p| BundlePluginEntry {
                plugin_id: p.plugin_id,
                version: p.version,
                registry_id: p.registry_id,
            })
            .collect(),
        instance: manifest,
    };
    let body = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    tokio::fs::write(&dest, body)
        .await
        .map_err(|e| format!("无法写入 Bundle: {e}"))
}

#[tauri::command]
pub async fn read_instance_bundle(path: String) -> Result<BundlePreview, String> {
    let bundle = read_bundle_file(&path).await?;
    Ok(BundlePreview {
        name: bundle.instance.name,
        version_id: bundle.instance.version_id,
        runtime_id: bundle.instance.runtime_id,
        port: bundle.instance.port,
        plugin_count: bundle.plugins.len(),
        exported_at: bundle.exported_at,
    })
}

/// Creates an instance from a bundle file. Identity (id, name, port) comes
/// from the importer so collisions stay a frontend concern; everything that
/// defines the *environment* — version, runtime, profile, env, args — comes
/// from the bundle. Plugin files are not in a bundle by design: the records
/// travel, the reinstall goes through the normal plugin pipeline.
#[tauri::command]
pub async fn import_instance_bundle(
    root: String,
    path: String,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    let bundle = read_bundle_file(&path).await?;

    let mut manifest = manifest;
    manifest.kind = bundle.instance.kind;
    manifest.hue = bundle.instance.hue;
    manifest.version_id = bundle.instance.version_id;
    manifest.runtime_id = bundle.instance.runtime_id;
    manifest.profile = bundle.instance.profile;
    manifest.note = Some("从 Bundle 导入".into());
    manifest.env = bundle.instance.env;
    manifest.args = bundle.instance.args;

    let root = Path::new(&root);
    let dir = build_instance_tree(root, &manifest).await?;
    // Same "boots configured" promise as a normal create: the imported
    // instance inherits the global library unless the manifest says otherwise.
    let manifest = apply_api_at_create(root, &dir, manifest).await;
    Ok(build_record(&dir, manifest).await)
}

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

async fn scan_snapshots(dir: &Path) -> Vec<SnapshotFile> {
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

fn ensure_not_running(processes: &Processes, id: &str) -> Result<(), String> {
    if processes.0.lock().expect("processes lock").contains_key(id) {
        return Err("实例正在运行，请先停止再进行快照操作".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn create_instance_snapshot(
    transfers: State<'_, Transfers>,
    processes: State<'_, Processes>,
    transfer_id: String,
    root: String,
    id: String,
    on_progress: Channel<CloneProgress>,
) -> Result<SnapshotFile, String> {
    let flag = transfers.take(&transfer_id);
    let result = run_snapshot_create(
        &flag,
        &processes,
        Path::new(&root),
        &id,
        &|p| {
            let _ = on_progress.send(p);
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

async fn run_snapshot_create<F: Fn(CloneProgress) + Send + Sync>(
    flag: &Arc<AtomicBool>,
    processes: &Processes,
    root: &Path,
    id: &str,
    on_progress: &F,
) -> Result<SnapshotFile, String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    let manifest = read_manifest(&dir)
        .await
        .ok_or_else(|| format!("实例不存在或缺少清单: {id}"))?;
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
    tokio::fs::create_dir_all(&staging).await.map_err(|e| e.to_string())?;

    // Same copy-with-progress shape as cloning; the snapshot lands under a
    // staging name so a cancelled copy cannot look like a real snapshot.
    let dest = staging.join("dsh-home");
    tokio::fs::create_dir_all(&dest).await.map_err(|e| e.to_string())?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
    let from = dsh_home.clone();
    let to = dest.clone();
    let worker_flag = Arc::clone(flag);
    let worker = std::thread::spawn(move || {
        let total = dir_size_skipping(&from);
        let mut done = 0u64;
        let result = copy_tree(&from, &to, &worker_flag, &mut done, total, &tx);
        let _ = tx.blocking_send(result.map(|()| (total, total)));
    });

    let mut copy_result = Ok(());
    let mut bytes_total = 0u64;
    while let Some(message) = rx.recv().await {
        match message {
            Ok((bytes_done, total)) => {
                bytes_total = total;
                let progress = if total > 0 {
                    (bytes_done as f64 / total as f64).min(1.0)
                } else {
                    1.0
                };
                on_progress(CloneProgress { progress, bytes_done, bytes_total: total });
            }
            Err(e) => {
                copy_result = Err(e);
                break;
            }
        }
    }
    let _ = worker.join();
    copy_result?;
    if flag.load(Ordering::SeqCst) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err("cancelled".into());
    }

    let snapshot = SnapshotFile {
        id: snap_id,
        label: format!("快照 {}", now_iso().replace('T', " ").trim_end_matches('Z')),
        created_at: now_iso(),
        version_id: manifest.version_id,
        runtime_id: manifest.runtime_id,
        plugin_count: scan_plugins(&profile_root(&dir, &manifest.profile)).await.len(),
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
    root: String,
    id: String,
    snapshot_id: String,
) -> Result<InstanceRecord, String> {
    restore_snapshot_inner(Path::new(&root), &id, &snapshot_id, &processes).await
}

async fn restore_snapshot_inner(
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
    tokio::fs::create_dir_all(&staging).await.map_err(|e| e.to_string())?;
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

    let manifest = read_manifest(&dir)
        .await
        .ok_or_else(|| format!("实例缺少清单: {id}"))?;
    Ok(build_record(&dir, manifest).await)
}

#[tauri::command]
pub async fn delete_instance_snapshot(
    processes: State<'_, Processes>,
    root: String,
    id: String,
    snapshot_id: String,
) -> Result<(), String> {
    delete_snapshot_inner(Path::new(&root), &id, &snapshot_id, &processes).await
}

async fn delete_snapshot_inner(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
) -> Result<(), String> {
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    ensure_not_running(processes, &id)?;
    let snap_dir = snapshot_dir(&dir, snapshot_id)?;
    if snap_dir.exists() {
        tokio::fs::remove_dir_all(&snap_dir)
            .await
            .map_err(|e| format!("无法删除快照: {e}"))?;
    }
    Ok(())
}

/// Blocking tree copy for restore; the amounts involved make a progress
/// channel not worth the wiring, and the swap after it is atomic.
fn copy_tree_sync(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_dir() {
            copy_tree_sync(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("复制 {} 失败: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/* ------------------------------ internals ----------------------------- */

/// The shared staging + rename create path (used by create and bundle
/// import). A create that fails halfway must not leave a directory that looks
/// like an instance but has no manifest — that is exactly how the current
/// orphans came about.
async fn build_instance_tree(root: &Path, manifest: &InstanceManifest) -> Result<PathBuf, String> {
    let id = sanitize_segment(&manifest.id, "实例 id")?;
    let profile = sanitize_segment(&manifest.profile, "profile 名")?;
    let dir = instance_dir(root, &id)?;
    if dir.exists() {
        return Err(format!("实例目录已存在: {id}"));
    }

    let staging = instances_root(root).join(format!(".phl-new-{id}"));
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let build = async {
        tokio::fs::create_dir_all(profile_root(&staging, &profile).join("node_modules"))
            .await
            .map_err(|e| format!("无法创建实例目录: {e}"))?;
        tokio::fs::create_dir_all(staging.join("workspace"))
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(staging.join("logs"))
            .await
            .map_err(|e| e.to_string())?;
        write_manifest(&staging, manifest).await
    }
    .await;

    if let Err(e) = build {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    tokio::fs::rename(&staging, &dir)
        .await
        .map_err(|e| format!("无法放置实例目录: {e}"))?;
    Ok(dir)
}

pub(crate) async fn read_manifest(dir: &Path) -> Option<InstanceManifest> {
    let raw = tokio::fs::read_to_string(manifest_path(dir)).await.ok()?;
    serde_json::from_str::<InstanceManifest>(&raw).ok()
}

/// Persist just the API binding on an existing manifest — the sync path must
/// update `syncedAt`/`syncedHash` without a full manifest round-trip from the
/// frontend (which could race a concurrent rename on the instance).
pub(crate) async fn set_instance_api(
    root: &Path,
    id: &str,
    binding: &ApiBinding,
) -> Result<(), String> {
    let dir = instance_dir(root, id)?;
    let mut manifest = read_manifest(&dir)
        .await
        .ok_or_else(|| format!("实例不存在或缺少清单: {id}"))?;
    manifest.api = Some(binding.clone());
    write_manifest(&dir, &manifest).await
}

async fn write_manifest(dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    let body = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // truncated manifest — that would make the instance unreadable entirely.
    let tmp = dir.join("instance.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))?;
    tokio::fs::rename(&tmp, manifest_path(dir))
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))
}

async fn build_record(dir: &Path, manifest: InstanceManifest) -> InstanceRecord {
    let profile = profile_root(dir, &manifest.profile);
    InstanceRecord {
        dsh_home: dir.join("dsh-home").to_string_lossy().into_owned(),
        workspace: dir.join("workspace").to_string_lossy().into_owned(),
        plugins: scan_plugins(&profile).await,
        snapshots: scan_snapshots(dir).await,
        manifest,
    }
}

/// Reads the installed plugins back off disk.
///
/// The instance manifest deliberately does not store this list. `node_modules`
/// plus `cordis.patch.yml` are what DSH itself reads, so treating them as the
/// source of truth removes a whole class of drift: an install whose record was
/// lost still shows up, a record whose files were removed does not, and a
/// plugin added out of band is discovered rather than ignored.
async fn scan_plugins(profile: &Path) -> Vec<InstalledPluginInfo> {
    let node_modules = profile.join("node_modules");
    let disabled = disabled_plugin_ids(profile).await;
    let mut out = Vec::new();
    collect_packages(&node_modules, &disabled, &mut out, true).await;
    out.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
    out
}

/// npm scopes are a directory level (`@scope/pkg`), so the walk descends once
/// into `@…` entries and no further.
async fn collect_packages(
    dir: &Path,
    disabled: &HashSet<String>,
    out: &mut Vec<InstalledPluginInfo>,
    allow_scopes: bool,
) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // `.phl-tmp-*` / `.phl-old-*` are the installer's staging dirs.
        if name.starts_with('.') {
            continue;
        }
        if allow_scopes && name.starts_with('@') {
            Box::pin(collect_packages(&path, disabled, out, false)).await;
            continue;
        }
        let Ok(raw) = tokio::fs::read_to_string(path.join("phl-plugin.json")).await else {
            continue; // not installed by PHL
        };
        let Ok(marker) = serde_json::from_str::<PluginMarker>(&raw) else {
            continue;
        };
        // Markers written before the catalog id was recorded fall back to the
        // registry id — a slightly wrong label beats losing the install.
        let plugin_id = marker
            .plugin_id
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| marker.registry_id.clone());
        if plugin_id.is_empty() {
            continue;
        }
        out.push(InstalledPluginInfo {
            enabled: !disabled.contains(&marker.registry_id),
            registry_id: marker.registry_id,
            plugin_id,
            version: marker.version,
        });
    }
}

async fn run_clone(
    flag: &Arc<AtomicBool>,
    root: &Path,
    source_id: &str,
    manifest: InstanceManifest,
    on_progress: &Channel<CloneProgress>,
) -> Result<InstanceRecord, String> {
    let source = instance_dir(root, source_id)?;
    if !manifest_path(&source).exists() {
        return Err(format!("源实例不存在: {source_id}"));
    }
    let id = sanitize_segment(&manifest.id, "实例 id")?;
    let dest = instance_dir(root, &id)?;
    if dest.exists() {
        return Err(format!("实例目录已存在: {id}"));
    }

    let staging = instances_root(root).join(format!(".phl-new-{id}"));
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let from = source.clone();
    let to = staging.clone();
    let worker_flag = Arc::clone(flag);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
    std::thread::spawn(move || {
        let total = dir_size_skipping(&from);
        let mut done = 0u64;
        let result = copy_tree(&from, &to, &worker_flag, &mut done, total, &tx);
        let _ = tx.blocking_send(result.map(|()| (total, total)));
    });

    let mut copy_result = Ok(());
    while let Some(message) = rx.recv().await {
        match message {
            Ok((bytes_done, bytes_total)) => {
                let progress = if bytes_total > 0 {
                    (bytes_done as f64 / bytes_total as f64).min(1.0)
                } else {
                    1.0
                };
                let _ = on_progress.send(CloneProgress {
                    progress,
                    bytes_done,
                    bytes_total,
                });
            }
            Err(e) => {
                copy_result = Err(e);
                break;
            }
        }
        if flag.load(Ordering::SeqCst) {
            copy_result = Err("cancelled".into());
            break;
        }
    }

    if let Err(e) = copy_result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    // The clone is a new instance, not a copy of the old one's history.
    let mut manifest = manifest;
    manifest.created_at = now_iso();
    manifest.last_run_at = None;
    manifest.total_runtime = 0;
    manifest.favorite = false;
    if let Err(e) = write_manifest(&staging, &manifest).await {
        // Without this the fully copied tree — potentially gigabytes — would
        // be left behind, invisible to the instance list and reclaimable only
        // through the orphan scanner.
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    tokio::fs::rename(&staging, &dest)
        .await
        .map_err(|e| format!("无法放置实例目录: {e}"))?;
    Ok(build_record(&dest, manifest).await)
}

/// Logs, download caches and snapshot history are per-instance run state;
/// copying them would inflate the clone and carry the source's history into a
/// fresh instance. A clone starts from the present, without the past.
fn skipped(name: &str) -> bool {
    name == "logs" || name == "snapshots" || name == ".phl-cache" || name.starts_with(".phl-")
}

pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

pub(crate) fn dir_size_skipping(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    flag: &AtomicBool,
    done: &mut u64,
    total: u64,
    tx: &tokio::sync::mpsc::Sender<Result<(u64, u64), String>>,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            copy_tree(&src, &dst, flag, done, total, tx)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("复制失败 {name}: {e}"))?;
            *done += meta.len();
            // A closed channel means the caller gave up; stop rather than keep
            // writing into a directory it is already deleting.
            if tx.blocking_send(Ok((*done, total))).is_err() {
                return Err("cancelled".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-inst-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str, name: &str) -> InstanceManifest {
        InstanceManifest {
            id: id.into(),
            name: name.into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-22".into(),
            port: 8080,
            auto_port: true,
            profile: "default".into(),
            created_at: now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: HashMap::new(),
            args: Vec::new(),
            api: None,
        }
    }

    #[test]
    fn segments_reject_path_tricks() {
        assert!(sanitize_segment("plugin-dev-a1b2", "id").is_ok());
        assert!(sanitize_segment("default", "profile").is_ok());
        assert!(sanitize_segment("..", "id").is_err());
        assert!(sanitize_segment("a/b", "id").is_err());
        assert!(sanitize_segment("a\\b", "id").is_err());
        assert!(sanitize_segment("", "id").is_err());
    }

    #[test]
    fn deletion_is_confined_to_the_instances_dir() {
        let root = Path::new("phl");
        assert!(assert_inside_instances(root, &root.join("instances").join("foo")).is_ok());
        assert!(assert_inside_instances(root, &root.join("versions").join("foo")).is_err());
        assert!(assert_inside_instances(root, &root.join("instances")).is_err());
        assert!(
            assert_inside_instances(root, &root.join("instances").join("a").join("b")).is_err()
        );
    }

    #[test]
    fn clone_skips_run_state() {
        assert!(skipped("logs"));
        assert!(skipped(".phl-cache"));
        assert!(skipped(".phl-tmp-foo"));
        assert!(!skipped("dsh-home"));
        assert!(!skipped("workspace"));
    }

    #[tokio::test]
    async fn instance_lifecycle_on_disk() {
        let root = temp_root("life");
        let root_s = root.to_string_lossy().into_owned();

        // Nothing created yet: an absent instances/ dir is not an error.
        assert!(list_instances(root_s.clone()).await.unwrap().is_empty());

        let record = create_instance(root_s.clone(), manifest("demo-a1b2", "Demo"))
            .await
            .unwrap();
        let dir = root.join("instances").join("demo-a1b2");
        assert!(manifest_path(&dir).exists(), "instance.json written");
        assert!(dir.join("workspace").exists());
        assert!(dir.join("logs").exists());
        assert!(dir
            .join("dsh-home")
            .join("profiles")
            .join("default")
            .join("node_modules")
            .exists());
        assert_eq!(record.dsh_home, dir.join("dsh-home").to_string_lossy());
        assert!(record.plugins.is_empty());

        // Creating the same id twice must not clobber the first one.
        assert!(create_instance(root_s.clone(), manifest("demo-a1b2", "Demo"))
            .await
            .is_err());

        let listed = list_instances(root_s.clone()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.name, "Demo");

        // Rename survives a round trip through disk.
        let mut renamed = manifest("demo-a1b2", "Renamed");
        renamed.port = 9001;
        save_instance(root_s.clone(), renamed).await.unwrap();
        let listed = list_instances(root_s.clone()).await.unwrap();
        assert_eq!(listed[0].manifest.name, "Renamed");
        assert_eq!(listed[0].manifest.port, 9001);

        delete_instance_inner(root.as_path(), "demo-a1b2", &Processes::default())
            .await
            .unwrap();
        assert!(!dir.exists());
        assert!(list_instances(root_s).await.unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn plugins_are_read_back_from_disk() {
        let root = temp_root("scan");
        let root_s = root.to_string_lossy().into_owned();
        create_instance(root_s.clone(), manifest("scan-0001", "Scan"))
            .await
            .unwrap();

        let profile = root
            .join("instances")
            .join("scan-0001")
            .join("dsh-home")
            .join("profiles")
            .join("default");
        let modules = profile.join("node_modules");

        // A scoped package, an unscoped one, and the installer's staging dir.
        std::fs::create_dir_all(modules.join("@acme").join("widget")).unwrap();
        std::fs::write(
            modules.join("@acme").join("widget").join("phl-plugin.json"),
            r#"{"pluginId":"acme/widget","version":"1.2.0","registryId":"@acme/widget"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(modules.join("solo")).unwrap();
        std::fs::write(
            modules.join("solo").join("phl-plugin.json"),
            r#"{"pluginId":"who/solo","version":"0.3.1","registryId":"solo"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(modules.join(".phl-tmp-junk")).unwrap();
        // A package PHL did not install carries no marker and must be ignored.
        std::fs::create_dir_all(modules.join("stranger")).unwrap();

        // `solo` is disabled in the profile's patch file.
        std::fs::write(
            profile.join("cordis.patch.yml"),
            "- id: solo\n  name: solo\n  disabled: true\n\n- id: @acme/widget\n  name: widget\n",
        )
        .unwrap();

        let listed = list_instances(root_s).await.unwrap();
        let plugins = &listed[0].plugins;
        assert_eq!(plugins.len(), 2, "staging dirs and unmarked packages skipped");

        let widget = plugins.iter().find(|p| p.plugin_id == "acme/widget").unwrap();
        assert_eq!(widget.version, "1.2.0");
        assert!(widget.enabled);
        // The registry id must survive the round trip: enable/uninstall use it
        // instead of re-deriving one from the (possibly offline) catalog.
        assert_eq!(widget.registry_id, "@acme/widget");

        let solo = plugins.iter().find(|p| p.plugin_id == "who/solo").unwrap();
        assert!(!solo.enabled, "cordis.patch.yml is the enabled-state source");
        assert_eq!(solo.registry_id, "solo");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn orphan_dirs_are_reported_and_reclaimable() {
        let root = temp_root("orphan");
        let root_s = root.to_string_lossy().into_owned();
        create_instance(root_s.clone(), manifest("real-0001", "Real"))
            .await
            .unwrap();

        // A tree with no manifest — what an interrupted create leaves behind.
        let orphan = root.join("instances").join("leftover");
        std::fs::create_dir_all(orphan.join("dsh-home")).unwrap();
        std::fs::write(orphan.join("dsh-home").join("blob"), vec![0u8; 2048]).unwrap();

        let found = scan_orphan_instances(root_s.clone()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "leftover");
        assert_eq!(found[0].size, 2048);

        // A real instance must never be removable through this door.
        assert!(delete_orphan_instance(root_s.clone(), "real-0001".into())
            .await
            .is_err());

        delete_orphan_instance(root_s.clone(), "leftover".into())
            .await
            .unwrap();
        assert!(!orphan.exists());
        assert!(scan_orphan_instances(root_s).await.unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn copying_a_tree_skips_run_state_and_reports_bytes() {
        let root = temp_root("copy");
        let from = root.join("src");
        std::fs::create_dir_all(from.join("dsh-home")).unwrap();
        std::fs::create_dir_all(from.join("logs")).unwrap();
        std::fs::write(from.join("dsh-home").join("a"), vec![7u8; 512]).unwrap();
        std::fs::write(from.join("logs").join("noisy"), vec![7u8; 4096]).unwrap();

        assert_eq!(dir_size_skipping(&from), 512, "logs excluded from the total");

        let to = root.join("dst");
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        // `copy_tree` uses `blocking_send`, so it has to run off the runtime
        // thread — exactly as `run_clone` does in production.
        let src = from.clone();
        let dst = to.clone();
        let worker = std::thread::spawn(move || {
            let flag = AtomicBool::new(false);
            let mut done = 0u64;
            copy_tree(&src, &dst, &flag, &mut done, 512, &tx).map(|()| done)
        });

        let mut events = 0;
        while rx.recv().await.is_some() {
            events += 1;
        }
        let done = worker.join().unwrap().unwrap();

        assert!(to.join("dsh-home").join("a").exists());
        assert!(!to.join("logs").exists(), "run state not carried into a clone");
        assert_eq!(done, 512);
        assert!(events > 0, "progress was reported");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bundle_roundtrip_export_preview_import() {
        let root = temp_root("bundle");
        let root_s = root.to_string_lossy().into_owned();
        let source = create_instance(root_s.clone(), manifest("src-a1b2", "Source"))
            .await
            .unwrap();

        // An installed plugin carries its marker; the bundle must record it.
        let pkg = root
            .join("instances/src-a1b2/dsh-home/profiles")
            .join(&source.manifest.profile)
            .join("node_modules/@scope/widget");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("phl-plugin.json"),
            r#"{"pluginId":"@scope/widget","version":"1.2.3","registryId":"@scope/widget"}"#,
        )
        .unwrap();

        let dest = root.join("out/source.phl-bundle.json");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        export_instance_bundle(
            root_s.clone(),
            "src-a1b2".into(),
            dest.to_string_lossy().into_owned(),
        )
        .await
        .unwrap();

        let preview = read_instance_bundle(dest.to_string_lossy().into_owned()).await.unwrap();
        assert_eq!(preview.name, "Source");
        assert_eq!(preview.plugin_count, 1);

        // Identity comes from the importer; environment fields come from the
        // bundle — the placeholder version id here must NOT survive.
        let mut importer = manifest("dst-c3d4", "Restored");
        importer.version_id = "placeholder".into();
        importer.port = 9999;
        let imported = import_instance_bundle(
            root_s.clone(),
            dest.to_string_lossy().into_owned(),
            importer,
        )
        .await
        .unwrap();

        assert_eq!(imported.manifest.id, "dst-c3d4");
        assert_eq!(
            imported.manifest.version_id, source.manifest.version_id,
            "version comes from the bundle, not the importer"
        );
        assert_eq!(imported.manifest.port, 9999, "port comes from the importer");
        assert_eq!(imported.manifest.note.as_deref(), Some("从 Bundle 导入"));
        assert!(
            manifest_path(&root.join("instances").join("dst-c3d4")).exists(),
            "imported instance is a real directory with a manifest"
        );
        assert!(
            imported.plugins.is_empty(),
            "a bundle carries records, not files — the plugin list is disk-derived"
        );

        // Identity collision is refused before anything is touched.
        let err = import_instance_bundle(
            root_s.clone(),
            dest.to_string_lossy().into_owned(),
            manifest("dst-c3d4", "Again"),
        )
        .await
        .unwrap_err();
        assert!(err.contains("已存在"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bundle_import_refuses_unknown_format_version() {
        let root = temp_root("bundlever");
        let path = root.join("bad.phl-bundle.json");
        std::fs::write(&path, r#"{"phlBundle":99,"exportedAt":"","instance":{},"plugins":[]}"#)
            .unwrap();
        let err =
            read_instance_bundle(path.to_string_lossy().into_owned()).await.unwrap_err();
        assert!(err.contains("不支持的 Bundle 版本"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn snapshot_create_restore_delete_roundtrip() {
        let root = temp_root("snap");
        let root_s = root.to_string_lossy().into_owned();
        let record = create_instance(root_s.clone(), manifest("snap-a1b2", "Snapped"))
            .await
            .unwrap();
        let dir = root.join("instances").join("snap-a1b2");
        let pkg = dir
            .join("dsh-home/profiles")
            .join(&record.manifest.profile)
            .join("node_modules/pkgone");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("phl-plugin.json"),
            r#"{"pluginId":"pkgone","version":"1.0.0","registryId":"pkgone"}"#,
        )
        .unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let processes = Processes::default();
        let snap = {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<CloneProgress>(16);
            let drainer = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let snap = run_snapshot_create(
                &flag,
                &processes,
                root.as_path(),
                "snap-a1b2",
                &|p| {
                    let _ = tx.try_send(p);
                },
            )
            .await
            .unwrap();
            drop(tx);
            let _ = drainer.await;
            snap
        };

        assert!(snap.id.starts_with("snap-"));
        assert_eq!(snap.plugin_count, 1);
        assert!(snap.size > 0, "snapshot size is the measured copy");
        assert!(
            dir.join("snapshots").join(&snap.id).join("dsh-home").exists(),
            "snapshot tree exists under snapshots/"
        );
        // Listed from the record, newest first.
        let listed = list_instances(root_s.clone()).await.unwrap();
        assert_eq!(listed[0].snapshots.len(), 1);
        assert_eq!(listed[0].snapshots[0].id, snap.id);

        // Mutate the live tree, then restore: the plugin disappears and the
        // recorded tree comes back — and the snapshot survives the restore.
        std::fs::remove_dir_all(&pkg).unwrap();
        let restored = restore_snapshot_inner(root.as_path(), "snap-a1b2", &snap.id, &processes)
            .await
            .unwrap();
        assert!(pkg.exists(), "restore brings the recorded plugin tree back");
        assert_eq!(restored.plugins.len(), 1);
        assert_eq!(scan_snapshots(&dir).await.len(), 1, "restore keeps the snapshot");

        delete_snapshot_inner(root.as_path(), "snap-a1b2", &snap.id, &processes)
            .await
            .unwrap();
        assert_eq!(scan_snapshots(&dir).await.len(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }
}
