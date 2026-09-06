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

use std::path::Path;

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
use crate::versions::now_iso;

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
    let home = home_of(&dir, &manifest);
    let profile_root = profile_root_of(&dir, &manifest);
    Ok(ResolvedInstance {
        manifest,
        home,
        profile_root,
    })
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

fn estimate_dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().fold(0u64, |acc, e| {
        let Ok(m) = e.metadata() else { return acc };
        if m.is_dir() {
            acc + estimate_dir_size(&e.path())
        } else {
            acc + m.len()
        }
    })
}

/// Recursively collect paths (relative to `base`, forward-slash) of files
/// whose *names* match the §12 secret families. Same rule the core
/// tree-walker applies at pack time, run here at preview so the plan can warn
/// before a byte is written. Symlinks are not followed (never packed anyway).
pub(crate) fn collect_secret_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_secret_files(base, &path, out);
        } else if super::is_secret_entry_name(&name) {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Build the export plan (no files written). Read-only over the instance.
pub(crate) async fn build_plan(root: &Path, id: &str) -> Result<PackExportPlan, String> {
    let inst = resolve(root, id).await?;
    let credential_envs = credential_env_names(root).await;
    let partition = partition_env(inst.manifest.env.clone(), &credential_envs);
    let plugins = scan_plugins(&inst.profile_root).await;
    let node_modules = inst.profile_root.join("node_modules");
    let mut plugin_plans = Vec::new();
    let mut estimated = 0u64;
    let mut warnings = Vec::new();
    let mut secret_files = Vec::new();
    for p in &plugins {
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
        let dir = node_modules.join(&p.registry_id);
        if !p.registry_id.trim().is_empty() && dir.is_dir() {
            estimated += estimate_dir_size(&dir);
            // Namespace by plugin so the user knows WHICH plugin carries it.
            let mut found = Vec::new();
            collect_secret_files(&dir, &dir, &mut found);
            secret_files.extend(
                found
                    .into_iter()
                    .map(|rel| format!("{}/{rel}", p.registry_id)),
            );
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
        version_id: inst.manifest.version_id.clone(),
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
pub async fn export_instance_pack(
    state: State<'_, PhlState>,
    id: String,
    dest: String,
    options: PackExportOptions,
) -> Result<PackExportReport, String> {
    let root = state.root();
    export_pack_inner(&root, id, dest, options).await
}

/// The testable core: everything the command does, over an explicit root.
pub(crate) async fn export_pack_inner(
    root: &Path,
    id: String,
    dest: String,
    options: PackExportOptions,
) -> Result<PackExportReport, String> {
    let inst = resolve(root, &id).await?;
    // Guard the only genuinely unsafe default: shipping sessions without the
    // explicit privacy acknowledgement the spec requires (part 11).
    let plan = build_plan(root, &id).await?;
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

    let mut builder = PackBuilder::create(Path::new(&dest))?;
    let node_modules = inst.profile_root.join("node_modules");
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
    for p in &plan.plugins {
        let embed = embed_set.contains(p.registry_id.as_str());
        let source = if embed {
            // The embedded archive path mirrors the install folder exactly
            // (`embedded/plugins/<registryId>`), so the installer can map it
            // back to `node_modules/<registryId>` — including scoped npm names,
            // which are two path segments. Falls back to a sanitised single
            // segment only when the raw id could escape the archive.
            let pack_dir = format!("embedded/plugins/{}", embed_dir_name(&p.registry_id));
            let from = node_modules.join(&p.registry_id);
            if !from.is_dir() {
                return Err(errors::coded(
                    errors::ErrCode::NotFound,
                    format!("无法打包插件目录：{}", p.registry_id),
                ));
            }
            builder
                .add_tree(&from, &pack_dir)
                .map(|report| withheld.extend(report.withheld_secrets))?;
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
            // A plugin with no registry id must be embedded, so if it got here
            // as registry it is genuinely required.
            required: true,
        });
    }

    let mut sessions_included = false;
    if options.include_sessions && plan.session_count > 0 {
        let sessions = inst.home.join("sessions");
        if sessions.is_dir() {
            builder
                .add_tree(&sessions, "sessions")
                .map(|report| withheld.extend(report.withheld_secrets))?;
            sessions_included = true;
        }
    }

    // Overrides are a forward slot; PHL-managed instances have none today, so
    // the list stays empty but the section is emitted for schema completeness.
    let manifest = PhlPackManifest {
        format_version: super::PACK_FORMAT_VERSION,
        pack: super::format::PackMeta {
            id: format!("pack-{}", sanitize_pack_id(&id)),
            name: inst.manifest.name.clone(),
            version: "1.0.0".into(),
            author: String::new(),
            description: format!("从 PHL 实例「{}」导出", inst.manifest.name),
            created_at: now_iso(),
            icon: None,
        },
        dsh: super::PackDsh {
            version: inst.manifest.version_id.clone(),
            source: None,
            hash: None,
        },
        runtime: super::PackRuntime {
            kind: Some("node".into()),
            node_version: Some(inst.manifest.runtime_id.clone()),
            arch: Some(current_arch()),
        },
        plugins: pack_plugins,
        overrides: Vec::new(),
        content: super::PackContent {
            sessions_included,
            session_count: sessions_included.then_some(plan.session_count),
            secrets_excluded: true,
            privacy: Some(super::PackPrivacy {
                warning_acknowledged: sessions_included && options.sessions_privacy_ack,
                note: sessions_included.then(|| "包含历史对话，可能含用户输入与敏感信息".into()),
            }),
        },
        integrity: None,
        environment: Some(EnvironmentSection(
            serde_json::to_value(&bundle).map_err(|e| e.to_string())?,
        )),
    };

    // Seal and re-open: never ship a pack our own installer would refuse.
    builder.finish(manifest)?;
    let _validated: ValidatedPack =
        super::read_pack_from_path(Path::new(&dest)).map_err(super::pack_error)?;

    withheld.sort();
    withheld.dedup();
    Ok(PackExportReport {
        pack_path: dest,
        plugin_count: plan.plugins.len(),
        embedded_count,
        sessions_included,
        credential_names: partition.credentials,
        secret_files_withheld: withheld,
    })
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
