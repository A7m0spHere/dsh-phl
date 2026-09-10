//! Environment repair (T-102) and residue cleanup (T-104), Batch E.
//!
//! Repair deliberately stays small and local: it only executes actions that
//! are *derivable* — recreating a workspace directory the manifest implies,
//! and sweeping transaction leftovers that no longer belong to a live
//! install. Anything that needs a download (a missing DSH version or
//! runtime) or user judgement is reported back as `requiresUser` so the
//! frontend can route it into the normal install flows instead of the
//! backend silently re-downloading things.
//!
//! Every action is followed by a fresh verify from the caller side: repair
//! returns what it did, the UI re-checks, and the health badge tells the
//! truth.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::paths::PhlState;

/// Transaction leftovers younger than this may belong to an install that is
/// running right now — sweeping them would race the installer. Stale ones
/// are what an interrupted launch of PHL left behind.
const RESIDUE_GRACE: Duration = Duration::from_secs(60 * 60);

/// The repair actions this module executes itself. Everything else that
/// verify can flag (install-version, install-runtime, reinstall-*) needs a
/// download and stays a user-routed action.
const LOCAL_ACTIONS: &[&str] = &["recreate-workspace", "recreate-skeleton", "cleanup-txn"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidueItem {
    /// Which domain the leftover belongs to: `versions` | `runtimes` | `instances` | `cache`.
    pub kind: String,
    pub path: String,
    /// Bytes, best-effort.
    pub size: u64,
    /// False while the leftover is younger than the grace period.
    pub stale: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairOutcome {
    pub applied: Vec<String>,
    /// Recognized but not executed here — the user runs them through the
    /// normal install/remove flows.
    pub requires_user: Vec<String>,
    /// Unknown action names, rejected outright.
    pub rejected: Vec<String>,
}

#[tauri::command]
pub async fn repair_instance(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    instance_id: String,
    actions: Vec<String>,
) -> Result<RepairOutcome, String> {
    crate::resources::guarded(
        crate::resources::next_task_id("instance-repair"),
        "instance-repair",
        format!("修复实例 {instance_id}"),
        vec![crate::resources::Resource::Instance(instance_id.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move { repair_instance_inner(&phl.root(), &instance_id, &actions).await },
    )
    .await
}

pub(crate) async fn scan_residue_inner(root: &Path) -> Result<Vec<ResidueItem>, String> {
    Ok(scan_residue_with(root, default_cutoff()).await)
}

/// The sweep/scan time boundary: leftovers modified before it are stale.
fn default_cutoff() -> std::time::SystemTime {
    std::time::SystemTime::now() - RESIDUE_GRACE
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    path.metadata().ok()?.modified().ok()
}

fn is_stale(path: &Path, cutoff: std::time::SystemTime) -> bool {
    modified(path).map(|m| m < cutoff).unwrap_or(true)
}

/// Every leftover PHL's installers can leave behind. `cutoff` splits fresh
/// (a running install may still own them) from stale (PHL died mid-run).
pub(crate) async fn scan_residue_with(
    root: &Path,
    cutoff: std::time::SystemTime,
) -> Vec<ResidueItem> {
    let mut out = Vec::new();

    // 1. Transaction staging/backup trees: every child of `<domain>/.phl-txn`.
    for domain in ["versions", "runtimes"] {
        let txn = root.join(domain).join(".phl-txn");
        let Ok(mut entries) = tokio::fs::read_dir(&txn).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let probe = path.clone();
            let size = tokio::task::spawn_blocking(move || crate::instances::dir_size(&probe))
                .await
                .unwrap_or(0);
            out.push(ResidueItem {
                kind: domain.into(),
                path: path.to_string_lossy().into_owned(),
                size,
                stale: is_stale(&path, cutoff),
            });
        }
    }

    // 2. Interrupted creates and pack installs: hidden staging trees that no
    //    scanner lists (see `paths::is_hidden_tree_name`). Left unswept they
    //    would be gigabytes of silently wasted disk; listed here they follow
    //    the same grace rule as `.phl-txn` children — a create/pack install
    //    still running owns its staging dir, so fresh items are reported
    //    stale=false and never removed.
    for path in staging_instance_dirs(root).await {
        let probe = path.clone();
        let size = tokio::task::spawn_blocking(move || crate::instances::dir_size(&probe))
            .await
            .unwrap_or(0);
        out.push(ResidueItem {
            kind: "instances".into(),
            path: path.to_string_lossy().into_owned(),
            size,
            stale: is_stale(&path, cutoff),
        });
    }

    // 3. Legacy staging names from the pre-transactional installers.
    let legacy_prefixes = [".phl-new-", ".phl-old-"];
    if let Ok(mut entries) = tokio::fs::read_dir(root.join("runtimes")).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || !legacy_prefixes.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            let probe = path.clone();
            let size = tokio::task::spawn_blocking(move || crate::instances::dir_size(&probe))
                .await
                .unwrap_or(0);
            out.push(ResidueItem {
                kind: "runtimes".into(),
                path: path.to_string_lossy().into_owned(),
                size,
                stale: is_stale(&path, cutoff),
            });
        }
    }

    // 4. Orphaned `.part` download fragments: a finished or cancelled
    // download always renames or removes them, so one left behind means PHL
    // died mid-download.
    if let Ok(mut entries) = tokio::fs::read_dir(root.join("cache")).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_file() || !name.ends_with(".part") {
                continue;
            }
            out.push(ResidueItem {
                kind: "cache".into(),
                path: path.to_string_lossy().into_owned(),
                size: path.metadata().map(|m| m.len()).unwrap_or(0),
                stale: is_stale(&path, cutoff),
            });
        }
    }
    out
}

