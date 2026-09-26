//! Legacy `node-<major>` → precise binding conversion (R1 · M1), split out
//! of `mod.rs` so the runtime module keeps its ratchet budget (R2-07).

use std::path::Path;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use super::install::check_runtime_health;
use super::{
    legacy_major_id, precise_runtime_id, probe_node_version, run_runtime_install, RuntimeMarker,
};
use crate::paths::PhlState;
use crate::versions::{ProgressEvent, Transfers};

/* --------------------------- legacy conversion -------------------------- */

/// Read-only preview of turning one instance's legacy `node-<major>` binding
/// into a precise one. Never guesses: the conversion target is the version
/// the legacy directory's marker records, and a marker/binary mismatch is a
/// blocked preview, not a conversion candidate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConversionPreview {
    pub instance_id: String,
    pub legacy_id: String,
    pub legacy_major: u64,
    /// Version recorded in the legacy directory's install marker.
    pub recorded_version: Option<String>,
    /// Version the legacy directory's binary actually reports.
    pub binary_version: Option<String>,
    /// The precise id the conversion would bind (`node-<recorded>`).
    pub target_id: String,
    pub target_installed: bool,
    /// Non-empty when conversion is refused right now, with the reason.
    pub blocked_reason: Option<String>,
}

pub(crate) async fn conversion_preview(
    root: &Path,
    processes: &crate::launch::Processes,
    instance_id: &str,
) -> Result<RuntimeConversionPreview, String> {
    let dir = crate::instances::instance_dir(root, instance_id)?;
    let manifest = crate::instances::read_manifest(&dir)
        .await
        .ok_or_else(|| "实例不存在或清单不可读".to_string())?;
    let runtime_id = manifest.runtime_id.clone();
    let legacy = legacy_major_id(&runtime_id);
    let Some(major) = legacy else {
        let reason = if runtime_id == "node-system" {
            "系统 Node 是外部可变资源，没有可转换的精确版本".to_string()
        } else {
            format!("当前绑定 {runtime_id} 不是旧式主版本绑定，无需转换")
        };
        return Ok(RuntimeConversionPreview {
            instance_id: instance_id.to_string(),
            legacy_id: runtime_id,
            legacy_major: 0,
            recorded_version: None,
            binary_version: None,
            target_id: String::new(),
            target_installed: false,
            blocked_reason: Some(reason),
        });
    };

    let old_dir = root.join("runtimes").join(&runtime_id);
    let marker = tokio::fs::read_to_string(old_dir.join("phl-runtime.json"))
        .await
        .ok()
        .and_then(|raw| serde_json::from_str::<RuntimeMarker>(&raw).ok());
    let recorded = marker.as_ref().map(|m| m.version.clone());
    let binary = probe_node_version(
        &crate::launch::runtime_bin_dir(root, &runtime_id).join(crate::launch::node_binary()),
    )
    .await;

    let blocked =
        if let Err(e) = crate::instances::snapshot::ensure_not_running(processes, instance_id) {
            Some(e)
        } else if !old_dir.exists() {
            Some(format!(
                "旧目录 {runtime_id} 不存在，实例本身已处于需修复状态"
            ))
        } else if recorded.is_none() {
            Some("旧目录缺少安装标记，无法确定要安装的精确版本；请先修复该 Runtime".to_string())
        } else if binary.is_none() {
            Some("无法运行旧目录中的 node 二进制，请先修复该 Runtime".to_string())
        } else if recorded != binary {
            Some(format!(
                "旧目录标记版本 v{} 与实际二进制 v{} 不一致，请先修复后再转换（转换不猜版本）",
                recorded.clone().unwrap_or_default(),
                binary.clone().unwrap_or_default()
            ))
        } else {
            None
        };

    let target_id = recorded
        .as_deref()
        .map(precise_runtime_id)
        .unwrap_or_default();
    let target_installed = !target_id.is_empty()
        && tokio::fs::read_to_string(
            root.join("runtimes")
                .join(&target_id)
                .join("phl-runtime.json"),
        )
        .await
        .is_ok();
    Ok(RuntimeConversionPreview {
        instance_id: instance_id.to_string(),
        legacy_id: runtime_id,
        legacy_major: major,
        recorded_version: recorded,
        binary_version: binary,
        target_id,
        target_installed,
        blocked_reason: blocked,
    })
}

