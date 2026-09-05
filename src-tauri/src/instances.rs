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
use crate::paths::{ensure_under_root, sanitize_segment, PhlState};
use crate::plugins::disabled_plugin_ids;
use crate::versions::{now_iso, Transfers};

/* ----------------------------- wire types ----------------------------- */

/// What `instance.json` holds: everything that defines the instance and
/// nothing that can be observed from the filesystem. The plugin list is
/// absent on purpose — see `scan_plugins`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceManifest {
    /// On-disk format version, stamped by the backend on every write. Absent
    /// means the manifest predates schema versioning — same shape, migrated
    /// in memory on read and stamped on the next write (original kept as a
    /// `.bak`). A version from a *newer* PHL is never guessed at; see
    /// `classify_manifest`.
    #[serde(default)]
    pub schema_version: u32,
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

/// The manifest format this build reads and writes. Bump only together with a
/// migration story: older versions must keep parsing, and this build must
/// refuse (not guess at) anything written by a newer one.
pub(crate) const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// What reading an `instance.json` actually found. The distinctions matter:
/// `Missing` means "not an instance at all", `Corrupt` is reclaimable junk,
/// and `UnsupportedSchema` is a *valid instance this build cannot parse* —
/// it must stay invisible to every destructive path.
enum ManifestRead {
    Ok(Box<InstanceManifest>),
    Missing,
    Corrupt(String),
    UnsupportedSchema { found: u32, supported: u32 },
}

async fn classify_manifest(dir: &Path) -> ManifestRead {
    let Ok(raw) = tokio::fs::read_to_string(manifest_path(dir)).await else {
        return ManifestRead::Missing;
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(e) => return ManifestRead::Corrupt(format!("JSON 无法解析: {e}")),
    };
    // The version is inspected on the raw JSON, before a full parse: a
    // manifest from a newer PHL may carry fields this struct would reject, and
    // that failure must report itself as "too new", not as corruption.
    let found = value
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if found > MANIFEST_SCHEMA_VERSION {
        return ManifestRead::UnsupportedSchema {
            found,
            supported: MANIFEST_SCHEMA_VERSION,
        };
    }
    match serde_json::from_value::<InstanceManifest>(value) {
        Ok(mut manifest) => {
            // Legacy manifests migrate in memory; the stamp reaches disk the
            // next time this manifest is written, with the original saved.
            manifest.schema_version = MANIFEST_SCHEMA_VERSION;
            ManifestRead::Ok(Box::new(manifest))
        }
        Err(e) => ManifestRead::Corrupt(e.to_string()),
    }
}

// Instance ids and profile names are pasted straight into a filesystem path —
// the whitelist lives in `paths::sanitize_segment`, shared with every module
// that builds paths from user-supplied ids.

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
pub async fn list_instances(state: State<'_, PhlState>) -> Result<Vec<InstanceRecord>, String> {
    list_instances_inner(&state.root()).await
}

/// The plain listing, shared with `run_diagnostics`, which walks instances
/// for its report but has no Tauri state of its own.
pub(crate) async fn list_instances_inner(root: &Path) -> Result<Vec<InstanceRecord>, String> {
    let dir = instances_root(root);
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
    state: State<'_, PhlState>,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    create_instance_inner(&state.root(), manifest).await
}

async fn create_instance_inner(
    root: &Path,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
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
    let Some(binding) = manifest.api.clone() else {
        return manifest;
    };
    match crate::api_config::apply_create_binding(root, dir, &manifest.id, binding).await {
        Some(applied) => manifest.api = Some(applied),
        None => eprintln!("[phl] api sync at create skipped for {}", manifest.id),
    }
    manifest
}

#[tauri::command]
pub async fn save_instance(
    state: State<'_, PhlState>,
    manifest: InstanceManifest,
) -> Result<(), String> {
    save_instance_inner(&state.root(), manifest).await
}

async fn save_instance_inner(root: &Path, manifest: InstanceManifest) -> Result<(), String> {
    let dir = instance_dir(root, &manifest.id)?;
    // Load before write: refuses a missing instance and — critically — a
    // manifest this build cannot parse. Overwriting the latter would
    // "succeed" while destroying data a newer PHL might still read.
    load_manifest(&dir, &manifest.id).await?;
    write_manifest(&dir, &manifest).await
}

#[tauri::command]
pub async fn delete_instance(
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
) -> Result<(), String> {
    delete_instance_inner(&state.root(), &id, &processes).await
}

