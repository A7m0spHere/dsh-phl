//! The install pipeline: download into the instance cache, verify, extract
//! to staging, swap with rollback, write the install marker, register in
//! cordis.patch.yml. Plus the enable/uninstall lifecycle commands.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use base64::Engine;
use tauri::ipc::Channel;
use tauri::State;

#[cfg(test)]
use super::cordis::patch_path;
use super::cordis::{
    plugin_registration_state, register_cordis_patch, restore_plugin_registration_state,
    set_plugin_disabled,
};
use super::resolve::{registry_id_of, resolve_source, sanitize_pkg_path};
use super::{
    cancelled, compute_trust, sanitize_cache_name, source_kind, PluginInstallOutcome,
    PluginProgressEvent, PluginSourceWire,
};
use crate::paths::PhlState;
use crate::resources::{guarded, Resource, ResourceLocks, Tasks};
use crate::versions::{download, extract, now_iso, verify_integrity, Transfers};

/* ------------------------------ install ------------------------------ */

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_plugin(
    transfers: State<'_, Transfers>,
    locks: State<'_, ResourceLocks>,
    tasks: State<'_, Tasks>,
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
    // `writable_` also clears the external-instance gate: PHL does not install
    // plugins into a DSH_HOME the user keeps owning (spec §3.4).
    let profile =
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "安装插件到").await?;
    // A committed install changes the tree the disk-usage cache measured.
    let profile_for_cache = profile.clone();
    let flag = transfers.take(&transfer_id);
    let result = guarded(
        transfer_id.clone(),
        "plugin-install",
        format!("安装插件 {plugin_id}"),
        vec![Resource::Instance(instance_id.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
            // The pipeline owns cancellation boundaries. A late cancel must
            // not turn an already committed install or failed recovery into
            // a false "cancelled" result.
            run_plugin_install(
                &flag,
                &task,
                &plugin_id,
                &source,
                version.as_deref(),
                &registry_base,
                &profile,
                &on_progress,
            )
            .await
        },
    )
    .await;
    transfers.release(&transfer_id);
    if result.is_ok() {
        crate::instances::invalidate_disk_usage(&profile_for_cache);
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_plugin_install(
    flag: &Arc<AtomicBool>,
    task: &crate::resources::Task,
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

    // Local recovery must work even when the registry or network is offline.
    recover_profile_transaction(instance_root).await?;

    task.set_phase("resolving");
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
    task.set_phase("downloading");
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
            // A cancellation keeps the partial `.part` *and* its `.resume`
            // sidecar (download's contract): the next attempt resumes with a
            // range request, bound to the same entity. Anything else is
            // fatal or exhausted: both halves go together.
            if e != "cancelled" {
                let _ = tokio::fs::remove_file(&part_path).await;
                let _ = tokio::fs::remove_file(crate::versions::sidecar_path_of(&part_path)).await;
            }
            return Err(e);
        }
    };

    task.set_phase("verifying");
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
    let dest = node_modules.join(&registry_id);
    let _ = tokio::fs::remove_dir_all(&staging).await;

    task.set_phase("extracting");
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
        begin_transaction(instance_root, &registry_id).await?;
        let backup = transaction_dir(instance_root).join("backup");
        if dest.exists() {
            tokio::fs::rename(&dest, &backup)
                .await
                .map_err(|e| format!("无法备份旧插件: {e}"))?;
        }
        tokio::fs::rename(&staging, &dest)
            .await
            .map_err(|e| format!("无法放置插件目录: {e}"))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    if let Err(e) = install_result {
        let recovery = recover_profile_transaction(instance_root).await;
        return Err(match recovery {
            Ok(()) => e,
            Err(r) => format!("{e}；恢复未完成: {r}"),
        });
    }

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

    // ---- commit -------------------------------------------------------
    // The swap above already moved the new package into place. From here
    // cancellation is deliberately *not* a valid outcome any more: the only
    // acceptable endings are "new install fully committed" or "old install
    // fully restored", so the cancel flag is never consulted again. The
    // backup and the pre-commit patch file are kept until every step below
    // has succeeded; a failure rolls both back.
    task.set_phase("committing");
    let commit_result = async {
        commit_install(dest.as_path(), &marker, instance_root, &registry_id).await?;
        mark_transaction_committed(instance_root).await
    }
    .await;
    if let Err(e) = commit_result {
        let recovery = recover_profile_transaction(instance_root)
            .await
            .map(|()| "已恢复原来的插件".to_string())
            .unwrap_or_else(|_| {
                format!(
                    "恢复未完成，旧版本保留在 {}，请重试或手动处理",
                    transaction_dir(instance_root).display()
                )
            });
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(format!("安装提交失败，{recovery}：{e}"));
    }
    recover_profile_transaction(instance_root).await?;
    let _ = tokio::fs::remove_file(&part_path).await;

    Ok(PluginInstallOutcome {
        version: resolved.version,
        registry_id,
        trust: trust.to_string(),
    })
}

