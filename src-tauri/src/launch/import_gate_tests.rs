//! R3-03: the community-import launch gate must refuse a boot for a
//! **missing, unreadable or unrecognisable** import record, not only for a
//! pending dependency install.
//!
//! Every case below drives the real gate (`launch::import_launch_gate`) over
//! an instance written to disk, so a regression in the state projection
//! cannot hide behind a read helper returning `None` — the reviewer's
//! counterexample was exactly that: the marker read returned `None` and the
//! gate let an instance with uninstalled dependencies boot.

use std::path::{Path, PathBuf};

use super::import_launch_gate;
use crate::instances::manifest::write_manifest;
use crate::instances::{ImportState, InstanceManifest};

const ID: &str = "pack-gate";

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-import-gate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One instance directory with the given provenance mode — the manifest
/// shape the community installer writes for `community-import`, and the one
/// `.phlpack` / trial flows write for their own modes.
async fn instance_with_mode(root: &Path, mode: &str) -> PathBuf {
    let dir = crate::instances::instance_dir(root, ID).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let manifest: InstanceManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2,
        "id": ID,
        "name": "整合包实例",
        "kind": "sandbox",
        "hue": 0,
        "versionId": "dsh-0.1.7-rc.2",
        "runtimeId": "node-system",
        "port": 8300,
        "autoPort": false,
        "profile": "web",
        "createdAt": "2026-01-01T00:00:00Z",
        "managementMode": "pack-installed",
        "source": "phlpack",
        "adoptedFrom": {
            "dshHome": "dspack:review@1",
            "detectedVersion": "0.1.7-rc.2",
            "adoptedAt": "2026-01-01T00:00:00Z",
            "mode": mode,
        }
    }))
    .unwrap();
    write_manifest(&dir, &manifest).await.unwrap();
    dir
}

async fn import_instance(root: &Path) -> PathBuf {
    instance_with_mode(root, "community-import").await
}

fn write_marker(dir: &Path, raw: &str) {
    std::fs::write(dir.join("phl-import.json"), raw).unwrap();
}

async fn gate(dir: &Path) -> Result<(), String> {
    let manifest = crate::instances::load_manifest(dir, ID).await.unwrap();
    import_launch_gate(dir, &manifest).await
}

async fn state(dir: &Path) -> ImportState {
    let manifest = crate::instances::load_manifest(dir, ID).await.unwrap();
    crate::instances::read_import_state(dir, Some(&manifest)).await
}

/// The retry may be ATTEMPTED for exactly the states the gate blocks; an
/// instance with no import history is not a retry target at all. (Attempt ≠
/// repair: a damaged record refuses inside the attempt — see R4-01. Only
/// `Pending` records can actually be completed in place.)
#[tokio::test]
async fn retry_target_covers_every_blocked_state_and_nothing_else() {
    let root = scratch("repairable");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"needsDependencies"}"#);
    assert!(state(&dir).await.is_import_target());

    write_marker(&dir, "{broken");
    assert!(state(&dir).await.is_import_target());

    std::fs::remove_file(dir.join("phl-import.json")).unwrap();
    assert!(state(&dir).await.is_import_target());

    // No import history at all: not a community import, so not repairable.
    let plain = instance_with_mode(&root, "trial-copy").await;
    assert!(!state(&plain).await.is_import_target());
    let _ = std::fs::remove_dir_all(&root);
}

