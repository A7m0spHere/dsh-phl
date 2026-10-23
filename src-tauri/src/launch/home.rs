//! Prepare a managed/external home under the launcher's instance lock.
//! Restore recovery must precede directory creation or plugin/API writes.
use super::import_launch_gate;
use std::path::{Path, PathBuf};

pub(crate) async fn prepare(
    instance_dir: &Path,
    profile: &str,
) -> Result<(PathBuf, PathBuf), String> {
    // The home follows the manifest: a managed copy lives inside the instance
    // directory, an externally-adopted instance points at its own DSH_HOME.
    // Creating `profiles/web` under it is idempotent (external homes already
    // have it; this only matters for a freshly created copy).
    let manifest = crate::instances::read_manifest(instance_dir).await;
    if crate::instances::snapshot::has_pending_restore(instance_dir).await? {
        let restore_manifest = manifest
            .as_ref()
            .ok_or("实例清单不可读取，已保留中断的快照还原现场")?;
        crate::instances::snapshot::recover_interrupted_restore(instance_dir, restore_manifest)
            .await?;
    }
    let dsh_home = match &manifest {
        Some(m) => crate::instances::home_of(instance_dir, m),
        None => instance_dir.join("dsh-home"),
    };
    // Launch gate for community imports (CR-06/D17, R3-03): a pending
    // dependency install AND a damaged/missing import record both refuse the
    // boot — the retry lives in the instance detail, not in a degraded launch.
    if let Some(m) = &manifest {
        import_launch_gate(instance_dir, m).await?;
    }
    let workspace = instance_dir.join("workspace");
    for dir in [dsh_home.join("profiles").join(profile), workspace.clone()] {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    if !manifest.as_ref().is_some_and(crate::instances::is_external) {
        // launch_instance holds the same instance lock as plugin writes.
        crate::plugins::install::recover_profile_transaction(
            &dsh_home.join("profiles").join(profile),
        )
        .await?;
    }

    Ok((dsh_home, workspace))
}