/// Everything that turns the swapped-in directory into a fully installed
/// plugin: the on-disk record, the cordis registration, and the enabled
/// flag. Any step failing aborts the commit — the caller rolls back to the
/// backup so the instance never sees a half-installed plugin.
///
/// Exposed to the crate because pack install reuses *this* primitive (not a
/// second protocol) to turn an embedded plugin that unpack dropped into
/// `node_modules/` into a fully registered one (§R3): writing the PHL install
/// marker and the Cordis registration have exactly one implementation.
pub(crate) async fn commit_install(
    dest: &Path,
    marker: &serde_json::Value,
    instance_root: &Path,
    registry_id: &str,
) -> Result<(), String> {
    tokio::fs::write(dest.join("phl-plugin.json"), marker.to_string())
        .await
        .map_err(|e| format!("无法写入安装记录: {e}"))?;

    install_plugin_dependencies(dest)
        .await
        .map_err(|e| format!("安装插件依赖失败: {e}"))?;

    register_cordis_patch(instance_root, registry_id, None)
        .await
        .map_err(|e| format!("注册 cordis.patch.yml 失败: {e}"))?;

    // A reinstall over a plugin the user had disabled must clear the flag:
    // `register_cordis_patch` leaves an existing block untouched, so without
    // this the frontend would record the plugin as enabled while DSH kept
    // refusing to load it — and the next disk scan would flip the switch back.
    set_plugin_disabled(instance_root, registry_id, false)
        .await
        .map_err(|e| format!("更新 cordis.patch.yml 失败: {e}"))?;

    Ok(())
}

/// A dependency install must complete before the package swap can commit.
const DEPENDENCY_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Keep the dependency closure inside the package being committed. A profile-
/// level `pnpm add` rewrites shared dependencies and package.json outside the
/// install rollback boundary. Ignoring the enclosing workspace also prevents
/// pnpm from mutating sibling plugins. Lifecycle scripts follow the same
/// disabled-by-default policy as the DSH version installer.
async fn install_plugin_dependencies(dest: &Path) -> Result<(), String> {
    if dependency_specs(dest).await?.is_empty() {
        return Ok(());
    }
    let bin = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let mut command = tokio::process::Command::new(bin);
    command
        .current_dir(dest)
        .args([
            "install",
            "--ignore-workspace",
            "--prod",
            "--ignore-scripts",
            "--no-lockfile",
        ])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command
        .spawn()
        .map_err(|e| format!("无法运行 pnpm 安装依赖，请先安装 pnpm 或 corepack：{e}"))?;
    let pid = child.id();
    let output = child.wait_with_output();
    tokio::pin!(output);
    let output = match tokio::time::timeout(DEPENDENCY_INSTALL_TIMEOUT, &mut output).await {
        Ok(result) => result.map_err(|e| format!("读取 pnpm 结果失败：{e}"))?,
        Err(_) => {
            if let Some(pid) = pid {
                crate::launch::kill_tree(pid).await?;
            }
            // Reap the stopped child before callers restore or delete its files.
            let _ = output.await;
            return Err("安装依赖超时（pnpm）".into());
        }
    };
    if !output.status.success() {
        return Err(format!(
            "pnpm 安装依赖失败: {}",
            stderr_tail(&output.stderr)
        ));
    }
    Ok(())
}

/// `name@range` specs for the plugin's own `dependencies`, read from its
/// extracted `package.json`. Empty for a dependency-free plugin.
async fn dependency_specs(dest: &Path) -> Result<Vec<String>, String> {
    let text = match tokio::fs::read_to_string(dest.join("package.json")).await {
        Ok(t) => t,
        Err(e) => return Err(format!("无法读取插件 package.json: {e}")),
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("插件 package.json 解析失败: {e}"))?;
    let mut specs = value
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|deps| {
            deps.iter()
                .filter_map(|(name, range)| range.as_str().map(|r| format!("{name}@{r}")))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    specs.sort();
    Ok(specs)
}

/// The last few lines of pnpm's stderr, for a failure the user can act on.
fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Undo the swap and the patch file after a failed commit, so the previous
/// plugin (or a clean pre-install state) is what remains on disk. Each step
/// is attempted even when the previous one failed; if the backup could not
/// be restored, it is left in place rather than deleted — a recoverable
/// `.phl-old-*` directory beats a silently lost previous version.
#[cfg(test)]
async fn rollback_install(
    instance_root: &Path,
    node_modules: &Path,
    dest: &Path,
    backup: &Path,
    patch_before: Option<&[u8]>,
    had_previous: bool,
) -> Result<(), String> {
    let outcome = if had_previous {
        let aside = node_modules.join(".phl-reject");
        let _ = tokio::fs::remove_dir_all(&aside).await;
        if tokio::fs::rename(dest, &aside).await.is_ok() {
            if tokio::fs::rename(backup, dest).await.is_ok() {
                let _ = tokio::fs::remove_dir_all(&aside).await;
                Ok(())
            } else {
                // Could not restore the backup — put the new copy back so at
                // least *something* loads, and leave the backup untouched.
                let _ = tokio::fs::rename(&aside, dest).await;
                Err("无法放回旧版本目录".to_string())
            }
        } else {
            Err("无法移开新版本目录".to_string())
        }
    } else {
        match tokio::fs::remove_dir_all(dest).await {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("无法移除未提交的新版本目录: {e}")),
        }
    };
    match patch_before {
        Some(bytes) => {
            let _ = tokio::fs::write(patch_path(instance_root), bytes).await;
        }
        // No patch file existed before; a partial commit may have created one.
        None => {
            let _ = tokio::fs::remove_file(patch_path(instance_root)).await;
        }
    }
    outcome
}