async fn delete_instance_inner(root: &Path, id: &str, processes: &Processes) -> Result<(), String> {
    let dir = instance_dir(root, id)?;
    assert_inside_instances(root, &dir)?;
    // Canonical containment on top of the lexical one: a directory junction
    // planted at the instance path must not redirect `remove_dir_all`.
    ensure_under_root(&instances_root(root), &dir)?;
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
    state: State<'_, PhlState>,
    transfer_id: String,
    source_id: String,
    manifest: InstanceManifest,
    on_progress: Channel<CloneProgress>,
) -> Result<InstanceRecord, String> {
    let flag = transfers.take(&transfer_id);
    let result = run_clone(&flag, &state.root(), &source_id, manifest, &on_progress).await;
    transfers.release(&transfer_id);
    result
}

#[tauri::command]
pub async fn instance_disk_usage(state: State<'_, PhlState>, id: String) -> Result<u64, String> {
    let dir = instance_dir(&state.root(), &id)?;
    tokio::task::spawn_blocking(move || dir_size(&dir))
        .await
        .map_err(|e| e.to_string())
}

/// Directories under `<root>/instances` that carry no manifest — interrupted
/// creates, and plugin trees written before instances were real. Surfacing
/// them gives the user a way to reclaim the space.
#[tauri::command]
pub async fn scan_orphan_instances(state: State<'_, PhlState>) -> Result<Vec<OrphanDir>, String> {
    scan_orphan_instances_inner(&state.root()).await
}

