//! PHL → `.phlpack` export (development spec part 6).
//!
//! The exporter is the inverse of P1-4's installer: it turns a managed instance
//! into a validated, self-contained pack. The rules the spec pins down:
//!
//! - **Remote plugins** travel as registry records only and re-download at
//!   install (spec §8.1); **embedded plugins** are packed under
//!   `embedded/plugins/<id>/` (spec §8.2). The user *chooses* the embed set in
//!   the UI (spec §9: "默认询问用户"), because PHL cannot decide redistribution
//!   rights — so the plan classifies each plugin and surfaces its license
//!   (spec §10) but does not auto-embed on our own judgement alone.
//! - **Sessions are excluded by default** (spec §11); including them requires an
//!   explicit acknowledgement, and the plan carries the count so the UI can show
//!   the privacy warning (spec §33) before the choice is honoured.
//! - **Secrets never ship**: the environment section is the same
//!   credential-stripped `InstanceBundle` the Bundle exporter produces, so an
//!   exported pack is no more leaky than an exported bundle (spec §12).
//! - Every payload file is integrity-hashed by the builder, and the finished pack
//!   is re-opened through the validator so we never emit something our own
//!   installer would refuse.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::format::PluginSource;
use super::write::PackBuilder;
use super::{EnvironmentSection, PhlPackManifest, ValidatedPack};
use crate::api_config::credential_env_names;
use crate::errors;
use crate::instances::{
    bundle::InstanceBundle, env_policy::partition_env, home_of, instance_dir, load_manifest,
    profile_root_of, scan_plugins, InstanceManifest,
};
use crate::paths::PhlState;
use crate::versions::{now_iso, Transfers};

/// One plugin as the export plan presents it: everything the UI needs to let
/// the user decide remote-vs-embedded, and to warn about redistribution.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPluginPlan {
    pub plugin_id: String,
    pub version: String,
    pub registry_id: String,
    /// License text from the plugin's package.json / LICENSE, when readable.
    pub license: Option<String>,
    /// True when a license could not be read at all → the UI must show the
    /// "PHL 无法确认该插件是否允许重新分发" note (spec §10).
    pub license_unknown: bool,
    /// A plugin already re-downloadable from a registry (has a registryId).
    /// Even these the user may choose to embed (offline transfer), so the UI
    /// exposes the toggle; the default embed set is the non-registry ones.
    pub registry_available: bool,
    /// Suggested default: embed when there is no registry id to re-download
    /// from (a local/unverified plugin).
    pub embed_recommended: bool,
}

/// What `preview_instance_pack_export` returns — the plan the UI confirms.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackExportPlan {
    pub instance_id: String,
    pub name: String,
    pub version_id: String,
    pub runtime_id: String,
    pub profile: String,
    pub plugins: Vec<ExportPluginPlan>,
    pub session_count: usize,
    /// Names stripped from the environment (re-configured after install).
    pub credential_names: Vec<String>,
    /// Machine-local env names dropped from the environment.
    pub machine_only: Vec<String>,
    /// Rough packed size (plugins to embed + optional sessions), informational.
    pub estimated_bytes: u64,
    /// §12: secret-shaped files found in plugin dirs / the session tree at
    /// preview time (`.env`, `id_rsa`, `token.json`, …). These are withheld
    /// from the archive by the core tree-walker regardless of the embed
    /// choice — the UI shows them so "why is my plugin misconfigured after
    /// install" is answered before export, not after.
    pub secret_files: Vec<String>,
    pub warnings: Vec<String>,
}

type PayloadScan = (u64, Vec<String>, Vec<String>);

/// The export command's result (mirrors BundleExportReport's shape + the pack
/// path so the UI can reveal it).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackExportReport {
    pub pack_path: String,
    pub plugin_count: usize,
    pub embedded_count: usize,
    pub sessions_included: bool,
    pub credential_names: Vec<String>,
    /// §12: secret-*shaped files* (a `.env` living inside an embedded plugin
    /// directory, an SSH key inside the copied session tree, …) that the core
    /// tree-walker withheld from the archive. `credential_names` covers the
    /// env-map strip; this list covers the file layer, so a plugin shipping a
    /// `.env` never rides out under a `secretsExcluded: true` promise — and
    /// the user learns exactly what was dropped rather than discovering a
    /// half-configured plugin at the far end.
    pub secret_files_withheld: Vec<String>,
    /// Filesystem links the tree-walker refused to pack. A pack carries bytes,
    /// never machine-local paths (a junction into `versions/` is meaningless on
    /// another machine), so links are skipped by design — but the user learns
    /// how many and which, exactly like the §12 secret withholdings.
    pub links_skipped: Vec<String>,
}