/// Every hidden staging tree an interrupted create/clone/pack-install or a
/// crashed snapshot create can leave inside the `instances/` domain:
/// `instances/.phl-new-*`, `instances/.phl-pack-*` and, per instance,
/// `instances/<id>/snapshots/.phl-new-*`. These names have no recovery role
/// (a retry's own `remove_dir_all(staging)` is their recovery path), unlike
/// `.phl-restore` / `.phl-old-dsh-home` inside an instance dir — those are
/// live restore-transaction state handled by
/// `snapshot::recover_interrupted_restore` and deliberately stay out here.
async fn staging_instance_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(root.join("instances")).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        if name.starts_with(".phl-new-") || name.starts_with(".phl-pack-") {
            out.push(path);
            continue;
        }
        if name.starts_with('.') {
            continue; // not ours to enumerate
        }
        let Ok(mut snaps) = tokio::fs::read_dir(path.join("snapshots")).await else {
            continue;
        };
        while let Ok(Some(s)) = snaps.next_entry().await {
            let sp = s.path();
            let sname = sp.file_name().map(|n| n.to_string_lossy().into_owned());
            if sp.is_dir() && sname.is_some_and(|n| n.starts_with(".phl-new-")) {
                out.push(sp);
            }
        }
    }
    out
}

pub(crate) async fn repair_instance_inner(
    root: &Path,
    instance_id: &str,
    actions: &[String],
) -> Result<RepairOutcome, String> {
    let mut outcome = RepairOutcome {
        applied: Vec::new(),
        requires_user: Vec::new(),
        rejected: Vec::new(),
    };

    for action in actions {
        if !LOCAL_ACTIONS.contains(&action.as_str()) {
            if is_known_remote_action(action) {
                if !outcome.requires_user.contains(action) {
                    outcome.requires_user.push(action.clone());
                }
            } else {
                outcome.rejected.push(action.clone());
            }
            continue;
        }
        let result = match action.as_str() {
            "recreate-workspace" => recreate_workspace(root, instance_id).await,
            "recreate-skeleton" => recreate_skeleton(root, instance_id).await,
            "cleanup-txn" => cleanup_txn(root).await,
            _ => unreachable!("LOCAL_ACTIONS is the whitelist above"),
        };
        match result {
            Ok(()) => {
                if !outcome.applied.contains(action) {
                    outcome.applied.push(action.clone());
                }
            }
            Err(e) => return Err(format!("修复动作 {action} 失败: {e}")),
        }
    }
    Ok(outcome)
}

