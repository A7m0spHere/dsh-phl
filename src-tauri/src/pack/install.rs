//! `.phlpack` install (development spec part 17-21).
//!
//! Hard rule: **a pack is validated (P1-2) before a single byte is written**.
//! `install_inner` re-opens and re-validates the archive, resolves its
//! dependencies, then builds a fresh staging DSH_HOME under
//! `instances/.phl-pack-<id>/` and publishes with one rename. Any failure
//! deletes the staging dir — "失败：删除 staging，不要留下半个实例" (spec §21).
//!
//! What a commit materialises:
//! - embedded plugins → the instance profile's `node_modules/` (already
//!   integrity-checked by the validator);
//! - sessions → the instance's `dsh-home/sessions/` — a pack that opted into
//!   history (spec §11) lands it whole, which is exactly what "启动 → DSH 中可见
//!   历史对话" (spec §11.2) needs;
//! - overrides → the instance `dsh-home/` (forward slot, empty for PHL packs);
//! - the environment (an embedded `InstanceBundle`) → the manifest's config /
//!   args, with credential values re-stripped on import (spec §12).
//!
//! Remote (registry) plugins are deliberately *not* auto-downloaded inside this
//! transactional commit — plugin install is its own resumable, progress-
//! reporting pipeline. They come back as `deferredPlugins` for the UI to run
//! through the normal `installPlugin` command right after the instance exists,
//! which keeps "PHL 始终是配置源" and never entangles two transactions.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use super::{PackPlugin, PluginSource, ValidatedPack};
use crate::api_config::credential_env_names;
use crate::errors;
use crate::instances::bundle::InstanceBundle;
use crate::instances::env_policy::partition_imported_env;
use crate::instances::manifest::write_manifest;
use crate::instances::{
    build_record, instance_dir, instances_root, AdoptedFrom, CloneProgress, InstanceManifest,
    InstanceRecord, InstanceSource, ManagementMode,
};
use crate::paths::{sanitize_segment, PhlState};
use crate::plugins::install::commit_install;
use crate::plugins::resolve::sanitize_pkg_path;
use crate::versions::{now_iso, Transfers};
use std::collections::HashMap;
use std::path::PathBuf;

/// A resolver verdict per dependency (spec §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyStatus {
    /// Present and ready (version/runtime dir carries its install marker).
    Installed,
    /// Fetchable through the normal pipeline; install reports it to the user.
    Downloadable,
    /// Carried inside the pack (embedded plugin) — always resolvable.
    Embedded,
    /// Named but unavailable (no registry id, not bundled) and required.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    /// `"dsh"` | `"runtime"` | `"plugin"`.
    pub kind: String,
    pub id: String,
    pub version: String,
    pub status: DependencyStatus,
    pub required: bool,
    /// Human note for the missing/downloadable UX (spec §19-20).
    pub note: Option<String>,
}