/// R4-01: the failure reasons the record holds ride on the instance record, so
/// the retry card can show them after a reload — a failure must not live only
/// in a toast. They are read from the import record, never invented.
#[tokio::test]
async fn the_record_carries_the_last_failure_reasons() {
    let root = scratch("record-failures");
    let dir = import_instance(&root).await;
    write_marker(
        &dir,
        r#"{"stage":"needsDependencies","dependenciesFailed":["依赖 a 安装失败","npm 不可用"]}"#,
    );
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert_eq!(
        record.import_failures,
        vec!["依赖 a 安装失败".to_string(), "npm 不可用".to_string()]
    );

    // A healthy record carries none …
    write_marker(&dir, r#"{"stage":"ready"}"#);
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert!(record.import_failures.is_empty());

    // … and a torn record cannot claim reasons it does not have.
    write_marker(&dir, "{broken");
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert!(record.import_failures.is_empty());
    assert!(matches!(state(&dir).await, ImportState::Corrupt(_)));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn ready_marker_boots() {
    let root = scratch("ready");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"ready"}"#);
    assert!(gate(&dir).await.is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn pending_dependencies_refuse_the_boot() {
    let root = scratch("pending");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"needsDependencies"}"#);
    let err = gate(&dir).await.unwrap_err();
    assert!(err.contains("依赖尚未安装完成"), "{err}");
    assert!(
        err.contains("重试依赖安装"),
        "the refusal names the repair: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The reviewer's counterexample: delete the record and the gate used to lift.
#[tokio::test]
async fn deleted_marker_refuses_the_boot() {
    let root = scratch("deleted");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"needsDependencies"}"#);
    std::fs::remove_file(dir.join("phl-import.json")).unwrap();
    let err = gate(&dir).await.unwrap_err();
    assert!(err.contains("记录缺失"), "{err}");
    assert!(err.contains("重试依赖安装"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn corrupt_marker_refuses_the_boot() {
    let root = scratch("corrupt");
    let dir = import_instance(&root).await;
    write_marker(&dir, "{broken");
    let err = gate(&dir).await.unwrap_err();
    assert!(err.contains("记录损坏"), "{err}");
    assert!(err.contains("重试依赖安装"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A marker that exists but cannot be READ — the real permission-denial case,
/// via an explicit deny ACE on Windows — must block exactly like a damaged one.
#[cfg(windows)]
#[tokio::test]
async fn permission_denied_marker_refuses_the_boot() {
    let root = scratch("denied");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"ready"}"#);
    let marker = dir.join("phl-import.json");
    let user = std::env::var("USERNAME").unwrap_or_default();
    let icacls = |args: &[&str]| {
        std::process::Command::new("icacls")
            .arg(&marker)
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !icacls(&["/deny", &format!("{user}:(R)")]) {
        eprintln!("skipping: icacls could not set a deny ACE (no local ACL support)");
        return;
    }
    let result = gate(&dir).await;
    // Restore first: the temp tree must stay removable whatever the verdict.
    let _ = icacls(&["/remove:d", &user]);
    let err = result.unwrap_err();
    assert!(err.contains("记录不可读取"), "{err}");
    assert!(err.contains("重试依赖安装"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// An unreadable record (here: a directory occupies the marker's path, which
/// fails the read exactly as a permission denial does) must not read as
/// "nothing to report".
#[tokio::test]
async fn unreadable_marker_refuses_the_boot() {
    let root = scratch("unreadable");
    let dir = import_instance(&root).await;
    std::fs::create_dir_all(dir.join("phl-import.json")).unwrap();
    let err = gate(&dir).await.unwrap_err();
    assert!(err.contains("记录不可读取"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn unknown_stage_refuses_the_boot() {
    let root = scratch("unknown");
    let dir = import_instance(&root).await;
    write_marker(&dir, r#"{"stage":"halfway"}"#);
    let err = gate(&dir).await.unwrap_err();
    assert!(err.contains("stage 无法识别"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Backwards compatibility: instances that never owned an import record keep
/// launching, whether they are a `.phlpack` install or a trial copy.
#[tokio::test]
async fn instances_without_import_provenance_boot_without_a_marker() {
    let root = scratch("compat");
    let pack = instance_with_mode(&root, "pack-installed").await;
    assert!(gate(&pack).await.is_ok());
    let trial = instance_with_mode(&root, "trial-copy").await;
    assert!(gate(&trial).await.is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

/// One judgement for every consumer: the instance record the list and detail
/// pages read reports the damaged record as `corruptImport` — the value the
/// UI turns into the retry entry — instead of omitting a readiness field.
#[tokio::test]
async fn the_record_reports_the_same_damaged_state() {
    let root = scratch("record");
    let dir = import_instance(&root).await;
    write_marker(&dir, "{broken");
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert_eq!(record.readiness.as_deref(), Some("corruptImport"));

    write_marker(&dir, r#"{"stage":"needsDependencies"}"#);
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert_eq!(record.readiness.as_deref(), Some("needsDependencies"));

    std::fs::remove_file(dir.join("phl-import.json")).unwrap();
    let manifest = crate::instances::load_manifest(&dir, ID).await.unwrap();
    let record = crate::instances::build_record(&dir, manifest).await;
    assert_eq!(record.readiness.as_deref(), Some("corruptImport"));

    // A plain instance still reports nothing at all.
    let plain = instance_with_mode(&root, "trial-copy").await;
    write_marker(&plain, r#"{"stage":"ready"}"#);
    let manifest = crate::instances::load_manifest(&plain, ID).await.unwrap();
    let record = crate::instances::build_record(&plain, manifest).await;
    assert_eq!(record.readiness.as_deref(), Some("readyToLaunch"));
    let _ = std::fs::remove_dir_all(&root);
}