/// The user's confirmed export selection.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackExportOptions {
    /// registryIds the user chose to embed (carry their files in the pack).
    #[serde(default)]
    pub embed_registry_ids: Vec<String>,
    /// Include session history (spec §11: off unless the user opts in + acks).
    #[serde(default)]
    pub include_sessions: bool,
    /// The privacy warning was acknowledged; required when include_sessions.
    #[serde(default)]
    pub sessions_privacy_ack: bool,
}

pub(crate) struct ResolvedInstance {
    pub manifest: InstanceManifest,
    home: std::path::PathBuf,
    profile_root: std::path::PathBuf,
}

async fn resolve(root: &Path, id: &str) -> Result<ResolvedInstance, String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
    // The pack format requires a `dsh.version`; an unbound instance (empty or
    // path-unsafe binding) has no version to promise the recipient. Fail with
    // the actionable next step rather than an invalid archive the core
    // validator would reject later with a stranger message.
    if bare_pack_version(&manifest.version_id).is_none() {
        return Err(errors::coded(
            errors::ErrCode::State,
            "实例尚未绑定 DSH 版本，无法导出整合包；请先在实例详情里选择版本",
        ));
    }
    let home = home_of(&dir, &manifest);
    let profile_root = profile_root_of(&dir, &manifest);
    Ok(ResolvedInstance {
        manifest,
        home,
        profile_root,
    })
}

/// The pack format's `dsh.version` carries a **bare** version string ("0.1.2"),
/// while an instance binds the canonical `dsh-<ver>` id. Strip the prefix so
/// every display of the dependency (preview rows, warnings) reads like a
/// version. `None` for an empty or path-unsafe binding.
fn bare_pack_version(version_id: &str) -> Option<String> {
    crate::versions::install::bound_id_for_version(version_id)
        .map(|id| id.trim_start_matches("dsh-").to_string())
}

/// Read a plugin package's declared license: `package.json`'s `license` (or
/// `licenses`), else the presence of a `LICENSE` file (its SPDX guess is only
/// the filename, so we surface the raw marker and let the UI say "unknown"
/// when neither is present — no legal judgement is rendered here, spec §10).
fn plugin_license(node_modules: &Path, registry_id: &str) -> (Option<String>, bool) {
    let pkg_dir = node_modules.join(registry_id);
    if let Ok(raw) = std::fs::read_to_string(pkg_dir.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(l) = v.get("license").and_then(|x| x.as_str()) {
                return (Some(l.to_string()), false);
            }
            if let Some(l) = v
                .get("license")
                .and_then(|x| x.get("type"))
                .and_then(|x| x.as_str())
            {
                return (Some(l.to_string()), false);
            }
            if v.get("licenses").is_some() {
                return (Some("licenses[]".into()), false);
            }
        }
    }
    for cand in ["LICENSE", "LICENSE.md", "LICENSE.txt", "license"] {
        if pkg_dir.join(cand).is_file() {
            return (Some("LICENSE (文件存在)".into()), false);
        }
    }
    (None, true)
}

