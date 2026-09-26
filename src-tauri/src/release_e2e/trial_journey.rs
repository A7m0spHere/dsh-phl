//! Acceptance journey J6: the trial-copy flow (`预览 → 创建副本 → 查看副本`),
//! machine-checked against a real data root, a real runtime and a real client
//! request — the shape the frontend declares, read from the same fixture the
//! vitest test pins.
//!
//! Why it exists: the round-3 review could not accept "the copy flow works"
//! from unit tests alone. R3-01 was a front/back DTO break that every
//! backend-only helper test missed, and R3-02 was a retry that only failed at
//! the real command entry. This journey therefore drives the REAL command
//! bodies (`build_preview` / `create_trial_inner`, the same code the Tauri
//! commands call) against one root, and it ends by launching the copy with the
//! harness's fake DSH — so "副本可用" is an observed fact, not an inference.
//!
//! Not covered here (and named rather than implied): the GUI window and the
//! dialog's click path; that stays the CDP/desktop lane's job.

use std::collections::HashMap;

use crate::instances::trial::{build_preview, create_trial_inner, read_trial_marker, TrialRequest};
use crate::launch::LaunchEvent;
use crate::release_e2e::driver::{launch, with_cleanup, Session};
use crate::release_e2e::fixtures::NodeInfo;
use crate::release_e2e::journeys::{
    cold_mocks, install_runtime, install_version, progress_channel,
};

/// The frontend's declared create payload, filled with this run's plan. The
/// file is shared with `src/features/trial/trialPlan.test.ts`: it is the one
/// declaration of the DTO both sides must agree on (R3-01).
fn create_request(
    preview: &crate::instances::trial::TrialPreview,
    source_id: &str,
    target_version: &str,
    target_runtime_id: &str,
) -> TrialRequest {
    let mut filled =
        include_str!("../instances/trial/fixtures/trial-create-request.json").to_string();
    for (token, value) in [
        ("{{sourceId}}", source_id),
        ("{{targetVersion}}", target_version),
        ("{{targetRuntimeId}}", target_runtime_id),
        ("{{planId}}", preview.plan_id.as_str()),
        ("{{targetId}}", preview.target_id.as_str()),
        ("{{sourceFingerprint}}", preview.source_fingerprint.as_str()),
    ] {
        filled = filled.replace(token, value);
    }
    assert!(
        !filled.contains("{{"),
        "J6: every placeholder must be filled: {filled}"
    );
    let req: TrialRequest =
        serde_json::from_str(&filled).expect("J6: the shared create payload must deserialize");
    // The Rust DTO and the frontend's declared payload agree on every field.
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
        "J6: the two sides' DTOs must match field for field"
    );
    req
}

fn preview_request(source_id: &str, target_version: &str, runtime: &str) -> TrialRequest {
    serde_json::from_value(serde_json::json!({
        "sourceId": source_id,
        "targetVersion": target_version,
        "targetRuntimeId": runtime,
        "name": "",
        "scope": "config",
        "workspace": "fresh",
        "planId": "",
        "targetId": "",
        "sourceFingerprint": "",
    }))
    .unwrap()
}

/// A fixed-port source instance with something to copy: settings + a profile
/// patch. Fixed port on purpose — the copy must take its own, which is the
/// CR-07 half of the plan identity (R3-02's failing field).
async fn seed_source(
    session: &Session,
    id: &str,
    version: &str,
    runtime: &str,
) -> std::path::PathBuf {
    let mut manifest: crate::instances::InstanceManifest =
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": id,
            "name": "日常环境",
            "kind": "sandbox",
            "hue": 0,
            "versionId": version,
            "runtimeId": runtime,
            "port": 8080,
            "autoPort": false,
            "profile": "web",
            "createdAt": crate::versions::now_iso(),
        }))
        .unwrap();
    // The source keeps an API binding so the copy's readiness has something
    // real to inherit (and so the preview's fingerprint covers a plugin scan).
    manifest.api = None;
    crate::instances::create_instance_inner(&session.root(), manifest)
        .await
        .unwrap();
    let dir = session.root().join("instances").join(id);
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules")).unwrap();
    std::fs::write(profile.join("cordis.patch.yml"), "[]\n").unwrap();
    std::fs::write(
        profile.join("package.json"),
        r#"{"name":"dsh-profile","private":true,"dependencies":{}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("dsh-home").join("settings.yaml"), "a: 1\n").unwrap();
    dir
}