pub(crate) async fn scan_orphan_instances_inner(root: &Path) -> Result<Vec<OrphanDir>, String> {
    let dir = instances_root(root);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        // Keyed on "can this be read as an instance", not on the file merely
        // existing: a directory whose manifest fails to parse is exactly the
        // one the user cannot see anywhere else, so it belongs in this list.
        // A manifest from a *newer* PHL is the exception — that is a valid
        // instance a newer build can still read, not reclaimable junk.
        if !path.is_dir() {
            continue;
        }
        match classify_manifest(&path).await {
            ManifestRead::Ok(_) | ManifestRead::UnsupportedSchema { .. } => continue,
            ManifestRead::Missing | ManifestRead::Corrupt(_) => {}
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
pub async fn delete_orphan_instance(
    state: State<'_, PhlState>,
    name: String,
) -> Result<(), String> {
    delete_orphan_instance_inner(&state.root(), &name).await
}

async fn delete_orphan_instance_inner(root: &Path, name: &str) -> Result<(), String> {
    let dir = instances_root(root).join(sanitize_segment(name, "目录名")?);
    assert_inside_instances(root, &dir)?;
    ensure_under_root(&instances_root(root), &dir)?;
    // Refuse anything that turns out to be a real instance after all — an
    // unparseable manifest must stay deletable, or the directory would be
    // unreclaimable from the UI, but a manifest from a *newer* PHL belongs to
    // an instance a newer build can still read; deleting it here would trade
    // a "wrong version" problem for permanent data loss.
    match classify_manifest(&dir).await {
        ManifestRead::Ok(_) => return Err("该目录是一个有效实例，请从实例页删除".into()),
        ManifestRead::UnsupportedSchema { found, supported } => {
            return Err(format!(
                "该目录是 schema 版本 {found} 的实例（当前支持 {supported}），请升级 PHL 后再删除"
            ))
        }
        ManifestRead::Missing | ManifestRead::Corrupt(_) => {}
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
pub async fn export_instance_bundle(
    state: State<'_, PhlState>,
    id: String,
    dest: String,
) -> Result<(), String> {
    export_instance_bundle_inner(&state.root(), &id, &dest).await
}

/// Exports to `dest`, which comes from the user's save dialog and is
/// deliberately *not* confined to the root — the confinement applies to what
/// gets read, while the destination is the user's own choice of file.
async fn export_instance_bundle_inner(root: &Path, id: &str, dest: &str) -> Result<(), String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
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
    tokio::fs::write(dest, body)
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

#[tauri::command]
pub async fn import_instance_bundle(
    state: State<'_, PhlState>,
    path: String,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    import_instance_bundle_inner(&state.root(), &path, manifest).await
}

/// Creates an instance from a bundle file. Identity (id, name, port) comes
/// from the importer so collisions stay a frontend concern; everything that
/// defines the *environment* — version, runtime, profile, env, args — comes
/// from the bundle. Plugin files are not in a bundle by design: the records
/// travel, the reinstall goes through the normal plugin pipeline.
async fn import_instance_bundle_inner(
    root: &Path,
    path: &str,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    let bundle = read_bundle_file(path).await?;

    let mut manifest = manifest;
    manifest.kind = bundle.instance.kind;
    manifest.hue = bundle.instance.hue;
    manifest.version_id = bundle.instance.version_id;
    manifest.runtime_id = bundle.instance.runtime_id;
    manifest.profile = bundle.instance.profile;
    manifest.note = Some("从 Bundle 导入".into());
    manifest.env = sanitize_imported_env(bundle.instance.env);
    manifest.args = bundle.instance.args;

    let dir = build_instance_tree(root, &manifest).await?;
    // Same "boots configured" promise as a normal create: the imported
    // instance inherits the global library unless the manifest says otherwise.
    let manifest = apply_api_at_create(root, &dir, manifest).await;
    Ok(build_record(&dir, manifest).await)
}

/* ------------------------------ snapshots ----------------------------- */

/// Environment variables that let their value execute code, or redirect the
/// process to a different runtime, and so must never survive an import.
///
/// A bundle is the format PHL tells users to share, so its contents are
/// attacker-supplied by design. `run_launch` applies instance env verbatim
/// (minus `DSH_HOME`), which means an imported `NODE_OPTIONS=--require
/// C:\evil.js` would run on the first 启动. Filtering belongs here, at the
/// trust boundary, rather than in the launcher's own allow-list.
const UNSAFE_IMPORT_ENV: &[&str] = &[
    "NODE_OPTIONS",
    "NODE_REPL_EXTERNAL_MODULE",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "PATH",
    "NODE_PATH",
];

fn sanitize_imported_env(env: HashMap<String, String>) -> HashMap<String, String> {
    env.into_iter()
        .filter(|(key, _)| {
            let upper = key.to_ascii_uppercase();
            // `DSH_HOME` is the isolation boundary and is recomputed per
            // instance anyway; the rest are code-injection vectors.
            upper != "DSH_HOME" && !UNSAFE_IMPORT_ENV.contains(&upper.as_str())
        })
        .collect()
}

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

async fn run_snapshot_create<F: Fn(CloneProgress) + Send + Sync>(
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
    ensure_under_root(&instances_root(root), &snap_dir)?;
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
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("复制 {} 失败: {e}", entry.path().display()))?;
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
    match classify_manifest(dir).await {
        ManifestRead::Ok(manifest) => Some(*manifest),
        ManifestRead::Missing => None,
        ManifestRead::Corrupt(e) => {
            // Silently returning None here made the instance vanish from the
            // list *and* from the orphan view (which used to key off the file
            // merely existing), leaving the user no signal at all beyond their
            // instance being gone. It is now reported as reclaimable, and the
            // reason goes to the log.
            eprintln!("[phl] 无法解析 {}: {e}", manifest_path(dir).display());
            None
        }
        ManifestRead::UnsupportedSchema { found, supported } => {
            eprintln!(
                "[phl] {}: schema 版本 {found} 超出当前支持的 {supported}，已隐藏该实例（请升级 PHL）",
                manifest_path(dir).display()
            );
            None
        }
    }
}

/// Load a manifest for an id-addressed command. Unlike `read_manifest`, every
/// failure mode is an explicit error — in particular a schema this build
/// cannot parse must fail loudly instead of being overwritten by a save.
pub(crate) async fn load_manifest(dir: &Path, id: &str) -> Result<InstanceManifest, String> {
    match classify_manifest(dir).await {
        ManifestRead::Ok(manifest) => Ok(*manifest),
        ManifestRead::Missing => Err(format!("实例不存在或缺少清单: {id}")),
        ManifestRead::Corrupt(e) => Err(format!("实例 {id} 清单损坏: {e}")),
        ManifestRead::UnsupportedSchema { found, supported } => Err(format!(
            "实例 {id} 的清单 schema 版本过新: {found}（当前支持 {supported}），请升级 PHL 后再操作"
        )),
    }
}

/// Resolves an instance's active profile directory — the one owning
/// `node_modules` and `cordis.patch.yml` — from the instance id. Plugin
/// commands key on this id so the WebView never supplies a filesystem path.
pub(crate) async fn profile_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
    Ok(profile_root(&dir, &manifest.profile))
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
    let mut manifest = load_manifest(&dir, id).await?;
    manifest.api = Some(binding.clone());
    write_manifest(&dir, &manifest).await
}

async fn write_manifest(dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    // The schema version is backend-owned: whatever the caller sent, the file
    // always records the format this build writes. Callers load their copy
    // from disk (already migrated) or author a fresh one — either way the
    // stamp must reflect this build, not round-trip stale state.
    let mut manifest = manifest.clone();
    manifest.schema_version = MANIFEST_SCHEMA_VERSION;

    let path = manifest_path(dir);
    // One-time backup when this write changes the on-disk schema — a legacy
    // manifest gaining its version stamp. If the upgrade write is somehow
    // interrupted, the `.bak` still carries the last readable state.
    if let Ok(raw) = tokio::fs::read_to_string(&path).await {
        let on_disk_version = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("schemaVersion").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;
        if on_disk_version != MANIFEST_SCHEMA_VERSION {
            let _ = tokio::fs::copy(&path, dir.join("instance.json.legacy.bak")).await;
        }
    }

    let body = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // truncated manifest — that would make the instance unreadable entirely.
    let tmp = dir.join("instance.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))?;
    tokio::fs::rename(&tmp, path)
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

    let copy_result = copy_tree_with_progress(
        source.clone(),
        staging.clone(),
        Arc::clone(flag),
        SkipRule::RunStateAtRoot,
        &|progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await;

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

/// What a copy is allowed to leave behind.
///
/// This is not a detail: the same `copy_tree` serves cloning an instance and
/// relocating the entire data root, and those want opposite things. Migrating
/// with the clone's filter silently dropped every instance's `snapshots/` and
/// `logs/` and then deleted the source — destroying the user's only rollback
/// points while reporting success.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipRule {
    /// Copy everything. Required whenever the copy replaces the original.
    Nothing,
    /// Drop run state, but only at the tree's own root. A nested `logs/` deep
    /// inside `node_modules` belongs to whatever package created it and is
    /// part of that package, not PHL's per-instance history.
    RunStateAtRoot,
}

impl SkipRule {
    fn skips(self, name: &str) -> bool {
        self == SkipRule::RunStateAtRoot && skipped(name)
    }
    /// Recursion always descends with `Nothing`: the rule only ever applies to
    /// the entries directly under the root it was given.
    fn inside(self) -> Self {
        SkipRule::Nothing
    }
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

/// Shared by cloning, snapshots and cross-drive relocation. Completion comes
/// from the worker result, never from a closed progress channel. Awaiting the
/// worker also ensures cancellation cannot race staging-directory cleanup.
pub(crate) async fn copy_tree_with_progress<F: Fn(CloneProgress) + Send + Sync>(
    from: PathBuf,
    to: PathBuf,
    flag: Arc<AtomicBool>,
    skip: SkipRule,
    on_progress: &F,
) -> Result<u64, String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
    let worker = tokio::task::spawn_blocking(move || {
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let total = match skip {
            SkipRule::Nothing => dir_size(&from),
            SkipRule::RunStateAtRoot => dir_size_skipping(&from),
        };
        let mut done = 0;
        copy_tree(&from, &to, &flag, &mut done, total, &tx, skip)?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        Ok(done)
    });
    while let Some(message) = rx.recv().await {
        let (bytes_done, bytes_total) = message?;
        on_progress(CloneProgress {
            progress: if bytes_total == 0 {
                1.0
            } else {
                (bytes_done as f64 / bytes_total as f64).min(1.0)
            },
            bytes_done,
            bytes_total,
        });
    }
    worker.await.map_err(|e| format!("复制线程异常退出: {e}"))?
}

pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    flag: &AtomicBool,
    done: &mut u64,
    total: u64,
    tx: &tokio::sync::mpsc::Sender<Result<(u64, u64), String>>,
    skip: SkipRule,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取复制源目录失败: {e}"))?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = entry.file_name();
        if skip.skips(&name.to_string_lossy()) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        let meta = entry
            .metadata()
            .map_err(|e| format!("读取复制源属性失败 {}: {e}", src.display()))?;
        // A relocation deletes the source afterwards. Silently skipping a
        // link (or following it outside the tree) would lose or duplicate data.
        if meta.file_type().is_symlink() {
            return Err(format!(
                "复制源包含符号链接，请先处理后重试: {}",
                src.display()
            ));
        }
        if meta.is_dir() {
            copy_tree(&src, &dst, flag, done, total, tx, skip.inside())?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("复制失败 {}: {e}", src.display()))?;
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

    #[tokio::test]
    async fn async_copy_reports_worker_failure_instead_of_success() {
        let root = temp_root("async-copy-error");
        let result = copy_tree_with_progress(
            root.join("missing"),
            root.join("target"),
            Arc::new(AtomicBool::new(false)),
            SkipRule::Nothing,
            &|_| {},
        )
        .await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn async_copy_cancellation_finishes_before_cleanup() {
        let root = temp_root("async-copy-cancel");
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(&source).unwrap();
        for i in 0..64 {
            std::fs::write(source.join(format!("file-{i}")), b"test").unwrap();
        }
        let flag = Arc::new(AtomicBool::new(false));
        let result = copy_tree_with_progress(
            source,
            target.clone(),
            Arc::clone(&flag),
            SkipRule::Nothing,
            &|_| {
                flag.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "cancelled");
        // No worker remains to recreate target after this removal.
        std::fs::remove_dir_all(&target).unwrap();
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-inst-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str, name: &str) -> InstanceManifest {
        InstanceManifest {
            schema_version: 0,
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

    #[test]
    fn skip_rule_applies_only_at_the_root_and_never_when_relocating() {
        // A clone drops the instance's own history …
        assert!(SkipRule::RunStateAtRoot.skips("snapshots"));
        assert!(SkipRule::RunStateAtRoot.skips("logs"));
        // … but a `logs/` nested inside a package belongs to that package.
        assert!(!SkipRule::RunStateAtRoot.inside().skips("logs"));
        // Relocating the data root replaces the original and then deletes it,
        // so it must copy everything — dropping snapshots here destroyed them.
        assert!(!SkipRule::Nothing.skips("snapshots"));
        assert!(!SkipRule::Nothing.skips("logs"));
    }

    #[test]
    fn imported_env_drops_code_injection_vectors() {
        let mut env = HashMap::new();
        env.insert("NODE_OPTIONS".into(), "--require C:\\evil.js".into());
        env.insert("node_options".into(), "--require C:\\evil.js".into());
        env.insert("LD_PRELOAD".into(), "/tmp/evil.so".into());
        env.insert("DSH_HOME".into(), "C:\\elsewhere".into());
        env.insert("MY_API_KEY".into(), "keep-me".into());

        let safe = sanitize_imported_env(env);
        assert_eq!(safe.len(), 1, "only the harmless variable survives");
        assert_eq!(safe.get("MY_API_KEY").map(String::as_str), Some("keep-me"));
    }

    #[tokio::test]
    async fn instance_lifecycle_on_disk() {
        let root = temp_root("life");

        // Nothing created yet: an absent instances/ dir is not an error.
        assert!(list_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());

        let record = create_instance_inner(root.as_path(), manifest("demo-a1b2", "Demo"))
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
        assert!(
            create_instance_inner(root.as_path(), manifest("demo-a1b2", "Demo"))
                .await
                .is_err()
        );

        let listed = list_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.name, "Demo");

        // Rename survives a round trip through disk.
        let mut renamed = manifest("demo-a1b2", "Renamed");
        renamed.port = 9001;
        save_instance_inner(root.as_path(), renamed).await.unwrap();
        let listed = list_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(listed[0].manifest.name, "Renamed");
        assert_eq!(listed[0].manifest.port, 9001);

        delete_instance_inner(root.as_path(), "demo-a1b2", &Processes::default())
            .await
            .unwrap();
        assert!(!dir.exists());
        assert!(list_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Serializes a manifest and strips the schema stamp, simulating a file
    /// written before schema versioning existed.
    fn legacy_manifest_body(m: &InstanceManifest) -> String {
        let mut value = serde_json::to_value(m).unwrap();
        value.as_object_mut().unwrap().remove("schemaVersion");
        serde_json::to_string_pretty(&value).unwrap()
    }

    #[tokio::test]
    async fn legacy_manifest_round_trips_and_upgrades_on_first_write() {
        let root = temp_root("legacy");
        create_instance_inner(root.as_path(), manifest("legacy-001", "Legacy"))
            .await
            .unwrap();
        let dir = instance_dir(root.as_path(), "legacy-001").unwrap();
        std::fs::write(
            manifest_path(&dir),
            legacy_manifest_body(&manifest("legacy-001", "Legacy")),
        )
        .unwrap();

        // A pre-schema manifest still lists, already carrying the migrated
        // version in memory even though the file has not changed yet.
        let listed = list_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        assert!(
            !dir.join("instance.json.legacy.bak").exists(),
            "reading alone must not rewrite anything"
        );

        // The first write stamps the version and keeps the original around.
        save_instance_inner(root.as_path(), manifest("legacy-001", "Renamed"))
            .await
            .unwrap();
        let on_disk: InstanceManifest =
            serde_json::from_str(&std::fs::read_to_string(manifest_path(&dir)).unwrap()).unwrap();
        assert_eq!(on_disk.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(on_disk.name, "Renamed");
        let backup: InstanceManifest = serde_json::from_str(
            &std::fs::read_to_string(dir.join("instance.json.legacy.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            backup.name, "Legacy",
            "the pre-upgrade manifest is preserved"
        );

        // The backup is a one-time artifact: a later write must not refresh it
        // with already-migrated content.
        save_instance_inner(root.as_path(), manifest("legacy-001", "Renamed Again"))
            .await
            .unwrap();
        let backup: InstanceManifest = serde_json::from_str(
            &std::fs::read_to_string(dir.join("instance.json.legacy.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup.name, "Legacy");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn manifest_from_a_newer_phl_is_hidden_refused_and_protected() {
        let root = temp_root("future");
        create_instance_inner(root.as_path(), manifest("future-01", "Future"))
            .await
            .unwrap();
        let dir = instance_dir(root.as_path(), "future-01").unwrap();
        let mut value = serde_json::to_value(manifest("future-01", "Future")).unwrap();
        value.as_object_mut().unwrap().insert(
            "schemaVersion".into(),
            serde_json::json!(MANIFEST_SCHEMA_VERSION + 7),
        );
        std::fs::write(
            manifest_path(&dir),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();

        // Invisible in the instance list, and — unlike a corrupt manifest —
        // not offered as reclaimable junk either: a newer PHL can still read it.
        assert!(list_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());
        assert!(scan_orphan_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());

        // Every id-addressed path refuses with the actual reason.
        let err = save_instance_inner(root.as_path(), manifest("future-01", "Overwrite"))
            .await
            .unwrap_err();
        assert!(err.contains("过新"), "{err}");
        let err = export_instance_bundle_inner(
            root.as_path(),
            "future-01",
            &root.join("out.json").to_string_lossy(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("过新"), "{err}");

        // And the reclaim door stays shut.
        let err = delete_orphan_instance_inner(root.as_path(), "future-01")
            .await
            .unwrap_err();
        assert!(err.contains("schema"), "{err}");
        assert!(dir.exists(), "the instance was never touched");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unreadable_manifest_stays_visible_and_reclaimable() {
        let root = temp_root("corrupt");
        create_instance_inner(root.as_path(), manifest("bad-0001", "Bad"))
            .await
            .unwrap();

        // Simulate version skew / a hand edit the parser rejects.
        let dir = instance_dir(root.as_path(), "bad-0001").unwrap();
        std::fs::write(manifest_path(&dir), "{ not json ").unwrap();

        // It must not simply vanish: absent from the instance list, but listed
        // in the reclaim view and deletable from there.
        assert!(list_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());
        let orphans = scan_orphan_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].name, "bad-0001");

        delete_orphan_instance_inner(root.as_path(), "bad-0001")
            .await
            .unwrap();
        assert!(!dir.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn plugins_are_read_back_from_disk() {
        let root = temp_root("scan");
        create_instance_inner(root.as_path(), manifest("scan-0001", "Scan"))
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

        let listed = list_instances_inner(root.as_path()).await.unwrap();
        let plugins = &listed[0].plugins;
        assert_eq!(
            plugins.len(),
            2,
            "staging dirs and unmarked packages skipped"
        );

        let widget = plugins
            .iter()
            .find(|p| p.plugin_id == "acme/widget")
            .unwrap();
        assert_eq!(widget.version, "1.2.0");
        assert!(widget.enabled);
        // The registry id must survive the round trip: enable/uninstall use it
        // instead of re-deriving one from the (possibly offline) catalog.
        assert_eq!(widget.registry_id, "@acme/widget");

        let solo = plugins.iter().find(|p| p.plugin_id == "who/solo").unwrap();
        assert!(
            !solo.enabled,
            "cordis.patch.yml is the enabled-state source"
        );
        assert_eq!(solo.registry_id, "solo");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn orphan_dirs_are_reported_and_reclaimable() {
        let root = temp_root("orphan");
        create_instance_inner(root.as_path(), manifest("real-0001", "Real"))
            .await
            .unwrap();

        // A tree with no manifest — what an interrupted create leaves behind.
        let orphan = root.join("instances").join("leftover");
        std::fs::create_dir_all(orphan.join("dsh-home")).unwrap();
        std::fs::write(orphan.join("dsh-home").join("blob"), vec![0u8; 2048]).unwrap();

        let found = scan_orphan_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "leftover");
        assert_eq!(found[0].size, 2048);

        // A real instance must never be removable through this door.
        assert!(delete_orphan_instance_inner(root.as_path(), "real-0001")
            .await
            .is_err());

        delete_orphan_instance_inner(root.as_path(), "leftover")
            .await
            .unwrap();
        assert!(!orphan.exists());
        assert!(scan_orphan_instances_inner(root.as_path())
            .await
            .unwrap()
            .is_empty());

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

        assert_eq!(
            dir_size_skipping(&from),
            512,
            "logs excluded from the total"
        );

        let to = root.join("dst");
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        // `copy_tree` uses `blocking_send`, so it has to run off the runtime
        // thread — exactly as `run_clone` does in production.
        let src = from.clone();
        let dst = to.clone();
        let worker = std::thread::spawn(move || {
            let flag = AtomicBool::new(false);
            let mut done = 0u64;
            copy_tree(
                &src,
                &dst,
                &flag,
                &mut done,
                512,
                &tx,
                SkipRule::RunStateAtRoot,
            )
            .map(|()| done)
        });

        let mut events = 0;
        while rx.recv().await.is_some() {
            events += 1;
        }
        let done = worker.join().unwrap().unwrap();

        assert!(to.join("dsh-home").join("a").exists());
        assert!(
            !to.join("logs").exists(),
            "run state not carried into a clone"
        );
        assert_eq!(done, 512);
        assert!(events > 0, "progress was reported");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bundle_roundtrip_export_preview_import() {
        let root = temp_root("bundle");
        let source = create_instance_inner(root.as_path(), manifest("src-a1b2", "Source"))
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
        export_instance_bundle_inner(root.as_path(), "src-a1b2", &dest.to_string_lossy())
            .await
            .unwrap();

        let preview = read_instance_bundle(dest.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(preview.name, "Source");
        assert_eq!(preview.plugin_count, 1);

        // Identity comes from the importer; environment fields come from the
        // bundle — the placeholder version id here must NOT survive.
        let mut importer = manifest("dst-c3d4", "Restored");
        importer.version_id = "placeholder".into();
        importer.port = 9999;
        let imported =
            import_instance_bundle_inner(root.as_path(), &dest.to_string_lossy(), importer)
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
        let err = import_instance_bundle_inner(
            root.as_path(),
            &dest.to_string_lossy(),
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
        std::fs::write(
            &path,
            r#"{"phlBundle":99,"exportedAt":"","instance":{},"plugins":[]}"#,
        )
        .unwrap();
        let err = read_instance_bundle(path.to_string_lossy().into_owned())
            .await
            .unwrap_err();
        assert!(err.contains("不支持的 Bundle 版本"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn snapshot_create_restore_delete_roundtrip() {
        let root = temp_root("snap");
        let record = create_instance_inner(root.as_path(), manifest("snap-a1b2", "Snapped"))
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
            let snap = run_snapshot_create(&flag, &processes, root.as_path(), "snap-a1b2", &|p| {
                let _ = tx.try_send(p);
            })
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
            dir.join("snapshots")
                .join(&snap.id)
                .join("dsh-home")
                .exists(),
            "snapshot tree exists under snapshots/"
        );
        // Listed from the record, newest first.
        let listed = list_instances_inner(root.as_path()).await.unwrap();
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
        assert_eq!(
            scan_snapshots(&dir).await.len(),
            1,
            "restore keeps the snapshot"
        );

        delete_snapshot_inner(root.as_path(), "snap-a1b2", &snap.id, &processes)
            .await
            .unwrap();
        assert_eq!(scan_snapshots(&dir).await.len(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }
}
