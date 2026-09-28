//! R3-01/R3-02 command-entry tests: the payload the frontend declares, and the
//! retry that must be answered after the plan has committed.
//!
//! Split out of `tests.rs` when the file crossed the size budget; the shared
//! fixtures live there and are `pub(super)` for exactly this reason.

use super::tests::{processes, request, scratch, seed_source, seed_versions, validate_plan};
use super::*;
use std::path::Path;

/* ------------ R3-01/R3-02: frontend payload + command-entry retry ------------ */

/// The payload shape the frontend declares (`src/features/trial/trialPlan.ts`),
/// read from the SAME fixture the vitest test pins, with a real preview's plan
/// substituted in. Driving the command with this file is the cross-language
/// DTO contract: if either side stops sending/accepting a field, one of the
/// two tests fails (R3-01).
fn frontend_request(preview: &TrialPreview) -> TrialRequest {
    let filled = fill_frontend_payload([
        ("{{sourceId}}", preview.source_id.as_str()),
        ("{{targetVersion}}", preview.target_version.as_str()),
        ("{{targetRuntimeId}}", preview.target_runtime_id.as_str()),
        ("{{planId}}", preview.plan_id.as_str()),
        ("{{targetId}}", preview.target_id.as_str()),
        ("{{sourceFingerprint}}", preview.source_fingerprint.as_str()),
    ]);
    let req: TrialRequest = serde_json::from_str(&filled).unwrap();
    // The fixture describes THIS flow, and its key set is exactly the key set
    // Rust serializes: a field renamed on either side fails here, not in
    // production (R3-01).
    assert_eq!(req.source_id, preview.source_id);
    assert_eq!(req.target_version, preview.target_version);
    assert_eq!(req.target_runtime_id, preview.target_runtime_id);
    assert_eq!(req.scope, preview.scope);
    assert_eq!(req.workspace, preview.workspace);
    assert_eq!(req.plan_id, preview.plan_id);
    assert_eq!(req.target_id, preview.target_id);
    assert_eq!(req.source_fingerprint, preview.source_fingerprint);
    let ours: Vec<String> = serde_json::to_value(&req)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let declared: Vec<String> = serde_json::from_str::<serde_json::Value>(&filled)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !k.starts_with('_'))
        .cloned()
        .collect();
    assert_eq!(
        ours, declared,
        "the Rust DTO and the frontend's declared payload must agree on every field"
    );
    req
}

/// Fills the shared payload fixture. Both sides substitute their own values;
/// the KEYS are the contract, the values are the caller's.
fn fill_frontend_payload<const N: usize>(values: [(&str, &str); N]) -> String {
    let mut filled = include_str!("fixtures/trial-create-request.json").to_string();
    for (token, value) in values {
        filled = filled.replace(token, value);
    }
    assert!(
        !filled.contains("{{"),
        "every placeholder in the shared payload must be filled: {filled}"
    );
    filled
}

fn transfer_locks() -> (
    crate::versions::Transfers,
    crate::resources::ResourceLocks,
    crate::resources::Tasks,
) {
    (
        crate::versions::Transfers::default(),
        crate::resources::ResourceLocks::default(),
        crate::resources::Tasks::default(),
    )
}

fn instance_dirs(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(instances_root(root))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    out.sort();
    out
}

