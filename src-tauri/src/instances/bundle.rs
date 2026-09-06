//! The bundle format: a manifest plus the plugin records that were on disk
//! at export time. Deliberately not a package of `node_modules` — the
//! manifest is what makes an environment reproducible, and the plugin files
//! come back through the normal install pipeline.
//!
//! Env values are classified at the boundary (`env_policy`): credential
//! carriers never carry their *value* in or out of a bundle — only their
//! name, so the receiving side knows what to re-configure. Format 1 predates
//! the rule; imports still apply the same value filter to those files.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::env_policy::{partition_env, partition_imported_env, EnvPartition};
use super::{
    apply_api_at_create, build_instance_tree, build_record, instance_dir, load_manifest,
    profile_root_of, scan_plugins, InstanceManifest, InstanceRecord,
};
use crate::api_config::credential_env_names;
use crate::paths::PhlState;
use crate::versions::now_iso;

/* ------------------------------- bundle ------------------------------- */

/// The bundle format: a manifest plus the plugin records that were on disk at
/// export time. Deliberately not a package of `node_modules` — the manifest
/// is what makes an environment reproducible, and the plugin files come back
/// through the normal plugin pipeline, not a private archive.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceBundle {
    /// Format tag. 1 is the original layout, whose `instance.env` could carry
    /// credential values; 2 strips those values at export and records the
    /// names in `credentials`. Import accepts both and applies the same value
    /// filter either way.
    pub phl_bundle: u32,
    pub exported_at: String,
    pub instance: InstanceManifest,
    pub plugins: Vec<BundlePluginEntry>,
    /// Env-var names whose values were stripped at export because they were
    /// classified as credential carriers. Absent in format-1 files.
    #[serde(default)]
    pub credentials: Vec<String>,
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
    /// Credential names to re-configure after import: what the exporting PHL
    /// stripped (format 2) plus whatever the classifier catches in this
    /// file's env — a format-1 file may still carry values, and they stop
    /// here.
    pub credentials: Vec<String>,
    /// Machine-local names (`PATH`, `DSH_HOME`, …) whose values import drops.
    pub machine_only: Vec<String>,
}

/// What an export kept back, shown before the file is written so the
/// omission is a decision the user sees, not a silent rewrite.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleExportReport {
    /// Credential-carrier names whose values were left out; re-enter them on
    /// the machine that imports this bundle.
    pub credentials: Vec<String>,
    /// Machine-local names — not credentials, but their values would not
    /// survive an import (or the isolation boundary) anyway.
    pub machine_only: Vec<String>,
}

pub(crate) async fn read_bundle_file(path: &str) -> Result<InstanceBundle, String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("无法读取 Bundle 文件: {e}"))?;
    // The format tag is checked before the full parse, so an unknown version
    // reports itself instead of a pile of missing-field errors.
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("Bundle 文件解析失败: {e}"))?;
    let version = value.get("phlBundle").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != 1 && version != 2 {
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
) -> Result<BundleExportReport, String> {
    export_instance_bundle_inner(&state.root(), &id, &dest).await
}

/// Exports to `dest`, which comes from the user's save dialog and is
/// deliberately *not* confined to the root — the confinement applies to what
/// gets read, while the destination is the user's own choice of file.
pub(crate) async fn export_instance_bundle_inner(
    root: &Path,
    id: &str,
    dest: &str,
) -> Result<BundleExportReport, String> {
    let (dir, manifest, partition) = load_shareable(root, id).await?;
    let plugins = scan_plugins(&profile_root_of(&dir, &manifest)).await;
    let bundle = InstanceBundle {
        phl_bundle: 2,
        exported_at: now_iso(),
        plugins: plugins
            .into_iter()
            .map(|p| BundlePluginEntry {
                plugin_id: p.plugin_id,
                version: p.version,
                registry_id: p.registry_id,
            })
            .collect(),
        instance: InstanceManifest {
            env: partition.env,
            ..manifest
        },
        credentials: partition.credentials.clone(),
    };
    let body = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    tokio::fs::write(dest, body)
        .await
        .map_err(|e| format!("无法写入 Bundle: {e}"))?;
    Ok(BundleExportReport {
        credentials: partition.credentials,
        machine_only: partition.machine_only,
    })
}