const TRANSACTION_DIR: &str = ".phl-plugin-txn";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
enum TransactionPhase {
    Prepared,
    RollingBack,
    Committed,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PluginTransaction {
    version: u32,
    registry_id: String,
    had_previous: bool,
    /// None = not registered; Some(true) = registered and disabled.
    registration_before: Option<bool>,
    phase: TransactionPhase,
}

fn transaction_dir(profile: &Path) -> std::path::PathBuf {
    profile.join(TRANSACTION_DIR)
}

async fn save_transaction(profile: &Path, transaction: &PluginTransaction) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let dir = transaction_dir(profile);
    let tmp = dir.join("journal.json.tmp");
    let bytes = serde_json::to_vec(transaction).map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).await.map_err(|e| e.to_string())?;
    file.sync_all().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&tmp, dir.join("journal.json"))
        .await
        .map_err(|e| format!("无法保存插件事务: {e}"))
}

async fn read_transaction(profile: &Path) -> Result<PluginTransaction, String> {
    let raw = tokio::fs::read(transaction_dir(profile).join("journal.json"))
        .await
        .map_err(|e| format!("无法读取插件事务，已保留备份: {e}"))?;
    let transaction: PluginTransaction =
        serde_json::from_slice(&raw).map_err(|e| format!("插件事务损坏，已保留备份: {e}"))?;
    if transaction.version != 1 || sanitize_pkg_path(&transaction.registry_id).is_err() {
        return Err("不支持的插件事务，已保留备份".into());
    }
    Ok(transaction)
}

async fn begin_transaction(profile: &Path, registry_id: &str) -> Result<(), String> {
    let registration_before = plugin_registration_state(profile, registry_id).await?;
    let dir = transaction_dir(profile);
    tokio::fs::create_dir(&dir)
        .await
        .map_err(|e| format!("无法创建插件事务目录: {e}"))?;
    let transaction = PluginTransaction {
        version: 1,
        registry_id: registry_id.to_owned(),
        had_previous: profile.join("node_modules").join(registry_id).exists(),
        registration_before,
        phase: TransactionPhase::Prepared,
    };
    // No package or patch mutation is permitted before this save succeeds.
    save_transaction(profile, &transaction).await
}

async fn mark_transaction_committed(profile: &Path) -> Result<(), String> {
    let mut transaction = read_transaction(profile).await?;
    transaction.phase = TransactionPhase::Committed;
    save_transaction(profile, &transaction).await
}

async fn remove_tree_if_present(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("无法清理 {}，恢复记录已保留: {e}", path.display())),
    }
}

async fn finish_transaction(profile: &Path) -> Result<(), String> {
    let dir = transaction_dir(profile);
    // The journal is deleted last. Interrupted cleanup can never leave the
    // only backup with no record explaining who owns it and whether committed.
    remove_tree_if_present(&dir.join("backup")).await?;
    remove_tree_if_present(&dir.join("reject")).await?;
    match tokio::fs::remove_file(dir.join("journal.json.tmp")).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    tokio::fs::remove_file(dir.join("journal.json"))
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::remove_dir(&dir).await.map_err(|e| e.to_string())
}

/// Must run under the instance resource lock, before any plugin mutation.
/// Recovery touches only this plugin's registration/disabled flag; another
/// plugin's settings, including edits made while PHL was stopped, survive.
pub(crate) async fn recover_profile_transaction(profile: &Path) -> Result<(), String> {
    let dir = transaction_dir(profile);
    if dir.exists() {
        crate::paths::ensure_under_root(profile, &dir)?;
        if !dir.join("journal.json").exists() {
            // A crash before the first journal commit cannot have moved a
            // package. Likewise final cleanup may have removed only journal.
            if dir.join("backup").exists() || dir.join("reject").exists() {
                return Err(format!(
                    "插件事务缺少记录，已保留 {}，请手动恢复",
                    dir.display()
                ));
            }
            remove_tree_if_present(&dir).await?;
        } else {
            let mut transaction = read_transaction(profile).await?;
            let nm = profile.join("node_modules");
            let dest = nm.join(&transaction.registry_id);
            crate::paths::ensure_under_root(&nm, &dest)?;
            let backup = dir.join("backup");
            if transaction.phase == TransactionPhase::Committed {
                if !dest.is_dir() {
                    return Err("已提交插件的目录缺失，保留事务与备份，请手动恢复".into());
                }
                finish_transaction(profile).await?;
            } else {
                // Persist BEFORE touching either copy. After a recovery crash,
                // backup absent + previous=true means the old dest is already
                // back (or the original swap never started): never delete it.
                if transaction.phase != TransactionPhase::RollingBack {
                    transaction.phase = TransactionPhase::RollingBack;
                    save_transaction(profile, &transaction).await?;
                }
                if transaction.had_previous {
                    if backup.exists() {
                        if dest.exists() {
                            remove_tree_if_present(&dir.join("reject")).await?;
                            tokio::fs::rename(&dest, dir.join("reject"))
                                .await
                                .map_err(|e| format!("无法移开未提交插件: {e}"))?;
                        }
                        tokio::fs::rename(&backup, &dest)
                            .await
                            .map_err(|e| format!("无法恢复旧插件，备份已保留: {e}"))?;
                    } else if !dest.is_dir() {
                        return Err("旧插件及其备份均缺失，保留事务待手动恢复".into());
                    }
                } else {
                    remove_tree_if_present(&dest).await?;
                }
                restore_plugin_registration_state(
                    profile,
                    &transaction.registry_id,
                    transaction.registration_before,
                )
                .await?;
                finish_transaction(profile).await?;
            }
        }
    }
    recover_legacy_backups(profile).await
}