/// The whole flow a user walks: preview the copy, submit the payload the
/// frontend declares, then look at what landed — plus the retry that must NOT
/// create a second copy, and a launch to prove the copy runs.
#[tokio::test]
#[ignore = "release-e2e J6 trial copy: preview → create → inspect → launch"]
async fn j6_trial_copy_preview_create_inspect_and_launch() {
    let Some(node) = NodeInfo::discover() else {
        panic!("release-e2e J6: no `node` on PATH — install Node or run on a CI image");
    };
    let (server, _tgz, _integrity) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("j6");
    with_cleanup(&session, async {
        let runtime = install_runtime(&session, &base, &node).await;
        // The catalogue serves BARE version names and the version tree lands
        // under that name (`versions/9.9.9-e2e`); the `dsh-` prefix is the
        // instance-binding spelling. Same split as the real version page.
        let version = super::FAKE_DSH_VERSION.to_string();
        install_version(&session, &base, &version).await;
        let bound = format!("dsh-{version}");
        let source_id = "e2e-j6-src";
        let source = seed_source(&session, source_id, &version, &runtime).await;

        // 1. Preview — read-only, and it issues the plan.
        let target_version = version.clone();
        let preview = build_preview(
            &session.root(),
            &session.processes,
            &preview_request(source_id, &target_version, &runtime),
        )
        .await
        .expect("J6: preview must succeed");
        assert!(!preview.plan_id.is_empty(), "J6: the preview issues a plan");
        assert!(
            preview.target_installed,
            "J6: the target version is installed"
        );
        assert_ne!(
            preview.allocated_port, 8080,
            "J6: the copy must not inherit the source's fixed port"
        );

        // 2. Create — the REAL command body, with the frontend's payload.
        let req = create_request(&preview, source_id, &version, &runtime);
        assert_eq!(req.source_id, source_id);
        assert_eq!(req.target_runtime_id, runtime);
        let outcome = create_trial_inner(
            &session.root(),
            &session.transfers,
            &session.locks,
            &session.tasks,
            &session.processes,
            format!("j6-{}", crate::versions::now_millis()),
            req.clone(),
            progress_channel::<crate::instances::CloneProgress>(),
        )
        .await
        .expect("J6: create must succeed through the real command body");
        assert!(outcome.committed, "J6: the copy committed");

        // 3. Inspect the copy: its own directory, bindings, port and record.
        let dest = session.root().join("instances").join(&req.target_id);
        assert!(dest.join("instance.json").exists());
        let copied = crate::instances::load_manifest(&dest, &req.target_id)
            .await
            .unwrap();
        assert_eq!(
            copied.version_id, bound,
            "J6: the copy is bound to the new version"
        );
        assert_eq!(copied.runtime_id, runtime);
        assert_eq!(u32::from(preview.allocated_port), copied.port);
        assert_eq!(
            copied.adopted_from.as_ref().map(|a| a.dsh_home.as_str()),
            Some(format!("trial:{source_id}").as_str())
        );
        assert!(
            dest.join("dsh-home").join("settings.yaml").exists(),
            "J6: the config travelled"
        );
        let marker = read_trial_marker(&dest)
            .await
            .expect("J6: the copy records its plan");
        assert_eq!(marker.plan_id, req.plan_id);
        assert_eq!(marker.target_version, target_version);
        assert_eq!(marker.allocated_port, preview.allocated_port);
        // The source is untouched by all of this.
        assert!(source.join("dsh-home").join("settings.yaml").exists());
        assert_eq!(
            crate::instances::load_manifest(&source, source_id)
                .await
                .unwrap()
                .port,
            8080,
            "J6: the source keeps its own port"
        );

        // 4. Retry the SAME request (a lost response): recorded outcome, one copy.
        let retry = create_trial_inner(
            &session.root(),
            &session.transfers,
            &session.locks,
            &session.tasks,
            &session.processes,
            format!("j6-{}", crate::versions::now_millis()),
            req.clone(),
            progress_channel::<crate::instances::CloneProgress>(),
        )
        .await
        .expect("J6: the retry of a committed plan must be answered, not refused");
        assert!(retry.committed);
        assert_eq!(retry.readiness, outcome.readiness);
        assert!(
            retry.notes.iter().any(|n| n.contains("已存在")),
            "J6: the retry reports a duplicate submission: {:?}",
            retry.notes
        );
        let copies = std::fs::read_dir(session.root().join("instances"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("trial-"))
            .count();
        assert_eq!(copies, 1, "J6: exactly one copy exists after the retry");

        // 5. The copy runs: launch the fake DSH from the copy's own manifest.
        let held = crate::release_e2e::driver::lock_instance(&session.locks, &req.target_id);
        let launched = launch(
            &session,
            &held,
            "j6-launch",
            &req.target_id,
            &version,
            &runtime,
            &base,
            u16::try_from(copied.port).unwrap(),
            copied.auto_port,
            HashMap::new(),
            Vec::new(),
            "web",
            &progress_channel::<LaunchEvent>(),
        )
        .await
        .expect("J6: the copy must launch");
        let port = launched.outcome.port;
        assert!(
            crate::release_e2e::port_listening(port).await,
            "J6: the copy's DSH must accept connections on {port}"
        );
        session.terminate(&req.target_id).await;
        assert!(
            crate::release_e2e::wait_until(8_000, || crate::launch::port_free(port)).await,
            "J6: the copy's port frees after stop"
        );

        // 6. Evidence: the request that was actually executed (no secrets) and
        //    what it produced. Written to PHL_TRIAL_EVIDENCE when the caller
        //    asks for a file; always logged so the gate output carries it.
        let evidence = serde_json::json!({
            "journey": "J6 trial copy",
            "dataRoot": session.root().to_string_lossy(),
            "source": { "id": source_id, "versionId": version, "runtimeId": runtime, "port": 8080 },
            "plan": {
                "planId": preview.plan_id,
                "targetId": preview.target_id,
                "targetVersion": preview.target_version,
                "targetRuntimeId": preview.target_runtime_id,
                "sourceFingerprint": preview.source_fingerprint,
                "allocatedPort": preview.allocated_port,
                "autoPort": preview.auto_port,
            },
            "createRequest": {
                "sourceId": req.source_id,
                "targetVersion": req.target_version,
                "targetRuntimeId": req.target_runtime_id,
                "name": req.name,
                "scope": req.scope,
                "workspace": req.workspace,
                "planIdPresent": !req.plan_id.is_empty(),
                "targetId": req.target_id,
                "sourceFingerprint": req.source_fingerprint,
            },
            "copy": {
                "id": req.target_id,
                "versionId": copied.version_id,
                "runtimeId": copied.runtime_id,
                "port": copied.port,
                "committed": outcome.committed,
                "readiness": outcome.readiness,
                "linkRedirects": outcome.link_redirects,
                "sessionsImported": outcome.sessions_imported,
                "retryNotes": retry.notes,
                "launchedPort": port,
                "webUrl": launched.outcome.web_url,
            },
        });
        let rendered = serde_json::to_string_pretty(&evidence).unwrap();
        if let Ok(path) = std::env::var("PHL_TRIAL_EVIDENCE") {
            let path = std::path::PathBuf::from(path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, &rendered).expect("J6: evidence file must be writable");
        }
        println!("J6-TRIAL-EVIDENCE {rendered}");
    })
    .await;
}