/// One pass over a plugin's directory yields BOTH the uncompressed size
/// estimate and the §12 secret-bearing files (§R7: the preview used to run two
/// separate recursive walks — `estimate_dir_size` + `collect_secret_files` —
/// over the same tree, and they disagreed on symlinks: the size walker followed
/// them while the secret walker skipped them, so the reported byte estimate and
/// the reported exclusions didn't describe the same set of files).
///
/// Now both are computed from a single walk that *skips symlinks* — matching
/// what `add_tree` actually packs (a pack is a data container; links are
/// excluded) — so "estimated N bytes" and "these files excluded" describe the
/// identical payload set. Scan errors are surfaced, not swallowed: a directory
/// we couldn't read contributes no bytes and reports an error, rather than
/// silently reading as "zero bytes, no secrets".
///
/// Returns `(total_regular_bytes, secret_paths_relative_to_dir, scan_errors)`.
fn scan_payload_dir(
    dir: &Path,
    cancel: &(impl Fn() -> bool + ?Sized),
) -> Result<(u64, Vec<String>, Vec<String>), String> {
    let mut bytes = 0u64;
    let mut secrets = Vec::new();
    let mut errors = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if cancel() {
            return Err("cancelled".into());
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            errors.push(format!("{}", current.display()));
            continue;
        };
        for entry in entries.flatten() {
            if cancel() {
                return Err("cancelled".into());
            }
            let path = entry.path();
            // Skip symlinks (never packed), exactly as the core tree-walker does.
            if path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            match path.metadata() {
                Ok(m) if m.is_dir() => stack.push(path),
                Ok(m) => {
                    bytes += m.len();
                    // `is_secret_entry_name` judges the basename, so passing the
                    // file name is enough — but a nested `config/.env` must count
                    // too, which the shared rule already guarantees.
                    if super::is_secret_entry_name(&name) {
                        if let Ok(rel) = path.strip_prefix(dir) {
                            secrets.push(rel.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
                Err(_) => errors.push(format!("{}", path.display())),
            }
        }
    }
    Ok((bytes, secrets, errors))
}

/// Build the export plan (no files written). Read-only over the instance.
pub(crate) async fn build_plan(root: &Path, id: &str) -> Result<PackExportPlan, String> {
    build_plan_with_cancel(root, id, Arc::new(AtomicBool::new(false))).await
}

async fn build_plan_with_cancel(
    root: &Path,
    id: &str,
    cancel: Arc<AtomicBool>,
) -> Result<PackExportPlan, String> {
    let inst = resolve(root, id).await?;
    let credential_envs = credential_env_names(root).await;
    let partition = partition_env(inst.manifest.env.clone(), &credential_envs);
    let plugins = scan_plugins(&inst.profile_root).await;
    let node_modules = inst.profile_root.join("node_modules");
    // §R7: the whole-tree filesystem scan (size + secrets together) runs in ONE
    // walk per plugin, off the async worker, so a large `node_modules` never
    // stalls the event loop and no plugin tree is recursored twice. Aligned 1:1
    // with `plugins` by index.
    let registry_ids: Vec<String> = plugins.iter().map(|p| p.registry_id.clone()).collect();
    let scan_root = node_modules.clone();
    let scan_cancel = cancel.clone();
    let scans = tokio::task::spawn_blocking(move || -> Result<Vec<PayloadScan>, String> {
        registry_ids
            .into_iter()
            .map(|rid| {
                if scan_cancel.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                if rid.trim().is_empty() {
                    return Ok((0u64, Vec::new(), Vec::new()));
                }
                let dir = scan_root.join(&rid);
                if !dir.is_dir() {
                    return Ok((0u64, Vec::new(), Vec::new()));
                }
                let (bytes, secrets, errors) =
                    scan_payload_dir(&dir, &|| scan_cancel.load(Ordering::SeqCst))?;
                let namespaced = secrets
                    .into_iter()
                    // Namespace by plugin so the user knows WHICH plugin carries it.
                    .map(|rel| format!("{rid}/{rel}"))
                    .collect();
                Ok((bytes, namespaced, errors))
            })
            .collect()
    })
    .await
    .map_err(|e| format!("导出预览扫描失败: {e}"))??;

    let mut plugin_plans = Vec::new();
    let mut estimated = 0u64;
    let mut warnings = Vec::new();
    let mut secret_files = Vec::new();
    for (p, (bytes, secrets, errors)) in plugins.iter().zip(scans) {
        let (license, license_unknown) = plugin_license(&node_modules, &p.registry_id);
        if license_unknown {
            warnings.push(format!(
                "插件 {} 无法确认许可证；如需再分发请先确保你有权分享它",
                p.plugin_id
            ));
        }
        // Re-downloadable means "the registry can serve this exact thing again":
        // a plugin with an install folder (registryId) AND an integrity-verified
        // or pinned trust. An unverified / unknown-trust plugin (a local build,
        // a hand-linked dir, a private fork) can't be re-fetched → embedding is
        // what preserves it, and the user decides (spec §9).
        let trusted = matches!(p.trust.as_str(), "verified" | "pinned");
        let registry_available = !p.registry_id.trim().is_empty() && trusted;
        let embed_recommended = !registry_available;
        estimated += bytes;
        secret_files.extend(secrets);
        if !errors.is_empty() {
            warnings.push(format!(
                "插件 {} 有 {} 处文件预览时无法读取；导出会跳过读不到的内容，估算未计入这些",
                p.plugin_id,
                errors.len()
            ));
        }
        plugin_plans.push(ExportPluginPlan {
            plugin_id: p.plugin_id.clone(),
            version: p.version.clone(),
            registry_id: p.registry_id.clone(),
            license,
            license_unknown,
            registry_available,
            embed_recommended,
        });
    }
    let session_count = tokio::task::spawn_blocking({
        let home = inst.home.clone();
        move || crate::discovery::inspect::count_sessions(&home)
    })
    .await
    .unwrap_or(0);
    if session_count > 0 {
        warnings.push(format!(
            "包含 {session_count} 条历史对话会被打包；会话可能含用户输入、文件内容与敏感信息"
        ));
    }
    warnings.push(format!(
        "凭据不会写入整合包，安装后需重新配置：{}",
        if partition.credentials.is_empty() {
            "（无）".into()
        } else {
            partition.credentials.join("、")
        }
    ));
    if !secret_files.is_empty() {
        secret_files.sort();
        secret_files.dedup();
        warnings.push(format!(
            "以下插件目录含疑似凭据文件，无论是否嵌入都不会打包：{}",
            secret_files.join("、")
        ));
    }
    Ok(PackExportPlan {
        instance_id: id.to_string(),
        name: inst.manifest.name.clone(),
        version_id: bare_pack_version(&inst.manifest.version_id).unwrap_or_default(),
        runtime_id: inst.manifest.runtime_id.clone(),
        profile: inst.manifest.profile.clone(),
        plugins: plugin_plans,
        session_count,
        credential_names: partition.credentials,
        secret_files,
        machine_only: partition.machine_only,
        estimated_bytes: estimated,
        warnings,
    })
}

#[tauri::command]
pub async fn preview_instance_pack_export(
    state: State<'_, PhlState>,
    id: String,
) -> Result<PackExportPlan, String> {
    build_plan(&state.root(), &id).await
}

/// The `dest` path comes from the user's save dialog and is not confined to
/// the root — same rule as the bundle export: confinement governs what we READ,
/// the destination file is the user's choice.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn export_instance_pack(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    transfer_id: String,
    id: String,
    dest: String,
    options: PackExportOptions,
) -> Result<PackExportReport, String> {
    let root = state.root();
    let flag = transfers.take(&transfer_id);
    let result =
        crate::resources::guarded(
            transfer_id.clone(),
            "pack-export",
            format!("导出实例 {id} 的整合包"),
            vec![crate::resources::Resource::Instance(id.clone())],
            Some(flag.clone()),
            &locks,
            &tasks,
            move |_task| async move {
                export_pack_inner_with_cancel(&root, id, dest, options, flag).await
            },
        )
        .await;
    transfers.release(&transfer_id);
    result
}

/// The testable core: everything the command does, over an explicit root.
#[cfg(test)]
pub(crate) async fn export_pack_inner(
    root: &Path,
    id: String,
    dest: String,
    options: PackExportOptions,
) -> Result<PackExportReport, String> {
    export_pack_inner_with_cancel(root, id, dest, options, Arc::new(AtomicBool::new(false))).await
}

async fn export_pack_inner_with_cancel(
    root: &Path,
    id: String,
    dest: String,
    options: PackExportOptions,
    cancel: Arc<AtomicBool>,
) -> Result<PackExportReport, String> {
    let inst = resolve(root, &id).await?;
    // Guard the only genuinely unsafe default: shipping sessions without the
    // explicit privacy acknowledgement the spec requires (part 11).
    let plan = build_plan_with_cancel(root, &id, cancel.clone()).await?;
    if options.include_sessions && !options.sessions_privacy_ack {
        return Err(errors::coded(
            errors::ErrCode::State,
            "导出历史对话需要先确认隐私风险警告",
        ));
    }
    if options.include_sessions && plan.session_count == 0 {
        // Nothing to include; not an error, just don't claim sessions.
    }

    let credential_envs = credential_env_names(root).await;
    let partition = partition_env(inst.manifest.env.clone(), &credential_envs);

    let bundle = InstanceBundle {
        phl_bundle: 2,
        exported_at: now_iso(),
        instance: InstanceManifest {
            env: partition.env.clone(),
            ..inst.manifest.clone()
        },
        plugins: {
            let plugins = scan_plugins(&inst.profile_root).await;
            plugins
                .into_iter()
                .map(|p| crate::instances::bundle::BundlePluginEntry {
                    plugin_id: p.plugin_id,
                    version: p.version,
                    registry_id: p.registry_id,
                })
                .collect()
        },
        credentials: partition.credentials.clone(),
    };

    // §R7: compress + hash + self-verify is pure synchronous disk/CPU work. It
    // runs in `spawn_blocking` (via `write_pack_archive`) so a large export can't
    // stall the async worker, while other tasks stay responsive. Every failure
    // path (a missing plugin dir, an un-embeddable plugin, a seal/verify error)
    // surfaces through the `?` here, and `write_pack_archive` deletes the
    // partial archive it created before returning the error.
    let build = PackBuild {
        dest,
        id,
        manifest: inst.manifest.clone(),
        sessions_home: inst.home.clone(),
        profile_root: inst.profile_root.clone(),
        plugin_plans: plan.plugins.clone(),
        session_count: plan.session_count,
        options,
        bundle,
    };
    let plugin_count = plan.plugins.len();
    let credential_names = partition.credentials;
    let pack_path = build.dest.clone();
    let (embedded_count, sessions_included, withheld, links_skipped) =
        tokio::task::spawn_blocking(move || write_pack_archive(build, cancel))
            .await
            .map_err(|e| format!("导出任务调度失败: {e}"))??;

    Ok(PackExportReport {
        pack_path,
        plugin_count,
        embedded_count,
        sessions_included,
        credential_names,
        secret_files_withheld: withheld,
        links_skipped,
    })
}

/// Owned inputs for [`write_pack_archive`], bundled so the blocking call takes
/// one `Send + 'static` argument (no borrow of the async scope) and stays under
/// clippy's arity limit (§R7).
struct PackBuild {
    dest: String,
    id: String,
    manifest: InstanceManifest,
    sessions_home: PathBuf,
    profile_root: PathBuf,
    plugin_plans: Vec<ExportPluginPlan>,
    session_count: usize,
    options: PackExportOptions,
    bundle: InstanceBundle,
}

/// Synchronously write a `.phlpack` and re-open it through the validator. The
/// whole body is blocking filesystem/CPU work and is called inside
/// `spawn_blocking` (§R7). Takes owned inputs so it can move onto the pool.
fn write_pack_archive(
    build: PackBuild,
    cancel_flag: Arc<AtomicBool>,
) -> Result<(usize, bool, Vec<String>, Vec<String>), String> {
    let PackBuild {
        dest,
        id,
        manifest: inst_manifest,
        sessions_home,
        profile_root,
        plugin_plans,
        session_count,
        options,
        bundle,
    } = build;
    let mut builder = PackBuilder::create(Path::new(&dest))?;
    let node_modules = profile_root.join("node_modules");
    let mut pack_plugins = Vec::new();
    let embed_set: std::collections::HashSet<&str> = options
        .embed_registry_ids
        .iter()
        .map(|s| s.as_str())
        .collect();
    let mut embedded_count = 0usize;
    // §12 file-layer strip: the core tree-walker withholds secret-named files
    // and reports them here so the pack's `secretsExcluded: true` is honest.
    let mut withheld: Vec<String> = Vec::new();
    let mut links_skipped: Vec<String> = Vec::new();
    let cancelled = || cancel_flag.load(Ordering::SeqCst);
    // On any error after the file is created, drop the partial archive: a failed
    // export must not leave a truncated `.phlpack` for the user to share (§R6).
    let result = (|| -> Result<(usize, bool), String> {
        for p in &plugin_plans {
            if cancelled() {
                return Err("cancelled".into());
            }
            let embed = embed_set.contains(p.registry_id.as_str());
            let source = if embed {
                // The embedded archive path mirrors the install folder exactly
                // (`embedded/plugins/<registryId>`), so the installer can map it
                // back to `node_modules/<registryId>` — including scoped npm
                // names, which are two path segments. Falls back to a sanitised
                // single segment only when the raw id could escape the archive.
                let pack_dir = format!("embedded/plugins/{}", embed_dir_name(&p.registry_id));
                let from = node_modules.join(&p.registry_id);
                if !from.is_dir() {
                    return Err(errors::coded(
                        errors::ErrCode::NotFound,
                        format!("无法打包插件目录：{}", p.registry_id),
                    ));
                }
                let report = builder.add_tree_with_cancel(&from, &pack_dir, &cancelled)?;
                withheld.extend(report.withheld_secrets);
                links_skipped.extend(report.skipped_links);
                embedded_count += 1;
                PluginSource::Embedded { path: pack_dir }
            } else {
                if !p.registry_available {
                    return Err(errors::coded(
                        errors::ErrCode::State,
                        format!(
                            "插件 {} 无法从注册表重新下载，请在导出前选择嵌入它",
                            p.plugin_id
                        ),
                    ));
                }
                PluginSource::Registry {
                    registry_id: p.registry_id.clone(),
                }
            };
            pack_plugins.push(super::PackPlugin {
                id: p.plugin_id.clone(),
                version: p.version.clone(),
                source,
                // A plugin with no registry id must be embedded, so if it got
                // here as registry it is genuinely required.
                required: true,
            });
        }

        let mut sessions_included = false;
        if options.include_sessions && session_count > 0 {
            let sessions = sessions_home.join("sessions");
            if sessions.is_dir() {
                let report = builder.add_tree_with_cancel(&sessions, "sessions", &cancelled)?;
                withheld.extend(report.withheld_secrets);
                links_skipped.extend(report.skipped_links);
                sessions_included = true;
            }
        }

        // Overrides are a forward slot; PHL-managed instances have none today,
        // so the list stays empty but the section is emitted for completeness.
        let manifest = PhlPackManifest {
            format_version: super::PACK_FORMAT_VERSION,
            pack: super::format::PackMeta {
                id: format!("pack-{}", sanitize_pack_id(&id)),
                name: inst_manifest.name.clone(),
                version: "1.0.0".into(),
                author: String::new(),
                description: format!("从 PHL 实例「{}」导出", inst_manifest.name),
                created_at: now_iso(),
                icon: None,
            },
            dsh: super::PackDsh {
                version: bare_pack_version(&inst_manifest.version_id).unwrap_or_default(),
                source: None,
                hash: None,
            },
            runtime: super::PackRuntime {
                kind: Some("node".into()),
                node_version: Some(inst_manifest.runtime_id.clone()),
                arch: Some(current_arch()),
            },
            plugins: pack_plugins,
            overrides: Vec::new(),
            content: super::PackContent {
                sessions_included,
                session_count: sessions_included.then_some(session_count),
                secrets_excluded: true,
                privacy: Some(super::PackPrivacy {
                    warning_acknowledged: sessions_included && options.sessions_privacy_ack,
                    note: sessions_included
                        .then(|| "包含历史对话，可能含用户输入与敏感信息".into()),
                }),
            },
            integrity: None,
            environment: Some(EnvironmentSection(
                serde_json::to_value(&bundle).map_err(|e| e.to_string())?,
            )),
        };

        // Seal and re-open: never ship a pack our own installer would refuse.
        builder.finish_with_cancel(manifest, &cancelled)?;
        let _validated: ValidatedPack =
            super::read_pack_from_path_with_cancel(Path::new(&dest), &cancelled)
                .map_err(super::pack_error)?;
        Ok((embedded_count, sessions_included))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&dest);
    }
    let (embedded_count, sessions_included) = result?;
    withheld.sort();
    withheld.dedup();
    links_skipped.sort();
    links_skipped.dedup();
    Ok((embedded_count, sessions_included, withheld, links_skipped))
}

/// Turn a registry id (`@scope/name`, or any npm name) into a single safe pack
/// directory segment: npm scopes and slashes become `-`, matching how the
/// plugin installer already flattens names, and the result passes `sanitize_segment`.
fn sanitize_pack_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

/// The embedded-plugin archive segment mirrors the npm install folder. A
/// registry id is already filesystem-safe (`@scope/name`, `solo`); keep it
/// verbatim so scoped packages map back correctly, but refuse anything that
/// could escape (traversal, leading slash, drive prefix) by flattening it.
fn embed_dir_name(registry_id: &str) -> String {
    let r = registry_id.replace('\\', "/");
    let unsafe_segment =
        r.is_empty() || r.starts_with('/') || r.split('/').any(|c| c == ".." || c.is_empty());
    if unsafe_segment {
        sanitize_pack_id(registry_id)
    } else {
        r
    }
}

fn current_arch() -> String {
    if cfg!(target_arch = "x86_64") {
        "x64".into()
    } else if cfg!(target_arch = "aarch64") {
        "arm64".into()
    } else {
        std::env::consts::ARCH.into()
    }
}

#[cfg(test)]
mod tests;
