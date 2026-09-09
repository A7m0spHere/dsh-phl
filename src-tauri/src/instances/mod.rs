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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::api_config::ApiBinding;
use crate::launch::Processes;
use crate::paths::{ensure_under_root, sanitize_segment, PhlState};
use crate::plugins::cordis::declared_plugin_ids;
use crate::plugins::disabled_plugin_ids;
use crate::versions::{now_iso, Transfers};

pub(crate) mod adoption;
pub(crate) mod bundle;
pub(crate) mod copy;
pub(crate) mod env_policy;
pub(crate) mod manifest;
pub(crate) mod snapshot;

pub(crate) use copy::{
    copy_tree_with_progress, dir_size, repoint_managed_links, LinkMatch, LinkPolicy, SkipRule,
};
use manifest::{classify_manifest, write_manifest, ManifestRead};
pub(crate) use manifest::{
    home_of, load_manifest, profile_root_of, read_manifest, AdoptedFrom, InstanceManifest,
    InstanceSource, ManagementMode,
};
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
    let instance_dir_path = instance_dir(&state.root(), &label_id)?;
    crate::resources::guarded(
        crate::resources::next_task_id("instance-create"),
        "instance-create",
        format!("创建实例 {label_id}"),
        vec![crate::resources::Resource::Instance(label_id)],
        None,
        &locks,
        &tasks,
        move |_| async move {
            let r = create_instance_inner(&state.root(), manifest).await;
            if r.is_ok() {
                invalidate_disk_usage(&instance_dir_path);
            }
            r
        },
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
        move |_| async move {
            let r = delete_instance_inner(&state.root(), &id, &processes).await;
            if r.is_ok() {
                if let Ok(dir) = instance_dir(&state.root(), &id) {
                    invalidate_disk_usage(&dir);
                }
            }
            r
        },
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
        tokio::fs::remove_dir_all(&dir).await.map_err(|e| {
            crate::errors::coded(crate::errors::io_code(&e), format!("无法删除实例目录: {e}"))
        })?;
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

/// TTL for the disk-usage memo (O-12). `measureDiskUsage` fans out over every
/// instance on the storage page and on each `instances` change; walking full
/// `node_modules` trees per call is the exact cost the roadmap says to bound.
/// A short memo collapses the fan-out and repeated visits into one real scan
/// per instance per window, and instance writes invalidate their own entry so
/// the answer never disagrees with a change the user just made.
const DISK_TTL: std::time::Duration = std::time::Duration::from_secs(15);

/// (instance dir) → (bytes, when it was measured). Global because the store
/// has no natural owner for it and the cache must survive across commands.
static DISK_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, std::time::Instant)>>,
> = std::sync::OnceLock::new();

fn disk_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, std::time::Instant)>> {
    DISK_CACHE.get_or_init(Default::default)
}

/// Drop any cached measurement for `dir` — called after anything that writes
/// into an instance tree (create, save, plugin ops, snapshot restore).
pub(crate) fn invalidate_disk_usage(dir: &Path) {
    if let Ok(mut map) = disk_cache().lock() {
        map.remove(dir);
    }
}

#[tauri::command]
pub async fn instance_disk_usage(state: State<'_, PhlState>, id: String) -> Result<u64, String> {
    let dir = instance_dir(&state.root(), &id)?;
    // Fresh memo wins outright; a stale one is still returned while the
    // refresh runs, so concurrent callers share one scan.
    let cached = disk_cache().lock().ok().and_then(|m| m.get(&dir).copied());
    if let Some((bytes, at)) = cached {
        if at.elapsed() < DISK_TTL {
            return Ok(bytes);
        }
    }
    let scan_dir = dir.clone();
    let bytes = tokio::task::spawn_blocking(move || dir_size(&scan_dir))
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(mut m) = disk_cache().lock() {
        m.insert(dir, (bytes, std::time::Instant::now()));
    }
    Ok(bytes)
}