/// R3-01, turned positive: the payload the frontend declares — filled from a
/// REAL preview — drives the real command entry to a committed copy. The
/// reviewer's counterexample was this same payload with the plan fields
/// dropped, which the backend refused with 「缺少试升级计划」.
#[tokio::test]
async fn frontend_create_payload_reaches_a_committed_copy() {
    let root = scratch("r3-frontend");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);

    let (transfers, locks, tasks) = transfer_locks();
    let outcome = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    assert!(outcome.committed);
    assert_eq!(
        outcome.record.manifest.name, "review",
        "the frontend's name travelled"
    );
    assert_eq!(outcome.record.manifest.version_id, "dsh-0.1.7-rc.2");
    assert_eq!(
        outcome.record.manifest.port,
        u32::from(preview.allocated_port),
        "the copy keeps the port its plan allocated"
    );
    assert!(instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: submit, lose the response, submit the SAME request again. The second
/// create must report the committed copy — the round-3 counterexample failed
/// here because validate_plan re-allocated the port, the copy's own
/// registration moved it, and the plan hash changed.
#[tokio::test]
async fn retry_after_a_lost_response_reports_the_committed_copy() {
    let root = scratch("r3-retry");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);
    let (transfers, locks, tasks) = transfer_locks();

    let first = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(first.committed);

    let second = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .expect("the same plan must still validate after it committed");
    assert!(second.committed);
    assert_eq!(second.readiness, first.readiness, "recorded, not invented");
    assert!(
        second.notes.iter().any(|n| n.contains("已存在")),
        "the retry says it created nothing: {:?}",
        second.notes
    );
    assert_eq!(
        instance_dirs(&root),
        vec!["daily".to_string(), req.target_id.clone()],
        "exactly one copy: the source plus this plan's target"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: a restart forgets every in-memory structure. The retry is answered
/// from the plan record + the copy's own marker, both on disk.
#[tokio::test]
async fn retry_after_a_restart_still_reports_the_committed_copy() {
    let root = scratch("r3-restart");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);
    create_trial_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    // The plan record survived the "restart" on disk.
    let plan_file = root
        .join("trial-plans")
        .join(format!("{}.json", req.plan_id));
    assert!(plan_file.exists(), "the issued plan is persisted");

    let outcome = create_trial_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(outcome.committed);
    assert_eq!(
        instance_dirs(&root),
        vec!["daily".to_string(), req.target_id.clone()],
        "exactly one copy: the source plus this plan's target"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: the copy is already RUNNING (it owns its port in the process table).
/// Its plan must not expire because of its own footprint.
#[tokio::test]
async fn retry_while_the_copy_is_running_still_reports_it() {
    let root = scratch("r3-running");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);
    let (transfers, locks, tasks) = transfer_locks();
    create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    // The copy is up: registered in PHL's process table on its own port.
    let processes = processes();
    processes.0.lock().unwrap().insert(
        req.target_id.clone(),
        crate::launch::ProcessEntry {
            pid: 4321,
            port: preview.allocated_port,
        },
    );
    let outcome = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes,
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(outcome.committed);
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: a committed copy is only reused when the request still describes it.
/// The same plan id asking for a different target version is a different plan
/// wearing this one's identity — refused, never reported as its result.
#[tokio::test]
async fn committed_copy_with_mismatched_identity_refuses() {
    let root = scratch("r3-mismatch");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);
    let (transfers, locks, tasks) = transfer_locks();
    create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    let mut tampered = req.clone();
    tampered.target_version = "0.1.5-rc.3".into(); // installed, but not what was committed
    let err = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        tampered,
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("已提交副本与当前计划参数不一致"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: the plan record is the identity authority for a plan that has NOT
/// committed; a request whose record is gone refuses instead of being rebuilt
/// from live state.
#[tokio::test]
async fn a_plan_record_that_vanished_refuses_at_the_command_entry() {
    let root = scratch("r3-noplan");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);
    std::fs::remove_file(
        root.join("trial-plans")
            .join(format!("{}.json", req.plan_id)),
    )
    .unwrap();

    let (transfers, locks, tasks) = transfer_locks();
    let err = create_trial_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes(),
        format!("t{}", crate::versions::now_millis()),
        req.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("计划不存在或已过期"), "{err}");
    assert!(!instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// R3-02: a fresh plan whose port another instance has since claimed refuses
/// with "re-preview" — the check reads configured ports and PHL's process
/// table, never a bind probe, so a create cannot be failed by transient
/// machine state (or by the plan's own copy once it has committed).
#[tokio::test]
async fn a_plan_port_claimed_by_another_instance_refuses() {
    let root = scratch("r3-portclaim");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let req = frontend_request(&preview);

    // Another instance takes the port the plan allocated, after the preview.
    let other = instance_dir(&root, "other").unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let manifest: InstanceManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2,
        "id": "other",
        "name": "另一个实例",
        "kind": "sandbox",
        "hue": 0,
        "versionId": "dsh-0.1.7-rc.2",
        "runtimeId": "node-system",
        "port": preview.allocated_port,
        "autoPort": false,
        "profile": "web",
        "createdAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    super::super::manifest::write_manifest(&other, &manifest)
        .await
        .unwrap();

    let err = validate_plan(&root, &processes(), &req).await.unwrap_err();
    assert!(err.contains("已被其他实例占用"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
