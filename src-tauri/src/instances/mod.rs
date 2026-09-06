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

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::api_config::ApiBinding;
use crate::launch::Processes;
use crate::paths::{ensure_under_root, sanitize_segment, PhlState};
use crate::plugins::disabled_plugin_ids;
use crate::versions::{now_iso, Transfers};

pub(crate) mod bundle;
pub(crate) mod copy;
pub(crate) mod env_policy;
pub(crate) mod manifest;
pub(crate) mod snapshot;

pub(crate) use copy::{copy_tree_with_progress, dir_size, SkipRule};
use manifest::{classify_manifest, write_manifest, ManifestRead};
pub(crate) use manifest::{load_manifest, read_manifest, InstanceManifest};
#[cfg(test)]
use snapshot::{delete_snapshot_inner, restore_snapshot_inner, run_snapshot_create};
use snapshot::{scan_snapshots, SnapshotFile};

/* ----------------------------- wire types ----------------------------- */

/// What `instance.json` holds: everything that defines the instance and
/// nothing that can be observed from the filesystem. The plugin list is
/// absent on purpose — see `scan_plugins`.
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
    /// verified | pinned | unverified — from the install record (T-107).
    /// Markers written before trust existed read back as `unknown`.
    #[serde(default)]
    pub trust: String,
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
    #[serde(default)]
    trust: String,
}

/* ------------------------------- paths -------------------------------- */

pub(crate) fn instances_root(root: &Path) -> PathBuf {
    root.join("instances")
}

pub(crate) fn instance_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(instances_root(root).join(sanitize_segment(id, "实例 id")?))
}

pub(crate) fn profile_root(dir: &Path, profile: &str) -> PathBuf {
    dir.join("dsh-home").join("profiles").join(profile)
}

pub(crate) fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("instance.json")
}

/// Refuses any path that is not a direct child of `<root>/instances`.
///
/// `sanitize_segment` already rejects separators, so this is belt and braces
/// — but the operation on the other side is `remove_dir_all`, and a guard
/// that only exists in one place is one refactor away from being gone.
pub(crate) fn assert_inside_instances(root: &Path, dir: &Path) -> Result<(), String> {
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
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    manifest: InstanceManifest,
) -> Result<InstanceRecord, String> {
    let label_id = manifest.id.clone();
    crate::resources::guarded(
        crate::resources::next_task_id("instance-create"),
        "instance-create",
        format!("创建实例 {label_id}"),
        vec![crate::resources::Resource::Instance(label_id)],
        None,
        &locks,
        &tasks,
        move |_| async move { create_instance_inner(&state.root(), manifest).await },
    )
    .await
}