/// Count the DSH sessions this instance's home holds. The "数据" section of
/// the instance detail uses it; it is a filename walk under
/// `<home>/sessions/*/*/session.jsonl*` (the spike's P0 rule: count, never
/// parse). Works for both management modes because `home_of` resolves them.
#[tauri::command]
pub async fn instance_session_count(
    state: State<'_, PhlState>,
    id: String,
) -> Result<usize, String> {
    let dir = instance_dir(&state.root(), &id)?;
    let manifest = load_manifest(&dir, &id).await?;
    let home = home_of(&dir, &manifest);
    tokio::task::spawn_blocking(move || crate::discovery::inspect::count_sessions(&home))
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
    Ok(profile_root_of(&dir, &manifest))
}

/// Whether an instance points at an in-place (external) DSH_HOME.
pub(crate) fn is_external(manifest: &InstanceManifest) -> bool {
    manifest.management_mode == ManagementMode::External
}

/// `profile_dir` for callers about to *write* into the profile: resolves the
/// path through the ungated read helper and clears the external-instance
/// gate in one step, so no mutation command can forget to check it.
pub(crate) async fn writable_profile_dir(
    root: &Path,
    id: &str,
    action: &str,
) -> Result<PathBuf, String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
    reject_external_write(&manifest, id, action)?;
    profile_dir(root, id).await
}

/// Refuse a write into an external instance's DSH_HOME. In-place adoption
/// deliberately keeps PHL out of the user's own directory (spec §3.1), so
/// plugin install/uninstall/enable, snapshot restore and repair must all
/// clear this gate before mutating. Reads are never gated.
pub(crate) fn reject_external_write(
    manifest: &InstanceManifest,
    id: &str,
    action: &str,
) -> Result<(), String> {
    if manifest.management_mode == ManagementMode::External {
        return Err(format!(
            "原地接入的实例「{id}」的 DSH_HOME 由你自己管理，PHL 不会{action}它。需要该能力请改用「复制到 PHL」方式接入"
        ));
    }
    Ok(())
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
    let profile = profile_root_of(dir, &manifest);
    InstanceRecord {
        dsh_home: home_of(dir, &manifest).to_string_lossy().into_owned(),
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
///
/// A package counts as installed when it carries PHL's `phl-plugin.json`
/// marker *or* when `cordis.patch.yml` declares a mount row for it (`id:` or
/// `name:`). The second clause is the out-of-band half: a plugin installed by
/// hand (`pnpm add` into the profile + a written insert block) is exactly as
/// installed to DSH as one PHL committed — it simply lacks PHL's paperwork.
/// Packages that are merely present in `node_modules` (transitive deps,
/// pnpm's store layout) match neither and stay invisible.
pub(crate) async fn scan_plugins(profile: &Path) -> Vec<InstalledPluginInfo> {
    let node_modules = profile.join("node_modules");
    let disabled = disabled_plugin_ids(profile).await;
    let declared = declared_plugin_ids(profile).await;
    let mut out = Vec::new();
    collect_packages(&node_modules, &disabled, &declared, &mut out, true).await;
    out.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
    out
}

/// npm scopes are a directory level (`@scope/pkg`), so the walk descends once
/// into `@…` entries and no further.
async fn collect_packages(
    dir: &Path,
    disabled: &HashSet<String>,
    declared: &HashMap<String, bool>,
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
            Box::pin(collect_packages(&path, disabled, declared, out, false)).await;
            continue;
        }
        if let Ok(raw) = tokio::fs::read_to_string(path.join("phl-plugin.json")).await {
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
            continue;
        }
        // Use the full scoped name; a bare leaf could alias an unrelated package.
        let name = if allow_scopes {
            name
        } else {
            format!(
                "{}/{name}",
                dir.file_name().unwrap_or_default().to_string_lossy()
            )
        };
        let Some(&disabled_here) = declared.get(&name) else {
            continue;
        };
        let version = package_json_version(&path).await;
        out.push(InstalledPluginInfo {
            enabled: !disabled_here,
            registry_id: name.clone(),
            plugin_id: name,
            version,
            trust: "unknown".to_string(),
        });
    }
}

