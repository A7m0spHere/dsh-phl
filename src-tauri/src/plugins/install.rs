//! The install pipeline: download into the instance cache, verify, extract
//! to staging, swap with rollback, write the install marker, register in
//! cordis.patch.yml. Plus the enable/uninstall lifecycle commands.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use base64::Engine;
use tauri::ipc::Channel;
use tauri::State;

use super::cordis::{register_cordis_patch, set_plugin_disabled};
use super::resolve::{registry_id_of, resolve_source, sanitize_pkg_path};
use super::{
    cancelled, compute_trust, sanitize_cache_name, source_kind, PluginInstallOutcome,
    PluginProgressEvent, PluginSourceWire,
};
use crate::paths::PhlState;
use crate::versions::{download, extract, now_iso, verify_integrity, Transfers};

/* ------------------------------ install ------------------------------ */

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_plugin(
    transfers: State<'_, Transfers>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    plugin_id: String,
    source: PluginSourceWire,
    version: Option<String>,
    registry_base: String,
    instance_id: String,
    on_progress: Channel<PluginProgressEvent>,
) -> Result<PluginInstallOutcome, String> {
    // The profile directory is resolved backend-side from the instance id —
    // the manifest's profile is the authority, not a path from the WebView.
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
    let flag = transfers.take(&transfer_id);
    let result = run_plugin_install(
        &flag,
        &plugin_id,
        &source,
        version.as_deref(),
        &registry_base,
        &profile,
        &on_progress,
    )
    .await;
    transfers.release(&transfer_id);
    result
}

async fn run_plugin_install(
    flag: &Arc<AtomicBool>,
    plugin_id: &str,
    source: &PluginSourceWire,
    requested_version: Option<&str>,
    registry_base: &str,
    instance_root: &Path,
    on_progress: &Channel<PluginProgressEvent>,
) -> Result<PluginInstallOutcome, String> {
    if instance_root.as_os_str().is_empty() {
        return Err("实例目录未设置".into());
    }
    let registry_id = registry_id_of(source);
    let registry_id = sanitize_pkg_path(&registry_id)?;

    let resolved = resolve_source(source, requested_version, registry_base, on_progress).await?;
    if cancelled(flag) {
        return Err("cancelled".into());
    }

    // Stream the tarball into the shared cache dir, hashing as we go.
    let cache_dir = instance_root.join(".phl-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| format!("无法创建缓存目录: {e}"))?;
    let part_path = cache_dir.join(format!("{}.part", sanitize_cache_name(&registry_id)));
    let downloaded = download(
        flag,
        &resolved.url,
        &part_path,
        None,
        &|progress, bytes_done, bytes_per_sec| {
            let _ = on_progress.send(PluginProgressEvent::Downloading {
                progress,
                bytes_done,
                bytes_per_sec,
            });
        },
    )
    .await;

    let downloaded = match downloaded {
        Ok(d) => d,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(e);
        }
    };

    let _ = on_progress.send(PluginProgressEvent::Verifying);
    if let Err(e) = verify_integrity(&downloaded.sha512, resolved.integrity.as_deref()) {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(e);
    }
    if cancelled(flag) {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err("cancelled".into());
    }

    // Unpack into a staging dir, then move into node_modules/<registry_id>.
    let node_modules = instance_root.join("node_modules");
    let staging = node_modules.join(format!(".phl-tmp-{}", sanitize_cache_name(&registry_id)));
    let backup = node_modules.join(format!(".phl-old-{}", sanitize_cache_name(&registry_id)));
    let dest = node_modules.join(&registry_id);
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;

    let install_result = async {
        extract(
            &part_path,
            &staging,
            &|progress| {
                let _ = on_progress.send(PluginProgressEvent::Installing { progress });
            },
            flag,
        )
        .await?;
        if !staging.exists() {
            return Err("压缩包内没有预期的 package/ 前缀".into());
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("无法创建插件目录: {e}"))?;
        }
        // Only now is there something to put in place. An install over an
        // existing plugin is an *update*, and a failed or cancelled extract
        // must leave the previous copy alone — the instance record still
        // points at it, and cordis.patch.yml still references it.
        let had_previous = dest.exists() && tokio::fs::rename(&dest, &backup).await.is_ok();
        match tokio::fs::rename(&staging, &dest).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if had_previous {
                    let _ = tokio::fs::rename(&backup, &dest).await;
                }
                Err(format!("无法放置插件目录: {e}"))
            }
        }
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;
    let _ = tokio::fs::remove_file(&part_path).await;
    install_result?;

    let trust = compute_trust(source, &resolved);
    if trust == "unverified" {
        eprintln!(
            "[phl] 插件 {plugin_id} 来自未固定来源（{}），无法校验内容一致性",
            source_kind(source)
        );
    }
    // npm spells integrity `sha512-<base64>`; the digest computed over the
    // downloaded bytes is recorded in the same shape, so any future
    // re-verification can compare against what actually landed.
    let actual = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(&downloaded.sha512)
    );
    let marker = serde_json::json!({
        "installedAt": now_iso(),
        // The catalog id (`owner/repo`) is *not* the registry id (npm package
        // name), and the instance's plugin list is keyed by the former. Record
        // it so the install can be read back off disk without guessing.
        "pluginId": plugin_id,
        "version": resolved.version,
        "kind": source_kind(source),
        "registryId": registry_id,
        "source": source,
        // The commit a GitHub install was pinned to; absent for HEAD installs.
        "ref": resolved.commit,
        "integrity": resolved.integrity.unwrap_or_default(),
        "actualIntegrity": actual,
        "trust": trust,
        "bytes": downloaded.bytes,
    });
    tokio::fs::write(dest.join("phl-plugin.json"), marker.to_string())
        .await
        .map_err(|e| format!("无法写入安装记录: {e}"))?;

    register_cordis_patch(instance_root, &registry_id, None)
        .await
        .map_err(|e| format!("注册 cordis.patch.yml 失败: {e}"))?;

    // A reinstall over a plugin the user had disabled must clear the flag:
    // `register_cordis_patch` leaves an existing block untouched, so without
    // this the frontend would record the plugin as enabled while DSH kept
    // refusing to load it — and the next disk scan would flip the switch back.
    set_plugin_disabled(instance_root, &registry_id, false)
        .await
        .map_err(|e| format!("更新 cordis.patch.yml 失败: {e}"))?;

    Ok(PluginInstallOutcome {
        version: resolved.version,
        registry_id,
        trust: trust.to_string(),
    })
}