/// The export dialog's omission preview: exactly what a shareable bundle of
/// this instance would keep back, without writing anything.
#[tauri::command]
pub async fn preview_instance_export(
    state: State<'_, PhlState>,
    id: String,
) -> Result<BundleExportReport, String> {
    let (_, _, partition) = load_shareable(&state.root(), &id).await?;
    Ok(BundleExportReport {
        credentials: partition.credentials,
        machine_only: partition.machine_only,
    })
}

/// The one classification that both the preview and the real export must
/// agree on.
async fn load_shareable(
    root: &Path,
    id: &str,
) -> Result<(std::path::PathBuf, InstanceManifest, EnvPartition), String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
    let credential_envs = credential_env_names(root).await;
    let partition = partition_env(manifest.env.clone(), &credential_envs);
    Ok((dir, manifest, partition))
}

#[tauri::command]
pub async fn read_instance_bundle(
    state: State<'_, PhlState>,
    path: String,
) -> Result<BundlePreview, String> {
    read_bundle_inner(&state.root(), &path).await
}

pub(crate) async fn read_bundle_inner(root: &Path, path: &str) -> Result<BundlePreview, String> {
    let bundle = read_bundle_file(path).await?;
    // Classify the file's own env rather than trusting the embedded list: a
    // bundle is attacker-supplied by design, and format-1 files predate the
    // stripping rule entirely.
    let credential_envs = credential_env_names(root).await;
    let partition = partition_env(bundle.instance.env.clone(), &credential_envs);
    let mut credentials = bundle.credentials;
    credentials.extend(partition.credentials);
    credentials.sort();
    credentials.dedup();
    Ok(BundlePreview {
        name: bundle.instance.name,
        version_id: bundle.instance.version_id,
        runtime_id: bundle.instance.runtime_id,
        port: bundle.instance.port,
        plugin_count: bundle.plugins.len(),
        exported_at: bundle.exported_at,
        credentials,
        machine_only: partition.machine_only,
    })
}

#[tauri::command]
pub async fn import_instance_bundle(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    path: String,
    manifest: InstanceManifest,
) -> Result<ImportOutcome, String> {
    let label_id = manifest.id.clone();
    crate::resources::guarded(
        crate::resources::next_task_id("bundle-import"),
        "bundle-import",
        format!("导入 Bundle 为实例 {label_id}"),
        vec![crate::resources::Resource::Instance(label_id)],
        None,
        &locks,
        &tasks,
        move |_| async move { import_instance_bundle_inner(&state.root(), &path, manifest).await },
    )
    .await
}

/// What an import produced plus the credential names the user must
/// re-configure before the instance can authenticate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub record: InstanceRecord,
    pub credentials: Vec<String>,
}

/// Creates an instance from a bundle file. Identity (id, name, port) comes
/// from the importer so collisions stay a frontend concern; everything that
/// defines the *environment* — version, runtime, profile, env, args — comes
/// from the bundle. Plugin files are not in a bundle by design: the records
/// travel, the reinstall goes through the normal plugin pipeline.
pub(crate) async fn import_instance_bundle_inner(
    root: &Path,
    path: &str,
    manifest: InstanceManifest,
) -> Result<ImportOutcome, String> {
    let bundle = read_bundle_file(path).await?;

    let mut manifest = manifest;
    manifest.kind = bundle.instance.kind;
    manifest.hue = bundle.instance.hue;
    manifest.version_id = bundle.instance.version_id;
    manifest.runtime_id = bundle.instance.runtime_id;
    manifest.profile = bundle.instance.profile;
    manifest.note = Some("从 Bundle 导入".into());
    // Credential values never cross the bundle boundary — not even from a
    // format-1 file whose exporter predates the rule. Ordinary variables keep
    // their values; the stripped names are reported for re-configuration.
    let credential_envs = credential_env_names(root).await;
    let (env, mut credentials) = partition_imported_env(bundle.instance.env, &credential_envs);
    credentials.extend(bundle.credentials);
    credentials.sort();
    credentials.dedup();
    manifest.env = env;
    manifest.args = bundle.instance.args;

    let dir = build_instance_tree(root, &manifest).await?;
    // Same "boots configured" promise as a normal create: the imported
    // instance inherits the global library unless the manifest says otherwise.
    let manifest = apply_api_at_create(root, &dir, manifest).await;
    Ok(ImportOutcome {
        record: build_record(&dir, manifest).await,
        credentials,
    })
}