async fn recover_legacy_backups(profile: &Path) -> Result<(), String> {
    let nm = profile.join("node_modules");
    let mut entries = match tokio::fs::read_dir(&nm).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(flat) = name.strip_prefix(".phl-old-") else {
            continue;
        };
        let backup = entry.path();
        crate::paths::ensure_under_root(&nm, &backup)?;
        let mut registry_id = None;
        for (file, field) in [("phl-plugin.json", "registryId"), ("package.json", "name")] {
            if let Ok(raw) = tokio::fs::read(backup.join(file)).await {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) {
                    if let Some(id) = value.get(field).and_then(|v| v.as_str()) {
                        if sanitize_pkg_path(id).is_ok() && sanitize_cache_name(id) == flat {
                            registry_id = Some(id.to_owned());
                            break;
                        }
                    }
                }
            }
        }
        let id = registry_id.ok_or_else(|| {
            format!(
                "无法确认旧插件备份 {} 的归属，已保留，请手动恢复",
                backup.display()
            )
        })?;
        let dest = nm.join(&id);
        crate::paths::ensure_under_root(&nm, &dest)?;
        recover_interrupted_swap(&nm, &dest, &backup).await?;
    }
    Ok(())
}

/// Reconcile legacy residue without trusting the package's early marker.
/// The old enabled flag was never persisted by that format, so keep its
/// current Cordis state; recover the old package conservatively.
///
/// A backup always wins over a landing copy: old package markers were
/// written before dependencies and Cordis, and cannot prove a commit.
async fn recover_interrupted_swap(
    node_modules: &Path,
    dest: &Path,
    backup: &Path,
) -> Result<(), String> {
    if !backup.exists() {
        return Ok(());
    }
    if !dest.exists() {
        return match tokio::fs::rename(backup, dest).await {
            Ok(()) => {
                eprintln!(
                    "[phl] 检测到中断的插件升级，已将旧版本放回 {}",
                    dest.display()
                );
                Ok(())
            }
            Err(e) => Err(format!(
                "存在旧插件备份 {} 但无法放回 {}：{e}（请重试；备份不会被删除）",
                backup.display(),
                dest.display()
            )),
        };
    }
    // The uncommitted landing copy cannot be trusted over the backup.
    let aside = node_modules.join(".phl-interrupted");
    let _ = tokio::fs::remove_dir_all(&aside).await;
    tokio::fs::rename(dest, &aside).await.map_err(|e| {
        format!(
            "检测到未提交的插件升级残留 {} 但无法移开：{e}（请重试）",
            dest.display()
        )
    })?;
    if let Err(e) = tokio::fs::rename(backup, dest).await {
        // Put the reject back rather than leave `dest` empty; the backup
        // survives, so a retry re-observes exactly this state.
        let _ = tokio::fs::rename(&aside, dest).await;
        return Err(format!(
            "旧插件备份 {} 无法放回 {}：{e}（请重试）",
            backup.display(),
            dest.display()
        ));
    }
    let _ = tokio::fs::remove_dir_all(&aside).await;
    eprintln!("[phl] 检测到未提交的插件升级，已恢复旧版本并丢弃未提交的新包");
    Ok(())
}

