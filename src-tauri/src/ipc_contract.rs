//! Wire-shape contract tests for the IPC boundary.
//!
//! The frontend mirrors every Rust payload by hand (`src/lib/desktop*.ts`,
//! `src/types`), so a renamed or dropped field is invisible to both the Rust
//! tests and `scripts/check-tauri-bridge.mjs` — the latter only proves a
//! command *name* is registered. These tests pin the exact JSON key set and the
//! round-trip behaviour of the types the UI deserialises, on a real instance
//! built by the real backend.
//!
//! What is deliberately NOT here: a live invoke round trip through
//! `tauri::test::mock_builder`. Linking `MockRuntime` into a test binary fails
//! to load on this toolchain (Windows, Rust 1.9x, tauri 2.11.5) with
//! `STATUS_ENTRYPOINT_NOT_FOUND` / `0xc0000139` before any test runs — verified
//! to be triggered by the mere presence of `tauri::test::mock_builder()`, with
//! the `test` feature on either the normal or the dev dependency. The command
//! surface itself is still testable from outside: `build_app` in `lib.rs`
//! registers it for exactly that purpose once the loader issue is solved.

#![cfg(test)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{json, Value};

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-wire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(id: &str, name: &str) -> crate::instances::InstanceManifest {
    serde_json::from_value(json!({
        "id": id,
        "name": name,
        "kind": "sandbox",
        "hue": 200,
        "versionId": "dsh-0.1.0",
        "runtimeId": "node-22",
        "port": 8080,
        "autoPort": true,
        "profile": "default",
        "createdAt": "2026-09-09T00:00:00Z"
    }))
    .expect("the wire manifest parses")
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

/// Mirrors `RemoteInstanceRecord` in `src/lib/desktopInstances.ts`. If a field
/// is added or renamed on either side, this test says so instead of the user.
#[tokio::test]
async fn instance_record_wire_shape_matches_the_typescript_mirror() {
    let root = temp_root("record");
    let record =
        crate::instances::create_instance_inner(root.as_path(), manifest("it-0001", "Wire"))
            .await
            .expect("create");
    let value = serde_json::to_value(&record).expect("serialize");

    assert_eq!(
        keys(&value),
        set(&[
            "schemaVersion",
            "id",
            "name",
            "note",
            "kind",
            "hue",
            "versionId",
            "runtimeId",
            "port",
            "autoPort",
            "profile",
            "createdAt",
            "lastRunAt",
            "totalRuntime",
            "favorite",
            "env",
            "args",
            "api",
            "managementMode",
            "source",
            "externalHome",
            "adoptedFrom",
            "dshHome",
            "workspace",
            "plugins",
            "snapshots",
        ]),
        "InstanceRecord drifted from the TS mirror"
    );

    // Every key is camelCase: a snake_case leak means the frontend reads
    // `undefined` and silently falls back to a default.
    for key in keys(&value) {
        assert!(!key.contains('_'), "non-camelCase field on the wire: {key}");
    }

    // The derived fields the UI relies on, with their real values. The create
    // response is the in-memory manifest, so its `schemaVersion` is still 0 —
    // `write_manifest` stamps the current version on write and re-stamps on
    // every later write, so echoing either value back is harmless. A listing
    // reads the file, which is where the stamp shows up.
    assert_eq!(value["schemaVersion"], 0);
    let listed = crate::instances::list_instances_inner(root.as_path())
        .await
        .expect("list");
    assert_eq!(
        serde_json::to_value(&listed[0]).unwrap()["schemaVersion"],
        2,
        "the on-disk manifest is stamped"
    );
    assert_eq!(value["managementMode"], "managed-copy");
    assert_eq!(value["source"], "created");
    assert_eq!(
        value["dshHome"].as_str().unwrap(),
        root.join("instances")
            .join("it-0001")
            .join("dsh-home")
            .to_string_lossy()
    );
    assert_eq!(value["plugins"], json!([]));
    assert_eq!(value["snapshots"], json!([]));

    let _ = std::fs::remove_dir_all(&root);
}

/// The bug `desktopInstances.ts` documents: `save_instance` round-trips the
/// whole manifest, so dropping the adoption fields would let serde defaults
/// reset an `external` instance to `managed-copy` and orphan its real DSH_HOME.
#[test]
fn manifest_round_trip_keeps_adoption_fields_and_defaults_v1() {
    let external = json!({
        "id": "ext-0001",
        "name": "External",
        "kind": "sandbox",
        "hue": 10,
        "versionId": "dsh-0.1.0",
        "runtimeId": "node-22",
        "port": 8080,
        "autoPort": false,
        "profile": "default",
        "createdAt": "2026-09-09T00:00:00Z",
        "managementMode": "external",
        "source": "adopted",
        "externalHome": "D:/user-dsh",
        "adoptedFrom": { "dshHome": "D:/user-dsh", "adoptedAt": "2026-09-09T00:00:00Z", "mode": "external" }
    });
    let parsed: crate::instances::InstanceManifest =
        serde_json::from_value(external.clone()).expect("parse");
    let again = serde_json::to_value(&parsed).expect("serialize");
    assert_eq!(again["managementMode"], "external");
    assert_eq!(again["source"], "adopted");
    assert_eq!(again["externalHome"], "D:/user-dsh");
    assert_eq!(again["adoptedFrom"]["dshHome"], "D:/user-dsh");

    // A v1 manifest carries none of these and must read as a PHL-owned copy —
    // the documented serde defaults, not an error.
    let v1: crate::instances::InstanceManifest = serde_json::from_value(json!({
        "id": "v1-0001",
        "name": "Old",
        "kind": "sandbox",
        "hue": 0,
        "versionId": "dsh-0.1.0",
        "runtimeId": "node-22",
        "port": 8080,
        "autoPort": true,
        "profile": "default",
        "createdAt": "2026-09-09T00:00:00Z"
    }))
    .expect("v1 parses");
    let v1 = serde_json::to_value(&v1).expect("serialize");
    assert_eq!(v1["managementMode"], "managed-copy");
    assert_eq!(v1["source"], "created");
    assert_eq!(v1["externalHome"], Value::Null);
}

/// The task registry the task centre polls: the list key always present, and
/// the enum-ish `state` spelled the way the TS union expects.
#[test]
fn task_registry_wire_shape_is_stable() {
    let tasks = crate::resources::Tasks::default();
    let list = crate::resources::TaskList { tasks: tasks.list() };
    let value = serde_json::to_value(&list).expect("serialize");
    assert_eq!(keys(&value), set(&["tasks"]));
    assert_eq!(value["tasks"], json!([]));

    // The guard's own record, serialized: the task centre reads these names.
    let info = crate::resources::TaskInfo {
        id: "t-1".into(),
        kind: "instance-create",
        label: "创建实例".into(),
        resources: vec!["instance:it-0001".into()],
        phase: "copying".into(),
        progress: None,
        state: crate::resources::TaskState::Running,
        cancel_requested: false,
        error: None,
        started_at: 0,
        finished_at: None,
    };
    let value = serde_json::to_value(&info).expect("serialize");
    assert_eq!(
        keys(&value),
        set(&[
            "id",
            "kind",
            "label",
            "resources",
            "phase",
            "progress",
            "state",
            "cancelRequested",
            "error",
            "startedAt",
            "finishedAt",
        ])
    );
    assert_eq!(value["state"], "running");
}