/// What the install dialog shows before committing (spec §17.1).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackPreview {
    pub pack_id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub icon: Option<String>,
    pub dsh_version: String,
    pub runtime: String,
    pub plugin_count: usize,
    pub embedded_plugin_count: usize,
    pub sessions_included: bool,
    pub session_count: usize,
    pub secrets_excluded: bool,
    pub dependencies: Vec<Dependency>,
    pub warnings: Vec<String>,
    /// A required dependency is missing → install is blocked until resolved.
    pub blocked: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackInstallRequest {
    /// Identity + naming the user owns (id, name, hue, port, autoPort). The
    /// environment (version/runtime/profile/env/args) is taken from the pack.
    pub manifest: InstanceManifest,
    /// Proceed despite a non-fatal missing dependency.
    #[serde(default)]
    pub allow_missing: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackInstallOutcome {
    pub record: InstanceRecord,
    /// registry plugin ids to re-install through the normal plugin pipeline.
    pub deferred_plugins: Vec<String>,
    pub sessions_imported: usize,
    pub credential_names: Vec<String>,
}

async fn version_installed(root: &Path, name: &str) -> bool {
    let bare = name.strip_prefix("dsh-").unwrap_or(name);
    tokio::fs::read_to_string(root.join("versions").join(bare).join("phl-install.json"))
        .await
        .is_ok()
}

async fn runtime_installed(root: &Path, name: &str) -> bool {
    if name == "node-system" {
        return true; // PATH node — nothing on disk to check
    }
    tokio::fs::read_to_string(root.join("runtimes").join(name).join("phl-runtime.json"))
        .await
        .is_ok()
}

/// Resolve a validated pack's dependencies against the machine's current
/// state. Read-only; the source `path` is needed again by the commit's reopen.
pub(crate) async fn resolve_dependencies(
    root: &Path,
    pack: &ValidatedPack,
) -> (Vec<Dependency>, bool) {
    let mut deps = Vec::new();
    let mut blocked = false;

    let dsh = &pack.manifest.dsh.version;
    let (st, note) = if version_installed(root, dsh).await {
        (DependencyStatus::Installed, None)
    } else {
        (
            DependencyStatus::Downloadable,
            Some(format!("DSH {dsh} 未安装，安装后可通过版本页获取")),
        )
    };
    deps.push(Dependency {
        kind: "dsh".into(),
        id: dsh.clone(),
        version: dsh.clone(),
        status: st,
        required: true,
        note,
    });

    let runtime = pack
        .manifest
        .runtime
        .node_version
        .clone()
        .unwrap_or_else(|| "node".into());
    let (rt_st, rt_note) = if runtime_installed(root, &runtime).await {
        (DependencyStatus::Installed, None)
    } else {
        (
            DependencyStatus::Downloadable,
            Some(format!("{runtime} 未安装，安装后可通过 Runtime 页获取")),
        )
    };
    deps.push(Dependency {
        kind: "runtime".into(),
        id: runtime.clone(),
        version: runtime.clone(),
        status: rt_st,
        required: true,
        note: rt_note,
    });

    for p in &pack.manifest.plugins {
        let (status, note) = match &p.source {
            PluginSource::Embedded { .. } => (DependencyStatus::Embedded, None),
            PluginSource::Registry { registry_id } => {
                if registry_id.trim().is_empty() {
                    (
                        DependencyStatus::Missing,
                        Some(format!("插件 {} 未声明 registryId 且未内置", p.id)),
                    )
                } else {
                    (DependencyStatus::Downloadable, None)
                }
            }
        };
        if status == DependencyStatus::Missing && p.required {
            blocked = true;
        }
        deps.push(Dependency {
            kind: "plugin".into(),
            id: p.id.clone(),
            version: p.version.clone(),
            status,
            required: p.required,
            note,
        });
    }
    (deps, blocked)
}

#[tauri::command]
pub async fn preview_pack(state: State<'_, PhlState>, path: String) -> Result<PackPreview, String> {
    let root = state.root();
    let pack = super::read_pack_from_path(Path::new(&path)).map_err(super::pack_error)?;
    let (deps, blocked) = resolve_dependencies(&root, &pack).await;
    let mut warnings: Vec<String> = deps.iter().filter_map(|d| d.note.clone()).collect();
    if pack.manifest.content.sessions_included {
        warnings
            .push("整合包包含历史对话，安装前请确认来源可信（可能含他人输入与敏感信息）".into());
    }
    Ok(PackPreview {
        pack_id: pack.manifest.pack.id.clone(),
        name: pack.manifest.pack.name.clone(),
        version: pack.manifest.pack.version.clone(),
        author: pack.manifest.pack.author.clone(),
        description: pack.manifest.pack.description.clone(),
        icon: pack.manifest.pack.icon.clone(),
        dsh_version: pack.manifest.dsh.version.clone(),
        runtime: pack
            .manifest
            .runtime
            .node_version
            .clone()
            .unwrap_or_else(|| "node".into()),
        plugin_count: pack.manifest.plugins.len(),
        embedded_plugin_count: pack.embedded_plugins.len(),
        sessions_included: pack.manifest.content.sessions_included,
        session_count: pack.manifest.content.session_count.unwrap_or(0),
        secrets_excluded: pack.manifest.content.secrets_excluded,
        dependencies: deps,
        warnings,
        blocked,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_pack(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    transfer_id: String,
    path: String,
    req: PackInstallRequest,
    on_progress: Channel<CloneProgress>,
) -> Result<PackInstallOutcome, String> {
    let id = req.manifest.id.clone();
    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "pack-install",
        format!("安装整合包 → 实例 {id}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        move |task| async move {
            let r = install_inner(&state.root(), task, &path, req, &on_progress, flag).await;
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

async fn install_inner(
    root: &Path,
    task: crate::resources::Task,
    path: &str,
    req: PackInstallRequest,
    on_progress: &Channel<CloneProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<PackInstallOutcome, String> {
    // Re-validate at commit time: the bytes on disk are attacker-supplied and
    // may have changed since the preview (TOCTOU on the archive is closed by
    // re-running the whole P1-2 validation here).
    let is_cancelled = || cancel.load(Ordering::SeqCst);
    let pack = super::read_pack_from_path_with_cancel(Path::new(path), &is_cancelled)
        .map_err(super::pack_error)?;
    let (_deps, blocked) = resolve_dependencies(root, &pack).await;
    if blocked && !req.allow_missing {
        let missing: Vec<String> = pack
            .manifest
            .plugins
            .iter()
            .filter(|p| p.required && matches!(&p.source, PluginSource::Registry { registry_id } if registry_id.trim().is_empty()))
            .map(|p| p.id.clone())
            .collect();
        return Err(errors::coded(
            errors::ErrCode::State,
            format!(
                "必需的整合包依赖缺失，安装已阻止：{}（可选择跳过或先在实例内手动安装）",
                missing.join("、")
            ),
        ));
    }

    let mut manifest = req.manifest;
    let id = sanitize_segment(&manifest.id, "实例 id")?;
    manifest.id = id.clone();
    // Environment is the pack's, not the (possibly blank) request's.
    manifest.version_id = pack.manifest.dsh.version.clone();
    manifest.runtime_id = pack
        .manifest
        .runtime
        .node_version
        .clone()
        .unwrap_or_else(|| "node".into());
    manifest.profile = sanitize_segment("web", "profile 名")?;
    manifest.created_at = if manifest.created_at.trim().is_empty() {
        now_iso()
    } else {
        manifest.created_at.clone()
    };
    manifest.management_mode = ManagementMode::PackInstalled;
    manifest.source = InstanceSource::Phlpack;
    manifest.external_home = None;
    manifest.adopted_from = Some(AdoptedFrom {
        dsh_home: format!("pack:{}", pack.manifest.pack.id),
        detected_version: Some(pack.manifest.dsh.version.clone()),
        adopted_at: now_iso(),
        mode: "pack-installed".into(),
    });

    let dir = instance_dir(root, &id)?;
    if dir.exists() {
        return Err(format!("实例目录已存在: {id}"));
    }
    let staging = instances_root(root).join(format!(".phl-pack-{id}"));
    let _ = std::fs::remove_dir_all(&staging);

    let (sessions_imported, credential_names) = {
        let built = async {
            task.set_phase("创建实例目录");
            let home = staging.join("dsh-home");
            let profile = home.join("profiles").join("web");
            std::fs::create_dir_all(profile.join("node_modules"))
                .map_err(|e| format!("创建实例目录失败: {e}"))?;
            for sub in ["workspace", "logs"] {
                std::fs::create_dir_all(staging.join(sub))
                    .map_err(|e| format!("创建 {sub} 失败: {e}"))?;
            }

            task.set_phase("解包整合包");
            // §R7: decompression + writing is the heaviest synchronous phase of
            // an install, so it runs on the blocking pool — the async worker
            // (and other instances' transfers) stay responsive during a big
            // unpack. Cancellation is re-checked at the phase boundary right
            // after, before any further staging work, so a cancel observed
            // during the unpack aborts into the existing staging cleanup.
            let extracted = {
                let pack_path = path.to_string();
                let profile_c = profile.clone();
                let home_c = home.clone();
                let unpack_cancel = cancel.clone();
                tokio::task::spawn_blocking(move || {
                    unpack_pack_with_cancel(&pack_path, &profile_c, &home_c, &|| {
                        unpack_cancel.load(Ordering::SeqCst)
                    })
                })
                .await
                .map_err(|e| format!("解包任务调度失败: {e}"))??
            };
            if task.cancel_observed() {
                return Err("cancelled".into());
            }

            // §R3: unpack only dropped the embedded plugins' files into
            // `node_modules/`; they are not "installed" until each carries a
            // PHL marker and a Cordis registration. Registering inside STAGING
            // (before the manifest write + rename) means a failure here aborts
            // into the existing `remove_dir_all(&staging)` cleanup — no half-
            // committed instance ever lands.
            task.set_phase("登记内置插件");
            register_embedded_plugins(&pack.manifest.plugins, &profile).await?;

            task.set_phase("应用环境");
            let mut credential_names = Vec::new();
            if let Some(env_section) = &pack.manifest.environment {
                if let Ok(bundle) = serde_json::from_value::<InstanceBundle>(env_section.0.clone())
                {
                    let credential_envs = credential_env_names(root).await;
                    let (env, creds) =
                        partition_imported_env(bundle.instance.env.clone(), &credential_envs);
                    credential_names = creds;
                    manifest.env = env;
                    manifest.args = bundle.instance.args.clone();
                }
            }

            task.set_phase("写清单");
            write_manifest(&staging, &manifest).await?;
            if task.cancel_observed() {
                return Err("cancelled".into());
            }
            std::fs::rename(&staging, &dir).map_err(|e| format!("放置实例目录失败: {e}"))?;
            Ok::<_, String>((extracted.sessions, credential_names))
        }
        .await;
        match built {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                let _ = on_progress;
                return Err(e);
            }
        }
    };

    let deferred: Vec<String> = pack
        .manifest
        .plugins
        .iter()
        .filter(|p| matches!(&p.source, PluginSource::Registry { registry_id } if !registry_id.trim().is_empty()))
        .map(|p| p.id.clone())
        .collect();

    Ok(PackInstallOutcome {
        record: build_record(&dir, manifest).await,
        deferred_plugins: deferred,
        sessions_imported,
        credential_names,
    })
}

struct Extracted {
    sessions: usize,
}

/// Extract a pack's file payloads into a fresh staging DSH_HOME, mapped onto
/// the instance's layout (embedded plugins → the profile store, sessions →
/// the home's session root, overrides → the home). The core extractor owns
/// the "every written byte stays inside its root" rule (spec §22); this
/// function only decides *where* each section lands. The archive is re-opened
/// and re-validated by the caller before this runs, so entry names are
/// already proven traversal-/symlink-free.
#[cfg(test)]
fn unpack_pack(path: &str, profile: &Path, home: &Path) -> Result<Extracted, String> {
    unpack_pack_with_cancel(path, profile, home, &|| false)
}

fn unpack_pack_with_cancel<F: Fn() -> bool + ?Sized>(
    path: &str,
    profile: &Path,
    home: &Path,
    cancel: &F,
) -> Result<Extracted, String> {
    let plugins_root = profile.join("node_modules");
    let sessions_root = home.join("sessions");
    let overrides_root = home.to_path_buf();
    let written = super::unpack::unpack_entries_to_with_cancel(
        Path::new(path),
        |rel| {
            // `rel` is the core-normalised (forward-slash, traversal-free)
            // relative entry name. Anything outside the three payload sections
            // (`phlpack.json`, `assets/…`) is metadata — mapped to nothing.
            if let Ok(rest) = rel.strip_prefix("embedded/plugins") {
                return Some((plugins_root.join(rest), plugins_root.clone()));
            }
            if let Ok(rest) = rel.strip_prefix("sessions") {
                return Some((sessions_root.join(rest), sessions_root.clone()));
            }
            if let Ok(rest) = rel.strip_prefix("overrides") {
                return Some((overrides_root.join(rest), overrides_root.clone()));
            }
            None
        },
        cancel,
    )
    .map_err(super::pack_error)?;
    let sessions = written
        .iter()
        .filter(|f| f.archive_name.starts_with("sessions/"))
        .filter(|f| f.archive_name.ends_with(".jsonl") || f.archive_name.ends_with(".jsonl.zstd"))
        .count();
    Ok(Extracted { sessions })
}

/// §R3 — turn the embedded plugins that [`unpack_pack`] dropped into
/// `node_modules/` into fully registered installs, reusing the plugin
/// installer's own [`commit_install`] (marker write + Cordis registration +
/// enable), so there is exactly one install protocol in the codebase.
///
/// Identity is taken from **what actually landed on disk here**, not from the
/// pack's claims or a carried-over marker: a CLI-built pack ships only a
/// `package.json` + sources (no PHL marker at all), and a desktop export may
/// carry a *stale* `phl-plugin.json` / trust flag from the source machine —
/// neither is trusted blindly (spec §12 honesty). The folder's `package.json`
/// `name` gives the registry id (correct for `@scope/name` packages, whose
/// unpacked folder is two path segments), and the manifest's `id` is recorded
/// as `pluginId` (the catalog id the instance plugin list is keyed by). Trust
/// reflects this import's facts: an embedded plugin came inside a pack from a
/// peer machine, with nothing registry-pinned to re-verify against, so it is
/// honestly `unverified`.
///
/// Returns the number of plugins registered. A declared embedded payload that
/// has no `package.json`, a missing name, an illegal package name, or an
/// install folder outside the store is a hard error — the staging caller then
/// rolls back rather than committing an instance whose plugins DSH would not
/// load. No enable/disable state is carried inside a pack, so plugins land
/// **enabled** (the documented default of `commit_install`), not a fabricated
/// "was it on before" guess.
async fn register_embedded_plugins(
    plugins: &[PackPlugin],
    profile: &Path,
) -> Result<usize, String> {
    let plugins_root = profile.join("node_modules");
    // Two manifest ids can share one unpacked folder; register each folder once.
    let mut folder_ids: HashMap<PathBuf, String> = HashMap::new();
    for p in plugins {
        let PluginSource::Embedded { path } = &p.source else {
            continue;
        };
        // The pack dir is `embedded/plugins/<folder>`; [`unpack_pack`] maps that
        // to `node_modules/<folder>`. Skip any path not under that section.
        let Some(folder) = path.strip_prefix("embedded/plugins/") else {
            continue;
        };
        let joined = plugins_root.join(folder.replace('/', std::path::MAIN_SEPARATOR_STR));
        // Defence in depth: the target must still sit inside the store. The core
        // validator already forbids traversal, but a mis-mapped name must not be
        // able to write a marker elsewhere.
        let dest = match joined.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                return Err(errors::coded(
                    errors::ErrCode::NotFound,
                    format!("整合包声明的内置插件目录未随包解出：{path}"),
                ))
            }
        };
        let store_root = plugins_root
            .canonicalize()
            .map_err(|e| format!("无法定位插件目录 {plugins_root:?}: {e}"))?;
        if !dest.starts_with(&store_root) {
            return Err(errors::coded(
                errors::ErrCode::State,
                format!("内置插件安装目录越界，已拒绝登记：{path}"),
            ));
        }
        folder_ids.entry(dest).or_insert_with(|| p.id.clone());
    }

    let mut count = 0;
    for (dir, plugin_id) in folder_ids {
        let raw = tokio::fs::read_to_string(dir.join("package.json"))
            .await
            .map_err(|e| {
                errors::coded(
                    errors::ErrCode::NotFound,
                    format!("内置插件缺少 package.json，无法登记：{dir:?}（{e}）"),
                )
            })?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("解析内置插件 package.json 失败: {e}"))?;
        let name = value
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("内置插件 package.json 缺少 name：{dir:?}"))?;
        let registry_id = sanitize_pkg_path(name)
            .map_err(|e| format!("内置插件包名 {name:?} 非法，无法登记：{e}"))?;
        let version = value
            .get("version")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("")
            .to_string();
        let marker = serde_json::json!({
            "installedAt": now_iso(),
            "pluginId": plugin_id,
            "version": version,
            "kind": "embedded",
            "registryId": registry_id,
            "trust": "unverified",
            "source": { "type": "pack-embedded" },
        });
        commit_install(&dir, &marker, profile, &registry_id)
            .await
            .map_err(|e| format!("登记内置插件 {registry_id} 失败：{e}"))?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