/// The `version` field of a package's `package.json`, for out-of-band
/// installs that carry no PHL marker. Best effort: a broken or absent
/// package.json degrades to an empty version, not a dropped plugin.
async fn package_json_version(pkg_dir: &Path) -> String {
    let Ok(raw) = tokio::fs::read_to_string(pkg_dir.join("package.json")).await else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return String::new();
    };
    value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
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
        return Err(crate::errors::coded(
            crate::errors::ErrCode::NotFound,
            format!("源实例不存在: {source_id}"),
        ));
    }
    // Cloning is a whole-directory copy. An external instance has no home to
    // copy, and its round-tripped `externalHome` would make the clone a second
    // instance pointing at the SAME DSH_HOME — a §1.1 violation. P0 has no
    // clone-semantics for external homes, so refuse rather than fork the tree.
    let source_manifest = load_manifest(&source, source_id).await?;
    if is_external(&source_manifest) {
        return Err(
            "原地接入的实例暂不支持克隆：请改用「接入本机 DSH → 复制到 PHL」再克隆副本".to_string(),
        );
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
        // A real instance's `node_modules` is ~1000 junctions into the shared,
        // immutable `versions/` tree, and pnpm's plugin layout adds junctions
        // into the instance's own `.pnpm`. Version links keep pointing at the
        // shared target; in-tree links follow the copy. Links that leave both
        // are still refused (see `LinkPolicy::Preserve`).
        // `dest`, not `staging`: the staging directory is renamed into place
        // right after the copy, and an absolute in-tree link that named the
        // staging path would dangle the moment it lands.
        LinkPolicy::Preserve {
            source_root: source.clone(),
            dest_root: dest.clone(),
        },
        root.to_path_buf(),
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
    use copy::{copy_tree, skipped, CopyCtx};
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
            LinkPolicy::Preserve {
                source_root: root.join("missing"),
                dest_root: root.join("target"),
            },
            root.clone(),
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
            source.clone(),
            target.clone(),
            Arc::clone(&flag),
            SkipRule::Nothing,
            LinkPolicy::Preserve {
                source_root: source,
                dest_root: target.clone(),
            },
            root.clone(),
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

    #[tokio::test]
    async fn cloning_an_external_instance_is_refused() {
        // A whole-tree clone of an external instance would either copy an
        // empty directory or re-point the round-tripped external home at the
        // source — both are wrong. The backend refuses; the menu disables it.
        let root = temp_root("clone-external");
        std::fs::create_dir_all(root.join("instances")).unwrap();
        let src = root.join("instances").join("ext-src");
        std::fs::create_dir_all(&src).unwrap();
        let external_home = root.join("user-dsh");
        std::fs::create_dir_all(&external_home).unwrap();
        let mut src_manifest = manifest("ext-src", "External Source");
        src_manifest.management_mode = ManagementMode::External;
        src_manifest.external_home = Some(external_home.to_string_lossy().into_owned());
        manifest::write_manifest(&src, &src_manifest).await.unwrap();

        let err = run_clone(
            &Arc::new(AtomicBool::new(false)),
            &root,
            "ext-src",
            manifest("clone-target", "Clone"),
            &Channel::new(|_| Ok(())),
        )
        .await
        .unwrap_err();
        assert!(err.contains("原地接入"), "got: {err}");
        assert!(
            !root.join("instances").join("clone-target").exists(),
            "no half clone left behind"
        );
        let _ = std::fs::remove_dir_all(&root);
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
            management_mode: Default::default(),
            source: Default::default(),
            external_home: None,
            adopted_from: None,
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
    async fn plugins_declared_in_the_patch_without_a_marker_are_adopted() {
        let root = temp_root("scan-outofband");
        create_instance_inner(root.as_path(), manifest("oob-0001", "Oob"))
            .await
            .unwrap();
        let profile = root
            .join("instances")
            .join("oob-0001")
            .join("dsh-home")
            .join("profiles")
            .join("default");
        let modules = profile.join("node_modules");

        // What a hand fix leaves behind (pnpm into the profile + insert
        // blocks): package.json only, no PHL marker anywhere.
        std::fs::create_dir_all(modules.join("dshmarket")).unwrap();
        std::fs::write(
            modules.join("dshmarket").join("package.json"),
            r#"{"name":"dshmarket","version":"1.43.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(modules.join("dsh-dream-skin")).unwrap();
        std::fs::write(
            modules.join("dsh-dream-skin").join("package.json"),
            r#"{"name":"dsh-dream-skin"}"#,
        )
        .unwrap();
        // pnpm's store layout and transitive deps carry no marker and no patch
        // entry — adopting anything present in node_modules would flood the
        // list with hundreds of unrelated packages.
        std::fs::create_dir_all(modules.join("schemastery")).unwrap();
        std::fs::create_dir_all(modules.join("@acme/toolkit")).unwrap();
        std::fs::write(
            modules.join("@acme/toolkit/package.json"),
            r#"{"name":"@acme/toolkit","version":"2.0.0"}"#,
        )
        .unwrap();

        std::fs::write(
            profile.join("cordis.patch.yml"),
            "- insert:\n    - id: dshmarket\n      name: 'dshmarket'\n\
             - insert:\n    - id: dsh-dream-skin\n      name: 'dsh-dream-skin'\n      disabled: true\n    - name: '@acme/toolkit'\n      id: toolkit\n",
        )
        .unwrap();

        let plugins = scan_plugins(&profile).await;
        assert_eq!(
            plugins.len(),
            3,
            "patch-declared packages are adopted; unlisted dirs are not"
        );

        let market = plugins.iter().find(|p| p.plugin_id == "dshmarket").unwrap();
        assert!(
            market.enabled,
            "a mounted insert block with no flag is enabled"
        );
        assert_eq!(market.version, "1.43.0", "version read from package.json");
        assert_eq!(market.registry_id, "dshmarket");
        assert_eq!(market.trust, "unknown");

        let skin = plugins
            .iter()
            .find(|p| p.plugin_id == "dsh-dream-skin")
            .unwrap();
        assert!(!skin.enabled, "the block's own disabled flag is honored");
        assert_eq!(skin.version, "", "missing version degrades, never drops");
        let scoped = plugins
            .iter()
            .find(|p| p.registry_id == "@acme/toolkit")
            .unwrap();
        assert!(scoped.enabled, "a sibling's disabled flag must not leak");
        assert_eq!(scoped.version, "2.0.0");

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
        let root_for_policy = root.clone();
        let worker = std::thread::spawn(move || {
            let flag = AtomicBool::new(false);
            let mut done = 0u64;
            let links = LinkPolicy::Preserve {
                source_root: src.clone(),
                dest_root: dst.clone(),
            };
            let ctx = CopyCtx {
                flag: &flag,
                total: 512,
                tx: &tx,
                skip: SkipRule::RunStateAtRoot,
                links: &links,
                data_root: &root_for_policy,
            };
            copy_tree(&src, &dst, &mut done, &ctx).map(|()| done)
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
            management_mode: Default::default(),
            source: Default::default(),
            external_home: None,
            adopted_from: None,
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
    async fn an_interrupted_restore_is_rolled_back_instead_of_refused() {
        // The swap renames the live home aside before the restored copy is
        // placed. A crash in between leaves no home at all, and the next
        // restore used to refuse ("实例缺少 dsh-home") — stranding the only
        // copy under a hidden name (2026-09-09 review, P2).
        let root = temp_root("snap-interrupted");
        let record = create_instance_inner(root.as_path(), manifest("snap-int1", "Interrupted"))
            .await
            .unwrap();
        let dir = root.join("instances").join("snap-int1");
        let home = dir.join("dsh-home");
        std::fs::write(home.join("keep.txt"), "live").unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let processes = Processes::default();
        let snap = run_snapshot_create(&flag, &processes, root.as_path(), "snap-int1", &|_| {})
            .await
            .unwrap();
        assert_eq!(record.manifest.id, "snap-int1");

        // Simulate the crash: the live home is aside, staging exists, and
        // nothing has been placed back over it.
        std::fs::create_dir_all(dir.join(".phl-restore/dsh-home")).unwrap();
        std::fs::rename(&home, dir.join(".phl-old-dsh-home")).unwrap();
        assert!(!home.exists());

        let restored = restore_snapshot_inner(root.as_path(), "snap-int1", &snap.id, &processes)
            .await
            .expect("an interrupted restore must be repaired, not refused");
        assert!(home.exists(), "the instance gets its home back");
        assert!(
            !dir.join(".phl-old-dsh-home").exists(),
            "the backup is retired once the home is back"
        );
        assert!(
            !dir.join(".phl-restore").exists(),
            "the interrupted staging tree is removed"
        );
        assert!(restored.plugins.len() <= 1);

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

    /// Recursively fingerprint a tree as (relative path, bytes) pairs —
    /// the byte-level identity check the lifecycle chains assert against.
    fn tree_fingerprint(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    let rel = path
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    #[tokio::test]
    async fn clone_diverges_from_source_at_the_byte_level() {
        // The isolation promise, tested as one chain: clone a populated
        // source, mutate the clone, and the source must not move — while
        // reload, rename and delete all keep listing and disk consistent.
        let root = temp_root("clone-chain");
        create_instance_inner(root.as_path(), manifest("src-a1aa", "Source"))
            .await
            .unwrap();
        let src = root.join("instances").join("src-a1aa");
        std::fs::write(src.join("dsh-home/config-app.json"), b"original").unwrap();
        std::fs::create_dir_all(src.join("dsh-home/nested")).unwrap();
        std::fs::write(src.join("dsh-home/nested/deep.txt"), b"deep").unwrap();
        let before = tree_fingerprint(&src.join("dsh-home"));
        assert!(!before.is_empty(), "seed content exists to compare");

        let cloned = run_clone(
            &Arc::new(AtomicBool::new(false)),
            &root,
            "src-a1aa",
            manifest("cln-b2bb", "Clone"),
            &Channel::new(|_| Ok(())),
        )
        .await
        .unwrap();
        assert_eq!(cloned.manifest.id, "cln-b2bb");
        assert!(cloned.manifest.last_run_at.is_none());
        assert!(!cloned.manifest.favorite, "a clone starts un-favorited");
        let cln = root.join("instances").join("cln-b2bb");
        assert_eq!(
            tree_fingerprint(&cln.join("dsh-home")),
            before,
            "the clone begins as an exact copy of the home"
        );

        // Mutate the clone two ways: overwrite and extend.
        std::fs::write(cln.join("dsh-home/config-app.json"), b"diverged").unwrap();
        std::fs::write(cln.join("dsh-home/nested/clone-only.txt"), b"new").unwrap();
        assert_eq!(
            tree_fingerprint(&src.join("dsh-home")),
            before,
            "source home is byte-identical after the clone diverges"
        );

        // Reload: both instances persist independently; rename the clone.
        let listed = list_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(listed.len(), 2);
        let mut renamed = listed
            .iter()
            .find(|r| r.manifest.id == "cln-b2bb")
            .unwrap()
            .manifest
            .clone();
        renamed.name = "Renamed Clone".into();
        save_instance_inner(root.as_path(), renamed).await.unwrap();
        let listed = list_instances_inner(root.as_path()).await.unwrap();
        assert_eq!(
            listed
                .iter()
                .find(|r| r.manifest.id == "cln-b2bb")
                .unwrap()
                .manifest
                .name,
            "Renamed Clone"
        );
        assert_eq!(
            listed
                .iter()
                .find(|r| r.manifest.id == "src-a1aa")
                .unwrap()
                .manifest
                .name,
            "Source",
            "renaming the clone never touches the source"
        );

        // Delete the clone: the source tree and its bytes are untouched,
        // and no staging directory survived the lifecycle.
        delete_instance_inner(root.as_path(), "cln-b2bb", &Processes::default())
            .await
            .unwrap();
        assert!(!cln.exists());
        assert_eq!(tree_fingerprint(&src.join("dsh-home")), before);
        assert_eq!(list_instances_inner(root.as_path()).await.unwrap().len(), 1);
        assert!(
            !std::fs::read_dir(root.join("instances"))
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".phl-new-")),
            "no clone staging residue"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn clone_and_snapshot_survive_a_managed_node_modules_link() {
        // Defect #4 regression on the production paths. A dependency install
        // leaves `<dsh-home>/profiles/<profile>/node_modules` as ~1000
        // junctions into the shared `versions/` tree; the old blanket
        // "refuse every link" rule made every such instance uncloneable and
        // unsnapshottable, with a message the user could not act on.
        let root = temp_root("clone-links");
        create_instance_inner(root.as_path(), manifest("src-a1aa", "Source"))
            .await
            .unwrap();
        let src = root.join("instances").join("src-a1aa");
        let home = src.join("dsh-home");

        let versions_nm = root.join("versions").join("v1").join("node_modules");
        std::fs::create_dir_all(versions_nm.join("dep")).unwrap();
        std::fs::write(versions_nm.join("dep").join("index.js"), b"module").unwrap();
        let link = home.join("profiles").join("default").join("node_modules");
        // The instance skeleton already created `node_modules` as a real
        // directory; `install-deps` is what replaces it with junctions.
        let _ = std::fs::remove_dir_all(&link);
        copy::recreate_link(&link, &versions_nm, true).unwrap();

        // pnpm's plugin layout, measured on Windows with pnpm 9: a dependency
        // is a junction into the home's *own* `.pnpm` — inside the instance,
        // not the shared version tree. Refusing it made every instance with a
        // dependency-bearing plugin unclonable, the same defect one level down.
        let plugin_nm = home
            .join("profiles")
            .join("default")
            .join("plugins")
            .join("node_modules");
        let store = plugin_nm
            .join(".pnpm")
            .join("dep@1.0.0")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("index.js"), b"pnpm-dep").unwrap();
        let internal = plugin_nm.join("pnpm-dep");
        copy::recreate_link(&internal, &store, true).unwrap();

        let cloned = run_clone(
            &Arc::new(AtomicBool::new(false)),
            &root,
            "src-a1aa",
            manifest("cln-b2bb", "Clone"),
            &Channel::new(|_| Ok(())),
        )
        .await
        .expect("an instance with managed links must be cloneable");
        assert_eq!(cloned.manifest.id, "cln-b2bb");
        let cloned_link = root
            .join("instances")
            .join("cln-b2bb")
            .join("dsh-home")
            .join("profiles")
            .join("default")
            .join("node_modules");
        assert!(
            std::fs::symlink_metadata(&cloned_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the clone keeps a link, it does not materialize 282 MB"
        );
        assert!(
            cloned_link.join("dep").join("index.js").exists(),
            "the recreated link still resolves"
        );

        // The in-tree link follows the copy instead of pointing back at the
        // source instance — a clone that referenced the original's `.pnpm`
        // would break the moment the source is deleted or edited.
        let cloned_internal = root
            .join("instances")
            .join("cln-b2bb")
            .join("dsh-home")
            .join("profiles")
            .join("default")
            .join("plugins")
            .join("node_modules")
            .join("pnpm-dep");
        assert!(
            std::fs::symlink_metadata(&cloned_internal)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the pnpm-style link survives as a link"
        );
        let clone_home =
            std::fs::canonicalize(root.join("instances").join("cln-b2bb").join("dsh-home"))
                .unwrap();
        let target = std::fs::canonicalize(&cloned_internal).unwrap();
        assert!(
            target.starts_with(&clone_home),
            "the in-tree link must point inside the clone, not at the source: {}",
            target.display()
        );
        assert_eq!(
            std::fs::read(cloned_internal.join("index.js")).unwrap(),
            b"pnpm-dep"
        );

        // Snapshot create walks the same engine over the same home.
        let snap = snapshot::run_snapshot_create(
            &Arc::new(AtomicBool::new(false)),
            &Processes::default(),
            &root,
            "src-a1aa",
            &|_| {},
        )
        .await
        .expect("an instance with managed links must be snapshottable");
        let snap_link = src
            .join("snapshots")
            .join(&snap.id)
            .join("dsh-home")
            .join("profiles")
            .join("default")
            .join("node_modules");
        assert!(
            std::fs::symlink_metadata(&snap_link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the snapshot keeps the link too"
        );

        // Restore copies the link-bearing snapshot back over the live home:
        // the same engine, the same guarantee — the tree comes back wired.
        std::fs::write(
            home.join("profiles").join("default").join("marker.txt"),
            b"x",
        )
        .unwrap();
        snapshot::restore_snapshot_inner(&root, "src-a1aa", &snap.id, &Processes::default())
            .await
            .expect("restoring a link-bearing snapshot must succeed");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "restore recreates the link rather than refusing or materializing it"
        );
        assert!(
            link.join("dep").join("index.js").exists(),
            "the restored home resolves through the recreated link"
        );
        // The pnpm-style link is rebuilt against the live home, not against
        // the snapshot directory it was copied from.
        assert!(
            std::fs::symlink_metadata(&internal)
                .unwrap()
                .file_type()
                .is_symlink(),
            "restore keeps the pnpm-style link"
        );
        assert_eq!(
            std::fs::read(internal.join("index.js")).unwrap(),
            b"pnpm-dep",
            "and it resolves inside the restored home"
        );
        assert!(
            !home
                .join("profiles")
                .join("default")
                .join("marker.txt")
                .exists(),
            "post-snapshot changes are gone, as restore promises"
        );

        // The source link is untouched: nothing rewrote or replaced it.
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn deleting_an_instance_never_reaches_through_its_links() {
        // The blast-radius test for the junction era: a real instance's tree
        // is hundreds of links into the shared `versions/` install. If a
        // recursive delete followed them, deleting one instance would erase
        // the version every other instance boots from. The desktop alpha
        // pass's cleanup proved the question is live (MSYS `rm -rf` does
        // follow junctions) — PHL's delete must not.
        let root = temp_root("delete-blast");
        let versions_dep = root
            .join("versions")
            .join("v1")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&versions_dep).unwrap();
        std::fs::write(versions_dep.join("index.js"), b"shared-gold").unwrap();
        create_instance_inner(root.as_path(), manifest("dlt-a1aa", "Doomed"))
            .await
            .unwrap();
        let nm = root
            .join("instances")
            .join("dlt-a1aa")
            .join("dsh-home")
            .join("profiles")
            .join("default")
            .join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        copy::recreate_link(&nm.join("dep"), &versions_dep, true).unwrap();

        delete_instance_inner(root.as_path(), "dlt-a1aa", &Processes::default())
            .await
            .unwrap();

        assert!(!root.join("instances").join("dlt-a1aa").exists());
        assert_eq!(
            std::fs::read(versions_dep.join("index.js")).unwrap(),
            b"shared-gold",
            "the shared version content SURVIVED the instance delete"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn snapshot_operations_refuse_a_running_instance() {
        // create → restore → delete are all mutating or reading a live tree;
        // the same `ensure_not_running` gate must hold every entry, and
        // releasing the process must re-open them (no sticky refusal).
        let root = temp_root("snap-running");
        create_instance_inner(root.as_path(), manifest("run-a1aa", "Runner"))
            .await
            .unwrap();
        let dir = root.join("instances").join("run-a1aa");
        std::fs::write(dir.join("dsh-home/state.txt"), b"a").unwrap();

        let processes = Processes::default();
        let snap = run_snapshot_create(
            &Arc::new(AtomicBool::new(false)),
            &processes,
            root.as_path(),
            "run-a1aa",
            &|_| {},
        )
        .await
        .unwrap();

        processes.set(
            "run-a1aa",
            crate::launch::ProcessEntry {
                pid: 777,
                port: 3080,
            },
        );
        let err = run_snapshot_create(
            &Arc::new(AtomicBool::new(false)),
            &processes,
            root.as_path(),
            "run-a1aa",
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("正在运行"), "got: {err}");
        assert!(
            restore_snapshot_inner(root.as_path(), "run-a1aa", &snap.id, &processes)
                .await
                .unwrap_err()
                .contains("正在运行")
        );
        assert!(
            delete_snapshot_inner(root.as_path(), "run-a1aa", &snap.id, &processes)
                .await
                .unwrap_err()
                .contains("正在运行")
        );
        assert_eq!(
            scan_snapshots(&dir).await.len(),
            1,
            "refused operations changed nothing, and left no staging behind"
        );
        assert_eq!(
            std::fs::read(dir.join("dsh-home/state.txt")).unwrap(),
            b"a",
            "the live tree is untouched by refused restore"
        );

        processes.remove_if_pid("run-a1aa", 777);
        run_snapshot_create(
            &Arc::new(AtomicBool::new(false)),
            &processes,
            root.as_path(),
            "run-a1aa",
            &|_| {},
        )
        .await
        .expect("operations resume once the process is gone");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn external_instances_survive_every_gated_write_attempt_untouched() {
        // Spec §3.1: an in-place (external) DSH_HOME belongs to the user.
        // Every mutation command must refuse it AND leave zero side effects
        // — the byte-identity of the home tree after the attempts is the
        // invariant under test, not merely the error strings.
        // `writable_profile_dir` is the single gate the plugin install /
        // enable / disable / uninstall commands pass through first; the
        // snapshot restore path checks inline. One test per gate point.
        let root = temp_root("ext-write");
        std::fs::create_dir_all(root.join("instances")).unwrap();
        let user_home = root.join("someone-elses-dsh");
        std::fs::create_dir_all(user_home.join("sessions")).unwrap();
        std::fs::write(user_home.join("settings.yaml"), b"provider: mine\n").unwrap();
        std::fs::write(user_home.join("sessions/s1.jsonl"), b"{\"a\":1}\n").unwrap();

        let dir = root.join("instances").join("ext-w1aa");
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = manifest("ext-w1aa", "External");
        m.management_mode = ManagementMode::External;
        m.external_home = Some(user_home.to_string_lossy().into_owned());
        manifest::write_manifest(&dir, &m).await.unwrap();

        let home_before = tree_fingerprint(&user_home);
        let inst_before = tree_fingerprint(&dir);

        // Plugin-write gate: refuses before the profile path is even returned.
        let err = writable_profile_dir(root.as_path(), "ext-w1aa", "安装插件到")
            .await
            .unwrap_err();
        assert!(
            err.contains("原地接入") && err.contains("安装插件到"),
            "got: {err}"
        );

        // Snapshot restore: refuses an external instance outright.
        let err = restore_snapshot_inner(
            root.as_path(),
            "ext-w1aa",
            "snap-anything",
            &Processes::default(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("原地接入"), "got: {err}");

        // The user's directory never changed, and neither did the instance.
        assert_eq!(
            tree_fingerprint(&user_home),
            home_before,
            "external home byte-identical"
        );
        assert_eq!(
            tree_fingerprint(&dir),
            inst_before,
            "instance tree byte-identical"
        );

        // The gate keys off management_mode only: a sandbox instance with the
        // same shape passes, so the refusal is not an artifact of the fixture.
        let mut m2 = m.clone();
        m2.management_mode = ManagementMode::default();
        assert!(reject_external_write(&m2, "ext-w1aa", "测试").is_ok());
        assert!(reject_external_write(&m, "ext-w1aa", "测试").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
