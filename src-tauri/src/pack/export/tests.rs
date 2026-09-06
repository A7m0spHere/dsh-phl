use super::*;
use crate::instances::manifest::write_manifest;
use crate::instances::{InstanceManifest, InstanceSource, ManagementMode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-packexport-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A managed-copy instance with: an npm-resolvable plugin (`remote-a`), a
/// local no-registry plugin (`mine`), a credential env var, and one session.
async fn seed_instance(r: &Path, id: &str) {
    let dir = r.join("instances").join(id);
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules").join("remote-a")).unwrap();
    std::fs::create_dir_all(profile.join("node_modules").join("mine")).unwrap();
    std::fs::write(
        profile.join("node_modules").join("remote-a").join("phl-plugin.json"),
        br#"{"pluginId":"remote-a","version":"1.2.0","registryId":"remote-a","kind":"npm","trust":"verified"}"#,
    )
    .unwrap();
    std::fs::write(
        profile
            .join("node_modules")
            .join("remote-a")
            .join("package.json"),
        br#"{"name":"remote-a","license":"MIT"}"#,
    )
    .unwrap();
    // `mine` is installed under a folder but unverified (a local build) and has
    // no license file → embed-recommended, license-unknown.
    std::fs::write(
        profile.join("node_modules").join("mine").join("phl-plugin.json"),
        br#"{"pluginId":"who/mine","version":"0.1.0","registryId":"mine","kind":"npm","trust":"unverified"}"#,
    )
    .unwrap();
    // A real plugin always ships a package.json; the embedded-plugin integrity
    // check (P1-2) relies on it, so the fixture must too.
    std::fs::write(
        profile
            .join("node_modules")
            .join("mine")
            .join("package.json"),
        br#"{"name":"mine","version":"0.1.0"}"#,
    )
    .unwrap();
    std::fs::write(
        profile.join("node_modules").join("mine").join("index.js"),
        b"export default 1",
    )
    .unwrap();
    // The plugin list also needs the profile to be discoverable: a package.json
    // + patch so `scan_plugins` sees them as installed (markers already do).
    let mut env = HashMap::new();
    env.insert("DEEPSEEK_API_KEY".to_string(), "sk-secret-123".to_string());
    env.insert("APP_MODE".to_string(), "prod".to_string());
    let manifest = InstanceManifest {
        schema_version: 2,
        id: id.into(),
        name: "Exportable".into(),
        note: None,
        kind: "sandbox".into(),
        hue: 0,
        version_id: "0.1.2-rc.1".into(),
        runtime_id: "node-22".into(),
        port: 8080,
        auto_port: false,
        profile: "web".into(),
        created_at: "now".into(),
        last_run_at: None,
        total_runtime: 0,
        favorite: false,
        env,
        args: Vec::new(),
        api: None,
        management_mode: ManagementMode::ManagedCopy,
        source: InstanceSource::Created,
        external_home: None,
        adopted_from: None,
    };
    write_manifest(&dir, &manifest).await.unwrap();
}

fn opts(embed: Vec<String>, sessions: bool, ack: bool) -> PackExportOptions {
    PackExportOptions {
        embed_registry_ids: embed,
        include_sessions: sessions,
        sessions_privacy_ack: ack,
    }
}

#[tokio::test]
async fn plan_classifies_remote_local_strips_secrets_and_flags_license() {
    let r = root("plan");
    std::fs::create_dir_all(r.join("instances")).unwrap();
    seed_instance(&r, "ex1").await;
    let plan = build_plan(&r, "ex1").await.unwrap();

    let remote = plan
        .plugins
        .iter()
        .find(|p| p.registry_id == "remote-a")
        .unwrap();
    assert!(remote.registry_available && !remote.embed_recommended);
    assert_eq!(remote.license.as_deref(), Some("MIT"));
    assert!(!remote.license_unknown);

    let mine = plan
        .plugins
        .iter()
        .find(|p| p.plugin_id == "who/mine")
        .unwrap();
    assert!(!mine.registry_available && mine.embed_recommended);
    assert!(mine.license_unknown);

    // The API key name is reported as stripped; its VALUE must not be in the env.
    assert!(plan
        .credential_names
        .contains(&"DEEPSEEK_API_KEY".to_string()));
    assert!(plan.warnings.iter().any(|w| w.contains("许可证")));
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn export_roundtrips_strips_secrets_embeds_and_is_validator_clean() {
    let r = root("round");
    std::fs::create_dir_all(r.join("instances")).unwrap();
    seed_instance(&r, "ex2").await;
    let dest = r.join("out.phlpack");
    let report = export_pack_inner(
        &r,
        "ex2".into(),
        dest.to_string_lossy().into_owned(),
        // Embed the unverified local plugin by its install id; ship remote-a as
        // a registry record.
        opts(vec!["mine".to_string()], false, false),
    )
    .await
    .unwrap();
    assert_eq!(report.plugin_count, 2);
    assert_eq!(report.embedded_count, 1);
    assert!(!report.sessions_included);
    assert!(report
        .credential_names
        .contains(&"DEEPSEEK_API_KEY".to_string()));

    // The validator (P1-2) accepts it, so we never ship something install refuses.
    let pack = crate::pack::read_pack_from_path(&dest).expect("self-export validates");
    assert_eq!(pack.embedded_plugins, vec!["mine".to_string()]);
    assert!(!pack.has_sessions);
    // integrity covered the embedded payload
    let integrity = pack.manifest.integrity.unwrap();
    assert!(integrity.keys().any(|k| k.starts_with("embedded/plugins/")));

    // The secret value must not appear anywhere in the archive bytes.
    let bytes = std::fs::read(&dest).unwrap();
    assert!(
        !bytes
            .windows(b"sk-secret-123".len())
            .any(|w| w == b"sk-secret-123"),
        "an exported pack must never carry a credential value"
    );
    // The env section still carries the non-secret variable.
    let env = pack.manifest.environment.unwrap().0;
    let app_mode = env
        .pointer("/instance/env/APP_MODE")
        .and_then(|v| v.as_str());
    assert_eq!(app_mode, Some("prod"));
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn sessions_require_the_privacy_ack_and_are_packed_when_given() {
    let r = root("sessions");
    std::fs::create_dir_all(r.join("instances")).unwrap();
    seed_instance(&r, "ex3").await;
    // Add one real session artifact under the home.
    let sess = r
        .join("instances")
        .join("ex3")
        .join("dsh-home")
        .join("sessions")
        .join("--P--")
        .join("session-abc");
    std::fs::create_dir_all(&sess).unwrap();
    let header = serde_json::json!({"type":"session","version":0,"id":"session-abc","createdAt":1,"cwd":"C:/x"}).to_string();
    std::fs::write(sess.join("session.jsonl"), format!("{header}\n")).unwrap();

    // Asking for sessions without acknowledging privacy is refused.
    let err = export_pack_inner(
        &r,
        "ex3".into(),
        r.join("nope.phlpack").to_string_lossy().into_owned(),
        opts(vec!["mine".to_string()], true, false),
    )
    .await
    .unwrap_err();
    assert!(err.contains("隐私"), "got {err}");

    // Acknowledged → the pack ships sessions and says so.
    let dest = r.join("with-sessions.phlpack");
    let report = export_pack_inner(
        &r,
        "ex3".into(),
        dest.to_string_lossy().into_owned(),
        opts(vec!["mine".to_string()], true, true),
    )
    .await
    .unwrap();
    assert!(report.sessions_included);
    let pack = crate::pack::read_pack_from_path(&dest).unwrap();
    assert!(pack.has_sessions);
    assert!(pack.manifest.content.sessions_included);
    let _ = std::fs::remove_dir_all(&r);
}