/// Repair actions verify can flag that need a download or user judgement —
/// known to the router, but never executed here.
fn is_known_remote_action(action: &str) -> bool {
    matches!(
        action,
        "install-version" | "reinstall-version" | "install-runtime" | "reinstall-runtime"
    )
}

/// Recreates the instance's whole derivable skeleton — `dsh-home` with its
/// profile's empty `node_modules`, plus `logs`. This is the local half of
/// "rebuild from manifest" (T-204): after the environment was wiped, the
/// directory tree the manifest implies comes back exactly as a fresh create
/// would have made it, and the pinned DSH version / runtime are restored
/// through the normal install flows the UI routes to.
async fn recreate_skeleton(root: &Path, instance_id: &str) -> Result<(), String> {
    let dir = crate::instances::instance_dir(root, instance_id)?;
    let manifest = crate::instances::load_manifest(&dir, instance_id).await?;
    // Only the PHL-owned copy tree is rebuildable. An external instance's
    // DSH_HOME is the user's directory — recreating an empty skeleton there
    // would be the opposite of repair, so those entries are skipped; the
    // wrapper dirs (workspace/logs) still live inside the instance tree.
    let mut relative = vec![PathBuf::from("logs"), PathBuf::from("workspace")];
    if !crate::instances::is_external(&manifest) {
        relative.push(
            PathBuf::from("dsh-home")
                .join("profiles")
                .join(&manifest.profile)
                .join("node_modules"),
        );
    }
    for rel in relative {
        let target = dir.join(&rel);
        if !target.exists() {
            tokio::fs::create_dir_all(&target)
                .await
                .map_err(|e| format!("无法重建 {}: {e}", rel.display()))?;
        }
    }
    Ok(())
}

