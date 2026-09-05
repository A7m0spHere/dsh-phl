//! The bundle format: a manifest plus the plugin records that were on disk
//! at export time. Deliberately not a package of `node_modules` — the
//! manifest is what makes an environment reproducible, and the plugin files
//! come back through the normal install pipeline.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::snapshot::sanitize_imported_env;
use super::{
    apply_api_at_create, build_instance_tree, build_record, instance_dir, load_manifest,
    profile_root, scan_plugins, InstanceManifest, InstanceRecord,
};
use crate::paths::PhlState;
use crate::versions::now_iso;

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

pub(crate) async fn read_bundle_file(path: &str) -> Result<InstanceBundle, String> {
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
pub(crate) async fn export_instance_bundle_inner(
    root: &Path,
    id: &str,
    dest: &str,
) -> Result<(), String> {
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
pub(crate) async fn import_instance_bundle_inner(
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
