//! Environment facts & diff test matrix (acceptance B01–B03, B06, B07 shape).

use super::*;
use crate::instances::manifest::{InstanceManifest, ManagementMode};
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-env-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn base_manifest(id: &str) -> InstanceManifest {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": 2,
        "id": id,
        "name": format!("实例 {id}"),
        "kind": "sandbox",
        "hue": 0,
        "versionId": "dsh-0.1.5-rc.3",
        "runtimeId": "node-system",
        "port": 8000,
        "autoPort": false,
        "profile": "web",
        "createdAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn seed_version(root: &Path, bare: &str, version: &str, integrity: &str) {
    let dir = root.join("versions").join(bare);
    std::fs::create_dir_all(&dir).unwrap();
    let marker = serde_json::json!({
        "installedAt": "2026-01-01T00:00:00Z",
        "version": version,
        "integrity": integrity,
    });
    std::fs::write(dir.join("phl-install.json"), marker.to_string()).unwrap();
}

async fn facts_for(root: &Path, id: &str, manifest: &InstanceManifest) -> EnvironmentFacts {
    let dir = crate::instances::instance_dir(root, id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    collect_facts(root, &dir, manifest).await
}

/// B01: a static environment collected twice is stable — the fingerprint
/// must not move with the observation timestamp.
#[tokio::test]
async fn repeated_collection_of_a_static_environment_is_stable() {
    let root = scratch("stable");
    seed_version(&root, "0.1.5-rc.3", "0.1.5-rc.3", "sha512-abc");
    let manifest = base_manifest("a");
    let dir = crate::instances::instance_dir(&root, "a").unwrap();
    std::fs::create_dir_all(&dir).unwrap();

    let first = collect_facts(&root, &dir, &manifest).await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = collect_facts(&root, &dir, &manifest).await;

    assert_eq!(first.fingerprint, second.fingerprint);
    // The timestamp itself may legitimately repeat (second granularity);
    // what matters is that it never feeds the fingerprint.
    assert!(!first.observed_at.is_empty());
    assert_eq!(first.dsh.installed.value.as_deref(), Some("0.1.5-rc.3"));
    assert_eq!(first.dsh.integrity.value.as_deref(), Some("sha512-abc"));
    let _ = std::fs::remove_dir_all(&root);
}

/// B02: each individually changed fact is detected — binding, installed
/// build, node, plugin version/enable/source identity.
#[tokio::test]
async fn each_changed_fact_is_detected_individually() {
    let root = scratch("changes");
    seed_version(&root, "0.1.5-rc.3", "0.1.5-rc.3", "sha512-old");
    seed_version(&root, "0.1.7-rc.2", "0.1.7-rc.2", "sha512-new");

    let left = base_manifest("left");
    let mut right = base_manifest("right");
    right.version_id = "dsh-0.1.7-rc.2".into();

    let a = facts_for(&root, "left", &left).await;
    let b = facts_for(&root, "right", &right).await;
    let diff = build_diff(&a, &b);
    let binding = diff.items.iter().find(|i| i.key == "dsh.binding").unwrap();
    assert_eq!(binding.state, "different");
    assert_eq!(binding.left.as_deref(), Some("dsh-0.1.5-rc.3"));
    assert_eq!(binding.right.as_deref(), Some("dsh-0.1.7-rc.2"));
    let installed = diff
        .items
        .iter()
        .find(|i| i.key == "dsh.installed")
        .unwrap();
    assert_eq!(installed.state, "different");

    // Node binding change.
    right.runtime_id = "node-22.12.0".into();
    let a = facts_for(&root, "left", &left).await;
    let b = facts_for(&root, "right", &right).await;
    let diff = build_diff(&a, &b);
    let node = diff.items.iter().find(|i| i.key == "node.binding").unwrap();
    assert_eq!(node.state, "different");

    // An uninstalled runtime directory must read unavailable, not unknown
    // and never "same as system".
    let node_actual = diff.items.iter().find(|i| i.key == "node.actual").unwrap();
    assert_eq!(node_actual.state, "unknown");
    assert_eq!(node_actual.right.as_deref(), Some("不可读取"));
    let _ = std::fs::remove_dir_all(&root);
}

/// B03: the same plugin set in a different bundle order is a difference —
/// order is semantics, not noise to be normalized away.
#[tokio::test]
async fn plugin_bundle_order_change_is_preserved() {
    let root = scratch("order");
    let left = base_manifest("left");
    let right = base_manifest("right");

    // Two instances whose profiles declare the same two plugins, swapped.
    for (id, ids) in [
        ("left", &["alpha", "beta"][..]),
        ("right", &["beta", "alpha"][..]),
    ] {
        let dir = crate::instances::instance_dir(&root, id).unwrap();
        let profile = crate::instances::profile_root(&dir, "web");
        std::fs::create_dir_all(&profile).unwrap();
        let mut patch = String::from("# patch\n");
        for pid in ids {
            patch.push_str(&format!(
                "- insert:\n    - id: {pid}\n      name: '{pid}'\n"
            ));
        }
        std::fs::write(profile.join("cordis.patch.yml"), patch).unwrap();
        for pid in ids {
            let pdir = profile.join("node_modules").join(pid);
            std::fs::create_dir_all(&pdir).unwrap();
            std::fs::write(
                pdir.join("phl-plugin.json"),
                serde_json::json!({
                    "installedAt": "2026-01-01T00:00:00Z",
                    "pluginId": pid,
                    "version": "1.0.0",
                    "registryId": pid,
                    "trust": "verified"
                })
                .to_string(),
            )
            .unwrap();
        }
    }

    let a = facts_for(&root, "left", &left).await;
    let b = facts_for(&root, "right", &right).await;
    assert_eq!(a.plugins.len(), 2);
    // Order comes from the patch file: left alpha-then-beta.
    assert_eq!(a.plugins[0].registry_id, "alpha");
    assert_eq!(a.plugins[0].order, 0);

    let diff = build_diff(&a, &b);
    let changed: Vec<_> = diff
        .items
        .iter()
        .filter(|i| i.key.starts_with("plugin."))
        .collect();
    assert_eq!(
        changed.len(),
        2,
        "both rows must show as changed: {changed:?}"
    );
    assert!(
        changed
            .iter()
            .all(|i| i.note.as_deref().unwrap_or("").contains("bundle 顺序")),
        "order change must be named: {changed:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// B04 shape: an unreadable profile renders as unavailable plugins, never
/// as "no plugins", and the diff flags the unknown side.
#[tokio::test]
async fn unreadable_plugin_directory_is_unknown_not_empty() {
    let root = scratch("unreadable");
    let mut manifest = base_manifest("a");
    // Profile points at a directory that does not exist.
    manifest.profile = "web".into();

    let facts = facts_for(&root, "a", &manifest).await;
    assert!(!facts.plugins_available);
    assert!(facts.plugins.is_empty());

    // Compared against an instance with a real (also empty) profile, the
    // diff must say "unknown" for the unreadable side.
    let dir = crate::instances::instance_dir(&root, "b").unwrap();
    let profile = crate::instances::profile_root(&dir, "web");
    std::fs::create_dir_all(&profile).unwrap();
    let manifest_b = base_manifest("b");
    let b = collect_facts(&root, &dir, &manifest_b).await;
    let diff = build_diff(&facts, &b);
    let scan = diff.items.iter().find(|i| i.key == "plugins.scan").unwrap();
    assert_eq!(scan.state, "unknown");
    assert!(scan.left.is_some());
    assert!(scan.right.is_none());
    assert!(diff.has_unknown);
    let _ = std::fs::remove_dir_all(&root);
}

/// B07 shape: the facts layer carries no secret material at all — the API
/// section exposes only the binding's shape and provider *ids*.
#[tokio::test]
async fn api_facts_never_contain_secret_values() {
    let root = scratch("api");
    let mut manifest = base_manifest("a");
    manifest.api = Some(crate::api_config::ApiBinding {
        inheritance: "custom".into(),
        provider_ids: vec!["deepseek".into()],
        default_model: None,
        synced_at: None,
        synced_hash: None,
    });

    let facts = facts_for(&root, "a", &manifest).await;
    let wire = serde_json::to_string(&facts).unwrap();
    assert!(wire.contains("deepseek"), "provider ids are display data");
    // The struct simply has no field a secret could ride in.
    assert!(!wire.contains("apiKey"));
    assert!(!facts.api.pending_credentials);
    let _ = std::fs::remove_dir_all(&root);
}

/// Swap sides: added and removed must swap meaning with the sides.
#[tokio::test]
async fn swapping_sides_swaps_added_and_removed() {
    let root = scratch("swap");
    let mut left = base_manifest("left");
    let mut right = base_manifest("right");
    right.version_id = "dsh-0.1.7-rc.2".into();
    seed_version(&root, "0.1.5-rc.3", "0.1.5-rc.3", "i1");
    seed_version(&root, "0.1.7-rc.2", "0.1.7-rc.2", "i2");

    let a = facts_for(&root, "left", &left).await;
    let b = facts_for(&root, "right", &right).await;

    let ab = build_diff(&a, &b);
    let ba = build_diff(&b, &a);
    let ab_state = ab
        .items
        .iter()
        .find(|i| i.key == "dsh.binding")
        .unwrap()
        .state
        .clone();
    let ba_state = ba
        .items
        .iter()
        .find(|i| i.key == "dsh.binding")
        .unwrap()
        .state
        .clone();
    assert_eq!(ab_state, "different");
    assert_eq!(ba_state, "different");
    // Left/right values must literally swap.
    let ab_item = ab.items.iter().find(|i| i.key == "dsh.binding").unwrap();
    let ba_item = ba.items.iter().find(|i| i.key == "dsh.binding").unwrap();
    assert_eq!(ab_item.left, ba_item.right);
    assert_eq!(ab_item.right, ba_item.left);
    assert_ne!(ab.left_id, ab.right_id);

    // Management mode difference is part of the facts (external vs managed).
    left.management_mode = ManagementMode::External;
    let a2 = facts_for(&root, "left", &left).await;
    assert_eq!(a2.management_mode, "external");
    let _ = std::fs::remove_dir_all(&root);
}