pub(crate) async fn create_instance_inner(
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
pub(crate) async fn apply_api_at_create(
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
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    manifest: InstanceManifest,
) -> Result<(), String> {
    let label_id = manifest.id.clone();
    crate::resources::guarded(
        crate::resources::next_task_id("instance-save"),
        "instance-save",
        format!("保存实例 {label_id}"),
        vec![crate::resources::Resource::Instance(label_id)],
        None,
        &locks,
        &tasks,
        move |_| async move { save_instance_inner(&state.root(), manifest).await },
    )
    .await
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
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, Processes>,
    state: State<'_, PhlState>,
    id: String,
) -> Result<(), String> {
    crate::resources::guarded(
        crate::resources::next_task_id("instance-delete"),
        "instance-delete",
        format!("删除实例 {id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move { delete_instance_inner(&state.root(), &id, &processes).await },
    )
    .await
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
    snapshot::ensure_not_running(processes, id)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| format!("无法删除实例目录: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn clone_instance(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    transfer_id: String,
    source_id: String,
    manifest: InstanceManifest,
    on_progress: Channel<CloneProgress>,
) -> Result<InstanceRecord, String> {
    let flag = transfers.take(&transfer_id);
    let new_id = manifest.id.clone();
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "instance-clone",
        format!("克隆实例 {source_id} → {new_id}"),
        vec![
            crate::resources::Resource::Instance(source_id.clone()),
            crate::resources::Resource::Instance(new_id),
        ],
        Some(flag.clone()),
        &locks,
        &tasks,
        move |_| async move {
            let r = run_clone(&flag, &state.root(), &source_id, manifest, &on_progress).await;
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

/* ------------------------------ internals ----------------------------- */

/// The shared staging + rename create path (used by create and bundle
/// import). A create that fails halfway must not leave a directory that looks
/// like an instance but has no manifest — that is exactly how the current
/// orphans came about.
pub(crate) async fn build_instance_tree(
    root: &Path,
    manifest: &InstanceManifest,
) -> Result<PathBuf, String> {
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

pub(crate) async fn build_record(dir: &Path, manifest: InstanceManifest) -> InstanceRecord {
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
pub(crate) async fn scan_plugins(profile: &Path) -> Vec<InstalledPluginInfo> {
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
            trust: if marker.trust.is_empty() {
                "unknown".to_string()
            } else {
                marker.trust
            },
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
#[cfg(test)]
mod tests {
    use super::bundle::import_instance_bundle_inner;
    use super::copy::dir_size_skipping;
    use super::manifest::MANIFEST_SCHEMA_VERSION;
    use super::*;
    use bundle::{export_instance_bundle_inner, read_bundle_inner};
    use copy::{copy_tree, skipped};
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

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
    fn imported_env_drops_injection_vectors_and_credential_values() {
        use super::env_policy::partition_imported_env;
        let mut env = HashMap::new();
        env.insert("NODE_OPTIONS".into(), "--require C:\\evil.js".into());
        env.insert("node_options".into(), "--require C:\\evil.js".into());
        env.insert("LD_PRELOAD".into(), "/tmp/evil.so".into());
        env.insert("DSH_HOME".into(), "C:\\elsewhere".into());
        env.insert("MY_API_KEY".into(), "keep-me".into());
        env.insert("HTTP_PROXY".into(), "http://proxy:8080".into());

        let (safe, credentials) = partition_imported_env(env, &HashSet::new());
        assert_eq!(credentials, vec!["MY_API_KEY".to_string()]);
        assert_eq!(
            safe.len(),
            1,
            "the harmless variable survives with its value"
        );
        assert_eq!(
            safe.get("HTTP_PROXY").map(String::as_str),
            Some("http://proxy:8080")
        );
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
        let report =
            export_instance_bundle_inner(root.as_path(), "src-a1b2", &dest.to_string_lossy())
                .await
                .unwrap();
        assert!(report.credentials.is_empty() && report.machine_only.is_empty());

        let preview = read_bundle_inner(root.as_path(), &dest.to_string_lossy())
            .await
            .unwrap();
        assert_eq!(preview.name, "Source");
        assert_eq!(preview.plugin_count, 1);

        // Identity comes from the importer; environment fields come from the
        // bundle — the placeholder version id here must NOT survive.
        let mut importer = manifest("dst-c3d4", "Restored");
        importer.version_id = "placeholder".into();
        importer.port = 9999;
        let outcome =
            import_instance_bundle_inner(root.as_path(), &dest.to_string_lossy(), importer)
                .await
                .unwrap();
        let imported = outcome.record;

        assert!(outcome.credentials.is_empty());
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
        let err = read_bundle_inner(root.as_path(), &path.to_string_lossy())
            .await
            .unwrap_err();
        assert!(err.contains("不支持的 Bundle 版本"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The shareability promise, end to end: an instance whose env carries a
    /// fictional credential under an explicit (library-declared) name and a
    /// heuristic one exports neither value; both names come back as "needs
    /// re-configuration", while an ordinary variable keeps its value.
    #[tokio::test]
    async fn bundle_export_omits_credential_values_and_names_them() {
        let root = temp_root("bundlesecret");
        create_instance_inner(root.as_path(), manifest("sec-a1b2", "Secret"))
            .await
            .unwrap();

        // The library declares MY_GATE_SLOT a credential carrier — a name the
        // heuristic alone would not catch.
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/api.json"),
            r#"{"version":1,"providers":[{"id":"gate","name":"Gate","apiKeyEnv":"MY_GATE_SLOT"}]}"#,
        )
        .unwrap();

        let dir = root.join("instances/sec-a1b2");
        let raw = std::fs::read_to_string(manifest_path(&dir)).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["env"] = serde_json::json!({
            "MY_GATE_SLOT": "phrase-fictional",
            "MY_API_KEY": "key-fictional",
            "HTTP_PROXY": "http://proxy:8080",
            "PATH": "C:\\bin"
        });
        std::fs::write(
            manifest_path(&dir),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();

        let dest = root.join("out/secret.phl-bundle.json");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let report =
            export_instance_bundle_inner(root.as_path(), "sec-a1b2", &dest.to_string_lossy())
                .await
                .unwrap();

        assert_eq!(
            report.credentials,
            vec!["MY_API_KEY".to_string(), "MY_GATE_SLOT".to_string()]
        );
        assert_eq!(report.machine_only, vec!["PATH".to_string()]);

        let body = std::fs::read_to_string(&dest).unwrap();
        assert!(
            !body.contains("phrase-fictional"),
            "explicit credential leaked"
        );
        assert!(
            !body.contains("key-fictional"),
            "heuristic credential leaked"
        );
        assert!(
            body.contains("http://proxy:8080"),
            "the ordinary variable keeps its value"
        );
        // The names travel, the values do not — the importer needs to know
        // what to re-enter, not what was there.
        assert!(body.contains("MY_GATE_SLOT"));

        // A fresh PHL reading the file back reports the same two names.
        let preview = read_bundle_inner(root.as_path(), &dest.to_string_lossy())
            .await
            .unwrap();
        assert_eq!(
            preview.credentials,
            vec!["MY_API_KEY".to_string(), "MY_GATE_SLOT".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Old format-1 bundles can carry credential *values*; the import filter
    /// is the last line, so importing one strips those values, reports the
    /// names, and keeps ordinary variables intact — the written manifest
    /// never sees the secret.
    #[tokio::test]
    async fn bundle_import_strips_format1_credential_values_and_reports_them() {
        let root = temp_root("bundleold");
        let path = root.join("legacy.phl-bundle.json");
        std::fs::write(
            &path,
            r#"{
                "phlBundle": 1,
                "exportedAt": "2025-01-01T00:00:00Z",
                "instance": {
                    "id": "legacy", "name": "Legacy", "kind": "sandbox", "hue": 0,
                    "versionId": "dsh-0.1.0", "runtimeId": "node-22", "port": 8080,
                    "autoPort": true, "profile": "default", "createdAt": "2025-01-01T00:00:00Z",
                    "env": {"MY_API_KEY": "legacy-secret", "HTTP_PROXY": "http://proxy:8080"},
                    "args": []
                },
                "plugins": []
            }"#,
        )
        .unwrap();

        let outcome = import_instance_bundle_inner(
            root.as_path(),
            &path.to_string_lossy(),
            manifest("old-c9d8", "Legacy"),
        )
        .await
        .unwrap();
        assert_eq!(outcome.credentials, vec!["MY_API_KEY".to_string()]);
        assert_eq!(
            outcome
                .record
                .manifest
                .env
                .get("HTTP_PROXY")
                .map(String::as_str),
            Some("http://proxy:8080"),
            "the ordinary variable survives the import"
        );

        let written =
            std::fs::read_to_string(manifest_path(&root.join("instances/old-c9d8"))).unwrap();
        assert!(
            !written.contains("legacy-secret"),
            "the secret never reaches the imported instance's manifest"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// T-110 Windows 专项：中文 / 空格 / Unicode 数据根下的完整实例生命周期。
    /// 每一段路径都直接进入文件系统调用，正是历史上一堆“莫名失败”的来源。
    #[tokio::test]
    async fn instance_lifecycle_survives_windows_path_quirks() {
        let quirks: &[&str] = &["phl 测试 中文", "phl with spaces", "phl-ünïcødé-④"];
        for tag in quirks {
            let root = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let record = create_instance_inner(&root, manifest("win-path-1", "Win 路径实例"))
                .await
                .unwrap();
            assert!(record.dsh_home.contains(tag));

            // Rename + clone survive the quirky root.
            save_instance_inner(&root, manifest("win-path-1", "改名后的实例"))
                .await
                .unwrap();
            let clone_manifest = manifest("win-path-2", "克隆实例");
            create_instance_inner(&root, clone_manifest).await.unwrap();

            let listed = list_instances_inner(&root).await.unwrap();
            assert_eq!(listed.len(), 2, "{tag}: both instances listed");

            // Orphan scan + snapshot round trip also stay sane.
            let orphans = scan_orphan_instances_inner(&root).await.unwrap();
            assert!(orphans.is_empty(), "{tag}: no orphans");

            let snapshot_dir = root.join("instances").join("win-path-1").join("snapshots");
            std::fs::create_dir_all(&snapshot_dir).unwrap();

            delete_instance_inner(&root, "win-path-2", &Processes::default())
                .await
                .unwrap();
            delete_instance_inner(&root, "win-path-1", &Processes::default())
                .await
                .unwrap();
            assert!(!root.join("instances").join("win-path-1").exists());

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// T-110 Instance 行：快照损坏（缺 dsh-home）时，还原必须干净地失败，
    /// 实例当前的 dsh-home 原封不动。
    #[tokio::test]
    async fn restoring_a_broken_snapshot_fails_and_leaves_the_instance_untouched() {
        use std::collections::HashMap;

        let root = std::env::temp_dir().join(format!("phl-snapsbroken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let manifest = InstanceManifest {
            schema_version: 0,
            id: "broken-1".into(),
            name: "Broken".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            port: 34495,
            auto_port: true,
            profile: "web".into(),
            created_at: now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: HashMap::new(),
            args: Vec::new(),
            api: None,
        };
        create_instance_inner(&root, manifest).await.unwrap();
        let dir = root.join("instances").join("broken-1");
        let settings = dir.join("dsh-home").join("settings.yaml");
        std::fs::write(&settings, b"original").unwrap();

        // A snapshot that never finished: metadata exists, dsh-home is missing.
        let snap_id = "snap-broken";
        let snap_dir = dir.join("snapshots").join(snap_id);
        std::fs::create_dir_all(&snap_dir).unwrap();
        std::fs::write(
            snap_dir.join("snapshot.json"),
            serde_json::json!({
                "id": snap_id, "label": "坏快照", "createdAt": now_iso(),
                "versionId": "dsh-0.1.0", "runtimeId": "node-system",
                "pluginCount": 0, "size": 0
            })
            .to_string(),
        )
        .unwrap();

        let err = restore_snapshot_inner(&root, "broken-1", snap_id, &Processes::default())
            .await
            .unwrap_err();
        assert!(err.contains("无法还原"), "{err}");

        // The live environment is untouched.
        assert_eq!(
            std::fs::read(&settings).unwrap(),
            b"original",
            "dsh-home survived the failed restore"
        );
        assert!(!dir.join(".phl-restore").exists(), "no staging leftover");

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
