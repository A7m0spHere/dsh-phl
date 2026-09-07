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

use super::cordis::{patch_path, register_cordis_patch, set_plugin_disabled};
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
            let r = run_plugin_install(
                &flag,
                &task,
                &plugin_id,
                &source,
                version.as_deref(),
                &registry_base,
                &profile,
                &on_progress,
            )
            .await;
            // A user abort must surface as `cancelled` regardless of which
            // step noticed the flag first — the task registry maps that exact
            // string to a cancellation, and anything else to a failure.
            if cancelled(&flag) {
                return Err("cancelled".into());
            }
            r
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
    let backup = node_modules.join(format!(".phl-old-{}", sanitize_cache_name(&registry_id)));
    let dest = node_modules.join(&registry_id);
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;

    // Whether the swap put a previous copy into `backup`. Read from disk
    // before the swap; kept in scope so the commit phase below can undo it.
    let had_previous = dest.exists();

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
        // Only now is there something to put in place. An install over an
        // existing plugin is an *update*, and a failed or cancelled extract
        // must leave the previous copy alone — the instance record still
        // points at it, and cordis.patch.yml still references it.
        let swapped_away = had_previous && tokio::fs::rename(&dest, &backup).await.is_ok();
        match tokio::fs::rename(&staging, &dest).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if swapped_away {
                    let _ = tokio::fs::rename(&backup, &dest).await;
                }
                Err(format!("无法放置插件目录: {e}"))
            }
        }
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
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

    // ---- commit -------------------------------------------------------
    // The swap above already moved the new package into place. From here
    // cancellation is deliberately *not* a valid outcome any more: the only
    // acceptable endings are "new install fully committed" or "old install
    // fully restored", so the cancel flag is never consulted again. The
    // backup and the pre-commit patch file are kept until every step below
    // has succeeded; a failure rolls both back.
    task.set_phase("committing");
    let patch_before = tokio::fs::read(patch_path(instance_root)).await.ok();
    if let Err(e) = commit_install(dest.as_path(), &marker, instance_root, &registry_id).await {
        let recovery = rollback_install(
            instance_root,
            &node_modules,
            &dest,
            &backup,
            patch_before.as_deref(),
            had_previous,
        )
        .await
        .map(|()| "已恢复原来的插件".to_string())
        .unwrap_or_else(|_| {
            format!(
                "恢复未完成，旧版本保留在 {}，请重试或手动处理",
                backup.display()
            )
        });
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(format!("安装提交失败，{recovery}：{e}"));
    }
    let _ = tokio::fs::remove_dir_all(&backup).await;
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

    install_plugin_dependencies(instance_root, dest)
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

/// Bound on `pnpm add` during a plugin install. A dependency closure can be
/// large (a real plugin may pull react + codemirror + …), so this is generous,
/// but it must never hang the commit — a timeout rolls the install back.
const DEPENDENCY_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Install the plugin's npm dependency closure into the profile `node_modules`.
///
/// The DSH loader imports each mounted package with the profile's own module
/// resolution, so a plugin is only runnable once its `dependencies` live in the
/// profile too — extraction alone (above) installs the bare package, and any
/// non-self-contained plugin then crashes the whole tree at boot with
/// `Cannot find package '<dep>'`. `pnpm` is the package manager the DSH profile
/// is built around (`pnpm-workspace.yaml`), so the install is pinned to it; a
/// missing `pnpm` surfaces as an actionable error, never a half-installed
/// plugin the user only discovers on the next launch.
async fn install_plugin_dependencies(profile: &Path, dest: &Path) -> Result<(), String> {
    let specs = dependency_specs(dest).await?;
    if specs.is_empty() {
        return Ok(()); // self-contained plugin: nothing to resolve
    }
    let bin = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let mut command = tokio::process::Command::new(bin);
    command
        .current_dir(profile)
        .arg("add")
        .args(&specs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CREATE_NO_WINDOW (0x0800_0000): a GUI app must not pop a console for
    // the package manager's child processes. `tokio::process::Command` exposes
    // this as an inherent method (not the std CommandExt trait).
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(DEPENDENCY_INSTALL_TIMEOUT, command.output())
        .await
        .map_err(|_| "安装依赖超时（pnpm），已回滚".to_string())?
        .map_err(|e| {
            format!(
                "无法运行 pnpm 安装依赖（DSH profile 依赖 pnpm，请先安装 pnpm 或 corepack）: {e}"
            )
        })?;
    if !output.status.success() {
        let tail = stderr_tail(&output.stderr);
        return Err(format!("pnpm 安装依赖失败: {tail}"));
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