#[tauri::command]
pub async fn preview_runtime_conversion(
    phl: State<'_, PhlState>,
    processes: State<'_, crate::launch::Processes>,
    instance_id: String,
) -> Result<RuntimeConversionPreview, String> {
    conversion_preview(&phl.root(), &processes, &instance_id).await
}

/// Converts one instance's legacy binding into a precise one: prepares (and
/// verifies) the precise object first, then flips the manifest atomically. A
/// failure before the manifest write leaves the old binding untouched; after
/// it, the conversion is done — the old directory, if now unreferenced, can
/// be removed through the normal runtime cleanup.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn convert_runtime_binding(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, crate::launch::Processes>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    instance_id: String,
    dist_base: String,
    keep_archive: bool,
    on_progress: Channel<ProgressEvent>,
) -> Result<RuntimeConversionOutcome, String> {
    convert_runtime_binding_inner(
        &transfers,
        &locks,
        &tasks,
        &processes,
        &phl,
        transfer_id,
        instance_id,
        dist_base,
        keep_archive,
        on_progress,
    )
    .await
}

/// See `convert_runtime_binding`; split out so tests can drive it without a
/// Tauri app (the same pattern the snapshot/version commands use).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn convert_runtime_binding_inner(
    transfers: &Transfers,
    locks: &crate::resources::ResourceLocks,
    tasks: &crate::resources::Tasks,
    processes: &crate::launch::Processes,
    phl: &PhlState,
    transfer_id: String,
    instance_id: String,
    dist_base: String,
    keep_archive: bool,
    on_progress: Channel<ProgressEvent>,
) -> Result<RuntimeConversionOutcome, String> {
    let root = phl.root().to_path_buf();
    // Preview outside the lock: it validates and answers "what would happen".
    // The authoritative re-check happens inside the guarded body.
    let preview = conversion_preview(&root, processes, &instance_id).await?;
    if let Some(reason) = preview.blocked_reason.clone() {
        return Err(reason);
    }
    let target_id = preview.target_id.clone();
    let recorded_version = preview
        .recorded_version
        .clone()
        .ok_or_else(|| "旧目录缺少安装标记，无法确定要安装的精确版本".to_string())?;

    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "runtime-convert",
        format!("转换实例 {instance_id} 为精确 Runtime 绑定"),
        vec![
            crate::resources::Resource::Instance(instance_id.clone()),
            crate::resources::Resource::Runtime(preview.legacy_id.clone()),
            crate::resources::Resource::Runtime(target_id.clone()),
        ],
        Some(flag.clone()),
        locks,
        tasks,
        |task| {
            let root = root.clone();
            let instance_id = instance_id.clone();
            async move {
                // The launch path holds the same instance lock, but the
                // process table is the truth about a running instance.
                crate::instances::snapshot::ensure_not_running(processes, &instance_id)?;
                let target_dir = root.join("runtimes").join(&target_id);
                if !target_dir.join("phl-runtime.json").exists() {
                    run_runtime_install(
                        &flag,
                        &dist_base,
                        &target_id,
                        &recorded_version,
                        &root,
                        keep_archive,
                        &task,
                        &on_progress,
                    )
                    .await?;
                }
                // The object must be healthy *before* anything points at it:
                // binary runs and reports exactly the recorded version.
                if let Err(e) = check_runtime_health(&target_dir, &recorded_version).await {
                    return Err(format!("精确 Runtime 校验未通过，转换中止: {e}"));
                }
                let dir = crate::instances::instance_dir(&root, &instance_id)?;
                let mut manifest = crate::instances::read_manifest(&dir)
                    .await
                    .ok_or_else(|| "实例清单不可读，转换中止".to_string())?;
                manifest.runtime_id = target_id;
                crate::instances::manifest::write_manifest(&dir, &manifest).await?;
                Ok(())
            }
        },
    )
    .await;
    transfers.release(&transfer_id);
    result?;

    // The commit point is the manifest write: a fresh read reports the
    // binding the instance actually carries now.
    let dir = crate::instances::instance_dir(&root, &instance_id)?;
    let manifest = crate::instances::read_manifest(&dir)
        .await
        .ok_or_else(|| "转换后实例清单不可读".to_string())?;
    Ok(RuntimeConversionOutcome {
        instance_id,
        runtime_id: manifest.runtime_id,
    })
}

/// What a finished conversion left in place: the instance now binds this id.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConversionOutcome {
    pub instance_id: String,
    pub runtime_id: String,
}