/* ------------------------------ tests ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("phl-plugin-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(tag: &str, with_previous: bool) -> HashMap<&'static str, std::path::PathBuf> {
        let instance = temp_root(tag);
        let node_modules = instance.join("node_modules");
        std::fs::create_dir_all(&node_modules).unwrap();
        let dest = node_modules.join("dsh-foo");
        let backup = node_modules.join(".phl-old-dsh-foo");
        if with_previous {
            std::fs::create_dir_all(&backup).unwrap();
            std::fs::write(backup.join("old.js"), "old code").unwrap();
        }
        // The swap already happened: `dest` holds the new package.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("new.js"), "new code").unwrap();
        HashMap::from([
            ("instance", instance),
            ("node_modules", node_modules),
            ("dest", dest),
            ("backup", backup),
        ])
    }

    #[tokio::test]
    async fn commit_writes_record_registration_and_enabled_state() {
        let f = fixture("commit", false);
        // A real npm package ships package.json; the commit now installs its
        // dependency closure, so give the fixture one with no deps (the
        // self-contained skip path).
        std::fs::write(
            f["dest"].join("package.json"),
            r#"{"name":"dsh-foo","version":"1.2.3"}"#,
        )
        .unwrap();
        let marker = serde_json::json!({"version": "1.2.3"});
        commit_install(&f["dest"], &marker, &f["instance"], "dsh-foo")
            .await
            .unwrap();
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(f["dest"].join("phl-plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["version"], "1.2.3");
        let patch = std::fs::read_to_string(patch_path(&f["instance"])).unwrap();
        assert!(patch.contains("- id: dsh-foo"));
        assert!(!patch.contains("disabled: true"));
    }

    #[tokio::test]
    async fn marker_write_failure_aborts_before_touching_cordis() {
        let f = fixture("marker-fail", false);
        // Deterministic failure on every platform: `phl-plugin.json` is a
        // directory, so writing it as a file must fail.
        std::fs::create_dir(f["dest"].join("phl-plugin.json")).unwrap();
        let err = commit_install(
            &f["dest"],
            &serde_json::json!({}),
            &f["instance"],
            "dsh-foo",
        )
        .await
        .expect_err("commit must fail");
        assert!(err.contains("安装记录"), "unexpected error: {err}");
        // The commit aborts at the first step: the patch file is never created.
        assert!(!patch_path(&f["instance"]).exists());
    }

    #[tokio::test]
    async fn rollback_restores_previous_package_and_old_state() {
        let f = fixture("rollback", true);
        let patch_before = b"- id: dsh-foo\n  name: dsh-foo\n  disabled: true\n";
        std::fs::write(patch_path(&f["instance"]), patch_before).unwrap();
        rollback_install(
            &f["instance"],
            &f["node_modules"],
            &f["dest"],
            &f["backup"],
            Some(patch_before),
            true,
        )
        .await
        .expect("rollback should fully restore the previous package");
        // The old package is back in place, byte-for-byte; the failed new
        // copy is gone; the backup no longer litters node_modules.
        assert_eq!(
            std::fs::read(f["dest"].join("old.js")).unwrap(),
            b"old code"
        );
        assert!(!f["dest"].join("new.js").exists());
        assert!(!f["backup"].exists());
        assert!(!f["node_modules"].join(".phl-reject").exists());
        // The pre-commit disabled state survives the failed install.
        let patch = std::fs::read_to_string(patch_path(&f["instance"])).unwrap();
        assert!(patch.contains("disabled: true"));
    }

    #[tokio::test]
    async fn rollback_of_fresh_install_removes_new_package_and_patch() {
        let f = fixture("rollback-fresh", false);
        // A partial commit created the patch file after there was none before.
        std::fs::write(
            patch_path(&f["instance"]),
            "- id: dsh-foo\n  name: dsh-foo\n",
        )
        .unwrap();
        rollback_install(
            &f["instance"],
            &f["node_modules"],
            &f["dest"],
            &f["backup"],
            None,
            false,
        )
        .await
        .expect("fresh-install rollback should fully clean up");
        assert!(!f["dest"].exists());
        assert!(!patch_path(&f["instance"]).exists());
    }

    /* ------------------- interrupted-swap recovery (R2) ------------------- */

    async fn transaction_fixture(tag: &str, previous: bool) -> std::path::PathBuf {
        let profile = temp_root(tag);
        let dest = profile.join("node_modules/dsh-foo");
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        if previous {
            std::fs::create_dir_all(&dest).unwrap();
            std::fs::write(dest.join("old.js"), "complete old package").unwrap();
            register_cordis_patch(&profile, "dsh-foo", Some(true))
                .await
                .unwrap();
        }
        begin_transaction(&profile, "dsh-foo").await.unwrap();
        profile
    }

    fn simulate_landing(profile: &Path, previous: bool) {
        let dest = profile.join("node_modules/dsh-foo");
        if previous {
            std::fs::rename(&dest, transaction_dir(profile).join("backup")).unwrap();
        }
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("new.js"), "new package").unwrap();
    }

    #[tokio::test]
    async fn transaction_every_precommit_cut_restores_old_package_and_disabled_state() {
        // 0: journal only; 1: old renamed; 2: new landed; 3: early marker;
        // 4: Cordis was enabled but final commit was not durably recorded.
        for cut in 0..=4 {
            let profile = transaction_fixture(&format!("txn-cut-{cut}"), true).await;
            let dest = profile.join("node_modules/dsh-foo");
            if cut >= 1 {
                std::fs::rename(&dest, transaction_dir(&profile).join("backup")).unwrap();
            }
            if cut >= 2 {
                std::fs::create_dir_all(&dest).unwrap();
                std::fs::write(dest.join("new.js"), "incomplete dependencies").unwrap();
            }
            if cut >= 3 {
                std::fs::write(dest.join("phl-plugin.json"), r#"{"version":"2.0.0"}"#).unwrap();
            }
            if cut >= 4 {
                set_plugin_disabled(&profile, "dsh-foo", false)
                    .await
                    .unwrap();
            }
            // Another plugin edited while PHL was stopped must survive.
            register_cordis_patch(&profile, "unrelated", Some(true))
                .await
                .unwrap();
            recover_profile_transaction(&profile).await.unwrap();
            recover_profile_transaction(&profile).await.unwrap();
            assert!(dest.join("old.js").exists(), "cut {cut}");
            assert!(!dest.join("new.js").exists(), "cut {cut}");
            assert_eq!(
                plugin_registration_state(&profile, "dsh-foo")
                    .await
                    .unwrap(),
                Some(true)
            );
            assert_eq!(
                plugin_registration_state(&profile, "unrelated")
                    .await
                    .unwrap(),
                Some(true)
            );
            assert!(!transaction_dir(&profile).exists());
            std::fs::remove_dir_all(profile).unwrap();
        }
    }

    #[tokio::test]
    async fn transaction_recovery_crash_after_old_restore_is_idempotent() {
        let profile = transaction_fixture("txn-rollback-cut", true).await;
        simulate_landing(&profile, true);
        set_plugin_disabled(&profile, "dsh-foo", false)
            .await
            .unwrap();
        let mut transaction = read_transaction(&profile).await.unwrap();
        transaction.phase = TransactionPhase::RollingBack;
        save_transaction(&profile, &transaction).await.unwrap();
        let dir = transaction_dir(&profile);
        let dest = profile.join("node_modules/dsh-foo");
        std::fs::rename(&dest, dir.join("reject")).unwrap();
        std::fs::rename(dir.join("backup"), &dest).unwrap();
        // Re-enter after the old package was restored, but before patch restore.
        recover_profile_transaction(&profile).await.unwrap();
        assert!(dest.join("old.js").exists());
        assert!(!dest.join("new.js").exists());
        assert_eq!(
            plugin_registration_state(&profile, "dsh-foo")
                .await
                .unwrap(),
            Some(true)
        );
        assert!(!dir.exists());
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[tokio::test]
    async fn transaction_only_final_commit_retires_old_backup() {
        let profile = transaction_fixture("txn-committed", true).await;
        simulate_landing(&profile, true);
        let dest = profile.join("node_modules/dsh-foo");
        std::fs::write(
            dest.join("package.json"),
            r#"{"name":"dsh-foo","version":"2.0.0"}"#,
        )
        .unwrap();
        commit_install(
            &dest,
            &serde_json::json!({"version":"2.0.0"}),
            &profile,
            "dsh-foo",
        )
        .await
        .unwrap();
        mark_transaction_committed(&profile).await.unwrap();
        recover_profile_transaction(&profile).await.unwrap();
        assert!(dest.join("new.js").exists());
        assert!(!dest.join("old.js").exists());
        assert_eq!(
            plugin_registration_state(&profile, "dsh-foo")
                .await
                .unwrap(),
            Some(false)
        );
        assert!(!transaction_dir(&profile).exists());
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[tokio::test]
    async fn transaction_failed_fresh_install_removes_only_its_registration() {
        let profile = transaction_fixture("txn-fresh", false).await;
        simulate_landing(&profile, false);
        register_cordis_patch(&profile, "dsh-foo", None)
            .await
            .unwrap();
        register_cordis_patch(&profile, "unrelated", Some(true))
            .await
            .unwrap();
        recover_profile_transaction(&profile).await.unwrap();
        assert!(!profile.join("node_modules/dsh-foo").exists());
        assert_eq!(
            plugin_registration_state(&profile, "dsh-foo")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            plugin_registration_state(&profile, "unrelated")
                .await
                .unwrap(),
            Some(true)
        );
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[tokio::test]
    async fn transaction_patch_restore_failure_retains_journal_and_restored_old_package() {
        let profile = transaction_fixture("txn-patch-failure", true).await;
        simulate_landing(&profile, true);
        std::fs::remove_file(patch_path(&profile)).unwrap();
        std::fs::create_dir(patch_path(&profile)).unwrap();
        assert!(recover_profile_transaction(&profile).await.is_err());
        assert!(profile.join("node_modules/dsh-foo/old.js").exists());
        assert_eq!(
            read_transaction(&profile).await.unwrap().phase,
            TransactionPhase::RollingBack
        );
        std::fs::remove_dir(patch_path(&profile)).unwrap();
        recover_profile_transaction(&profile).await.unwrap();
        assert_eq!(
            plugin_registration_state(&profile, "dsh-foo")
                .await
                .unwrap(),
            Some(true)
        );
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[tokio::test]
    async fn transaction_committed_cleanup_interruption_never_rolls_back_new_package() {
        let profile = transaction_fixture("txn-cleanup-cut", true).await;
        simulate_landing(&profile, true);
        mark_transaction_committed(&profile).await.unwrap();
        // A hard exit after backup cleanup but before journal removal.
        std::fs::remove_dir_all(transaction_dir(&profile).join("backup")).unwrap();
        recover_profile_transaction(&profile).await.unwrap();
        assert!(profile.join("node_modules/dsh-foo/new.js").exists());
        assert!(!transaction_dir(&profile).exists());
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[tokio::test]
    async fn legacy_backup_without_identity_is_preserved_and_blocks_mutation() {
        let f = fixture("legacy-unknown", true);
        assert!(recover_profile_transaction(&f["instance"]).await.is_err());
        assert!(f["backup"].join("old.js").exists());
        assert!(f["dest"].join("new.js").exists());
    }

    /// The crash window this recovery exists for: the old copy was renamed to
    /// `.phl-old-*` and the process died before the new package landed. The
    /// retry must move the OLD copy back — the pre-fix code deleted the
    /// backup here, and a then-failing install left no copy of the plugin.
    #[tokio::test]
    async fn interrupted_swap_restores_the_old_copy_when_dest_is_gone() {
        // `true`: the old package sits in the backup. Then `dest` disappears
        // — the state a crash between the two renames of the swap leaves.
        let f = fixture("recover-missing", true);
        std::fs::remove_dir_all(&f["dest"]).unwrap();
        recover_interrupted_swap(&f["node_modules"], &f["dest"], &f["backup"])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(f["dest"].join("old.js")).unwrap(),
            b"old code",
            "the old package is back at its real path"
        );
        assert!(!f["backup"].exists(), "restored by rename, not deleted");
    }

    /// The new package landed but never committed (no marker): the backup is
    /// the only trustworthy copy — restore it, and the uncommitted landing is
    /// discarded only once the restore has succeeded.
    #[tokio::test]
    async fn uncommitted_landing_restores_the_old_copy_then_rejects_the_new() {
        let f = fixture("recover-uncommitted", true);
        recover_interrupted_swap(&f["node_modules"], &f["dest"], &f["backup"])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(f["dest"].join("old.js")).unwrap(),
            b"old code"
        );
        assert!(
            !f["dest"].join("new.js").exists(),
            "the uncommitted copy is gone"
        );
        assert!(!f["backup"].exists());
        assert!(
            !f["node_modules"].join(".phl-interrupted").exists(),
            "the reject is discarded only after the restore"
        );
    }

    /// The legacy marker was written before dependencies and Cordis, so even
    /// a valid marker cannot authorize discarding the only complete old copy.
    #[tokio::test]
    async fn legacy_marker_does_not_prove_commit_or_retire_backup() {
        let f = fixture("recover-committed", true);
        std::fs::write(f["dest"].join("phl-plugin.json"), r#"{"version":"2.0.0"}"#).unwrap();
        recover_interrupted_swap(&f["node_modules"], &f["dest"], &f["backup"])
            .await
            .unwrap();
        assert!(
            f["dest"].join("old.js").exists(),
            "an early marker cannot prove dependency and Cordis commit"
        );
        assert!(!f["dest"].join("new.js").exists());
    }

    /// A torn marker is NOT a commit — half-written JSON reads as
    /// uncommitted, and the old copy wins until the retry commits cleanly.
    #[tokio::test]
    async fn a_torn_marker_does_not_count_as_committed() {
        let f = fixture("recover-torn", true);
        std::fs::write(f["dest"].join("phl-plugin.json"), b"{\"version\":\"2").unwrap();
        recover_interrupted_swap(&f["node_modules"], &f["dest"], &f["backup"])
            .await
            .unwrap();
        assert!(f["dest"].join("old.js").exists());
        assert!(!f["dest"].join("new.js").exists());
    }

    /// No residue at all: recovery is inert, the normal swap applies.
    #[tokio::test]
    async fn no_backup_leaves_nothing_to_recover() {
        let f = fixture("recover-none", false);
        recover_interrupted_swap(&f["node_modules"], &f["dest"], &f["backup"])
            .await
            .unwrap();
        assert!(f["dest"].join("new.js").exists());
    }

    /// A writable directory on a volume *other* than `%TEMP%`, or `None` on a
    /// single-volume machine (then the cross-device rename cannot be proven
    /// and the test skips — same convention as the storage undo tests).
    fn other_volume_dir(tag: &str) -> Option<std::path::PathBuf> {
        let temp = std::env::temp_dir();
        let temp_prefix = temp.components().next()?;
        for letter in b'C'..=b'Z' {
            let root = std::path::PathBuf::from(format!("{}:\\", letter as char));
            if !root.is_dir() || root.components().next() == Some(temp_prefix) {
                continue;
            }
            let probe = root.join(format!("phl-plugin-{tag}-{}", std::process::id()));
            if std::fs::create_dir_all(&probe).is_ok() {
                return Some(probe);
            }
        }
        None
    }

    /// The failure path of the restore itself: recovery never trades the only
    /// old copy for an error. The backup lives on a different volume, so the
    /// `dest` landing rename fails exactly as it would for a wedged disk
    /// (EXDEV / ERROR_NOT_SAME_DEVICE) — the old copy must survive untouched.
    #[tokio::test]
    async fn a_blocked_restore_keeps_the_backup_and_reports() {
        let f = fixture("recover-xdev", true);
        std::fs::remove_dir_all(&f["dest"]).unwrap();
        let Some(other) = other_volume_dir("xdev") else {
            eprintln!("[phl] 只有一个卷，跳过跨卷恢复失败测试");
            return;
        };
        // Precondition stated, not assumed: a rename must genuinely fail
        // between these two roots, otherwise the injection proves nothing.
        let probe_src = f["node_modules"].join("xdev-probe");
        std::fs::write(&probe_src, b"x").unwrap();
        if std::fs::rename(&probe_src, other.join("xdev-probe")).is_ok() {
            let _ = std::fs::rename(other.join("xdev-probe"), &probe_src);
            let _ = std::fs::remove_file(&probe_src);
            let _ = std::fs::remove_dir_all(&other);
            eprintln!("[phl] 两个根之间可以重命名，跳过跨卷恢复失败测试");
            return;
        }
        let _ = std::fs::remove_file(&probe_src);

        let far_backup = other.join("dsh-far-backup");
        std::fs::create_dir_all(&far_backup).unwrap();
        std::fs::write(far_backup.join("old.js"), "old code").unwrap();
        std::fs::remove_dir_all(&f["backup"]).unwrap();

        let err = recover_interrupted_swap(&f["node_modules"], &f["dest"], &far_backup)
            .await
            .unwrap_err();
        assert!(err.contains("放回"), "{err}");
        assert!(
            far_backup.join("old.js").exists(),
            "the only copy of the old plugin survives the failed restore"
        );
        let _ = std::fs::remove_dir_all(&other);
    }

    #[tokio::test]
    async fn rollback_keeps_backup_when_restore_fails() {
        // Simulate the backup directory being unrecoverable: it was already
        // deleted by an outside hand. The old package must NOT be destroyed
        // and the backup path must not be reported as consumed.
        let f = fixture("rollback-keep", true);
        std::fs::remove_dir_all(&f["backup"]).unwrap();
        rollback_install(
            &f["instance"],
            &f["node_modules"],
            &f["dest"],
            &f["backup"],
            None,
            true,
        )
        .await
        .expect_err("a lost backup must be reported, not papered over");
        // dest holds the new copy again (rename aside -> rename backup fails
        // -> rename aside back), nothing is silently deleted.
        assert!(f["dest"].join("new.js").exists());
    }

    #[tokio::test]
    async fn dependency_specs_are_sorted_and_ranged() {
        let dir = temp_root("dep-specs");
        let dest = dir.join("node_modules").join("dsh-foo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("package.json"),
            r#"{"dependencies":{"ws":"^8.0.0","schemastery":"^3.0.0","react":"18.3.1"}}"#,
        )
        .unwrap();
        assert_eq!(
            dependency_specs(&dest).await.unwrap(),
            vec!["react@18.3.1", "schemastery@^3.0.0", "ws@^8.0.0"],
        );
    }

    #[tokio::test]
    #[ignore = "requires pnpm and Node on PATH; uses only a local fixture dependency"]
    async fn dependencies_stay_inside_the_plugin_and_scripts_do_not_run() {
        let root = temp_root("dep-isolation");
        let profile = root.join("profile");
        let dest = profile.join("node_modules/dsh-local");
        let dependency = root.join("local-dep");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(
            dependency.join("package.json"),
            r#"{"name":"phl-fixture-dep","version":"1.0.0","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(dependency.join("index.js"), "module.exports = 42").unwrap();
        let package = serde_json::json!({
            "name": "dsh-local", "version": "1.0.0",
            "dependencies": {"phl-fixture-dep": format!("file:{}", dependency.to_string_lossy().replace('\\', "/"))},
            "scripts": {"postinstall": "node -e \"require('fs').writeFileSync('script-ran', 'bad')\""}
        });
        std::fs::write(dest.join("package.json"), package.to_string()).unwrap();
        std::fs::write(
            profile.join("package.json"),
            "{\"name\":\"profile\",\"private\":true}",
        )
        .unwrap();
        std::fs::write(
            profile.join("pnpm-workspace.yaml"),
            "packages:\n  - node_modules/*\n",
        )
        .unwrap();
        let before = std::fs::read(profile.join("package.json")).unwrap();
        install_plugin_dependencies(&dest).await.unwrap();
        assert_eq!(std::fs::read(profile.join("package.json")).unwrap(), before);
        assert!(!profile.join("pnpm-lock.yaml").exists());
        assert!(!profile.join("node_modules/phl-fixture-dep").exists());
        assert!(!dest.join("script-ran").exists());
        let output = tokio::process::Command::new("node")
            .current_dir(&dest)
            .args([
                "-e",
                "if (require('phl-fixture-dep') !== 42) process.exit(1)",
            ])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn self_contained_plugin_has_no_dependency_specs() {
        let dir = temp_root("dep-none");
        let dest = dir.join("node_modules").join("dsh-bar");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("package.json"), r#"{"name":"dsh-bar"}"#).unwrap();
        assert!(dependency_specs(&dest).await.unwrap().is_empty());
    }

    #[test]
    fn stderr_tail_keeps_the_last_few_lines_in_order() {
        let bytes = b"line1\nline2\nline3\n".to_vec();
        assert_eq!(stderr_tail(&bytes), "line1\nline2\nline3");
    }
}