/// The workspace directory is fully derivable from the manifest — recreating
/// an empty one can never lose data, it only makes a missing one visible
/// again.
async fn recreate_workspace(root: &Path, instance_id: &str) -> Result<(), String> {
    let dir = crate::instances::instance_dir(root, instance_id)?;
    // The manifest load doubles as the existence check: a directory without
    // a readable manifest is not ours to "repair".
    crate::instances::load_manifest(&dir, instance_id).await?;
    let workspace = dir.join("workspace");
    if !workspace.exists() {
        tokio::fs::create_dir_all(&workspace)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Sweep transaction leftovers. Fresh leftovers are skipped (a running
/// install may still own them) and stay visible through `scan_residue`;
/// stale ones are safe to remove by construction — the transactional
/// installers only ever leave them behind when PHL itself died mid-run.
async fn cleanup_txn(root: &Path) -> Result<(), String> {
    sweep_txn(root, default_cutoff()).await.map(|_| ())
}

pub(crate) async fn sweep_txn(root: &Path, cutoff: std::time::SystemTime) -> Result<usize, String> {
    let mut removed = 0;
    for base in [
        root.join("versions").join(".phl-txn"),
        root.join("runtimes").join(".phl-txn"),
    ] {
        let Ok(mut entries) = tokio::fs::read_dir(&base).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() || !is_stale(&path, cutoff) {
                continue;
            }
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| format!("无法清理 {}: {e}", path.display()))?;
            removed += 1;
        }
        // The transaction parent is a persistent empty shell once every tree
        // inside it has been consumed — remove it best-effort. A non-empty
        // parent (a fresh tree a running install still owns) fails harmlessly
        // here and stays visible through the residue scan.
        let _ = tokio::fs::remove_dir(&base).await;
    }

    // Hidden staging trees from interrupted creates, pack installs and
    // snapshot copies: stale ones are safe to remove by construction (their
    // owning transaction is over — a retry cleans them itself), fresh ones
    // belong to a live install and survive the sweep.
    for path in staging_instance_dirs(root).await {
        if !is_stale(&path, cutoff) {
            continue;
        }
        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(|e| format!("无法清理 {}: {e}", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{create_instance_inner, InstanceManifest};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-repair-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str) -> InstanceManifest {
        InstanceManifest {
            schema_version: 0,
            id: id.into(),
            name: "Repaired".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            port: 34480,
            auto_port: true,
            profile: "web".into(),
            created_at: crate::versions::now_iso(),
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

    #[tokio::test]
    async fn workspace_is_recreated_when_missing() {
        let root = temp_root("workspace");
        let id = "ws-0001";
        create_instance_inner(&root, manifest(id)).await.unwrap();
        let workspace = root.join("instances").join(id).join("workspace");
        tokio::fs::remove_dir_all(&workspace).await.unwrap();

        let outcome = repair_instance_inner(&root, id, &["recreate-workspace".into()])
            .await
            .unwrap();
        assert_eq!(outcome.applied, vec!["recreate-workspace"]);
        assert!(workspace.exists(), "workspace is back");
        // Idempotent: repairing again changes nothing but still succeeds.
        let outcome = repair_instance_inner(&root, id, &["recreate-workspace".into()])
            .await
            .unwrap();
        assert_eq!(outcome.applied, vec!["recreate-workspace"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_wiped_environment_skeleton_is_rebuildable_from_the_manifest() {
        let root = temp_root("rebuild");
        let id = "rebuild-1";
        create_instance_inner(&root, manifest(id)).await.unwrap();

        // Simulate "deleted the local environment, kept the manifest": the
        // instance directory keeps only instance.json.
        let dir = root.join("instances").join(id);
        for name in ["dsh-home", "workspace", "logs"] {
            tokio::fs::remove_dir_all(dir.join(name)).await.unwrap();
        }

        let outcome = repair_instance_inner(&root, id, &["recreate-skeleton".into()])
            .await
            .unwrap();
        assert_eq!(outcome.applied, vec!["recreate-skeleton"]);
        assert!(dir.join("workspace").exists());
        assert!(dir.join("logs").exists());
        assert!(
            dir.join("dsh-home")
                .join("profiles")
                .join("web")
                .join("node_modules")
                .exists(),
            "profile skeleton comes back exactly as a fresh create"
        );

        // And verify agrees the derivable half is healthy again.
        let result = crate::verify::verify_instance_inner(&root, id)
            .await
            .unwrap();
        let broken = result
            .checks
            .iter()
            .filter(|c| c.status == "fail")
            .all(|c| {
                c.repair_action
                    .as_deref()
                    .is_some_and(|a| a.starts_with("install-"))
            });
        assert!(
            broken,
            "only install actions may still fail: {:?}",
            result.checks
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn remote_actions_route_to_the_user_and_unknown_ones_are_rejected() {
        let root = temp_root("route");
        let id = "route-01";
        create_instance_inner(&root, manifest(id)).await.unwrap();

        let outcome = repair_instance_inner(
            &root,
            id,
            &[
                "install-version".into(),
                "reinstall-runtime".into(),
                "format-c-drive".into(),
            ],
        )
        .await
        .unwrap();
        assert!(outcome.applied.is_empty());
        assert_eq!(
            outcome.requires_user,
            vec!["install-version", "reinstall-runtime"]
        );
        assert_eq!(outcome.rejected, vec!["format-c-drive"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn txn_sweep_skips_fresh_leftovers_and_removes_stale_ones() {
        let root = temp_root("txn");
        let txn = root.join("versions").join(".phl-txn");
        let fresh = txn.join("0.1.0.staging-1");
        std::fs::create_dir_all(fresh.join("lib")).unwrap();
        std::fs::write(fresh.join("lib").join("bin.js"), "// bin").unwrap();
        let older = txn.join("0.2.0.backup-2");
        std::fs::create_dir_all(older.join("lib")).unwrap();

        // A cutoff in the future marks everything stale: the sweep removes
        // both trees, exactly as it would for leftovers old enough that no
        // running install can own them.
        let everything_stale = std::time::SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(sweep_txn(&root, everything_stale).await.unwrap(), 2);
        assert!(!fresh.exists() && !older.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn fresh_leftovers_survive_the_default_sweep_and_stay_visible() {
        let root = temp_root("fresh-txn");
        let txn = root.join("runtimes").join(".phl-txn");
        let fresh = txn.join("node-22.staging-7");
        std::fs::create_dir_all(&fresh).unwrap();

        // The default cutoff is an hour ago; a leftover created just now is
        // fresh, so the sweep leaves it — and scan_residue still reports it
        // so the diagnostics page can show what an interrupted install left.
        cleanup_txn(&root).await.unwrap();
        assert!(fresh.exists(), "fresh leftover survives the sweep");

        let residue = scan_residue_inner(&root).await.unwrap();
        let item = residue
            .iter()
            .find(|r| r.path.ends_with("node-22.staging-7"))
            .expect("leftover is reported");
        assert_eq!(item.kind, "runtimes");
        assert!(!item.stale);

        // A cutoff in the future marks it stale for the scan (anything
        // modified before that instant counts as old enough to sweep).
        let future = std::time::SystemTime::now() + Duration::from_secs(3600);
        let residue = scan_residue_with(&root, future).await;
        assert!(residue[0].stale);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn residue_scan_sees_part_files_and_legacy_staging() {
        let root = temp_root("residue");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(
            root.join("cache").join("dsh-0.1.0.tgz.part"),
            vec![0u8; 128],
        )
        .unwrap();
        let legacy = root.join("runtimes").join(".phl-old-node-22");
        std::fs::create_dir_all(&legacy).unwrap();

        let residue = scan_residue_inner(&root).await.unwrap();
        assert!(residue.iter().any(|r| r.kind == "cache" && r.size == 128));
        assert!(residue.iter().any(|r| r.path.ends_with(".phl-old-node-22")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn interrupted_instance_staging_is_residue_and_sweepable() {
        let root = temp_root("instance-staging");
        let instances = root.join("instances");
        // A create/clone that died between the manifest write and the
        // promote rename, and a pack install staging tree.
        let clone = instances.join(".phl-new-clone-9");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::write(clone.join("instance.json"), "{}").unwrap();
        let pack = instances.join(".phl-pack-inst-7");
        std::fs::create_dir_all(&pack).unwrap();
        // A snapshot create that died after its snapshot.json, still under the
        // hidden staging name.
        let snap = instances
            .join("inst-1")
            .join("snapshots")
            .join(".phl-new-snap-5");
        std::fs::create_dir_all(&snap).unwrap();
        // Business objects that must survive every sweep: a healthy instance
        // and a live restore transaction's backup tree *inside* an instance.
        let healthy = instances.join("inst-2");
        std::fs::create_dir_all(&healthy).unwrap();
        std::fs::write(healthy.join("instance.json"), "{}").unwrap();
        let backup = instances.join("inst-3").join(".phl-old-dsh-home");
        std::fs::create_dir_all(&backup).unwrap();

        let residue = scan_residue_inner(&root).await.unwrap();
        for name in [".phl-new-clone-9", ".phl-pack-inst-7", ".phl-new-snap-5"] {
            let item = residue
                .iter()
                .find(|r| r.path.ends_with(name))
                .unwrap_or_else(|| panic!("{name} must be reported as residue: {residue:?}"));
            assert_eq!(item.kind, "instances");
            assert!(!item.stale, "everything created now is fresh");
        }
        assert!(
            !residue
                .iter()
                .any(|r| r.path.ends_with(".phl-old-dsh-home")),
            "restore-transaction state carries recovery meaning and is not residue"
        );
        assert!(
            !residue.iter().any(|r| r.path.ends_with("inst-2")),
            "a healthy instance is never residue"
        );

        // The default (one-hour-ago) cutoff leaves every fresh tree alone...
        cleanup_txn(&root).await.unwrap();
        assert!(clone.exists() && pack.exists() && snap.exists());
        // ...a future cutoff marks all stale: the sweep takes the three
        // staging trees and nothing else.
        let future = std::time::SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(sweep_txn(&root, future).await.unwrap(), 3);
        assert!(!clone.exists() && !pack.exists() && !snap.exists());
        assert!(healthy.exists());
        assert!(backup.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
