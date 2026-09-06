use super::*;
use crate::pack::write::PackBuilder;
use serde_json::json;
use std::path::PathBuf;

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-packinstall-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pack with one embedded plugin (with package.json + a source file) and one
/// session, plus an environment section, built exactly as the exporter would.
fn build_sample_pack(dest: &Path) {
    let mut b = PackBuilder::create(dest).unwrap();
    b.add_bytes(
        "embedded/plugins/mine/package.json",
        br#"{"name":"mine","version":"0.1.0"}"#,
    )
    .unwrap();
    b.add_bytes("embedded/plugins/mine/index.js", b"export default 1")
        .unwrap();
    let header =
        json!({"type":"session","version":0,"id":"session-abc","createdAt":1,"cwd":"C:/x"});
    b.add_bytes(
        "sessions/--P--/session-abc/session.jsonl",
        format!("{header}\n").as_bytes(),
    )
    .unwrap();
    let env = json!({
        "phlBundle": 2,
        "exportedAt": "now",
        "instance": {
            "id":"x","name":"x","kind":"sandbox","hue":0,"versionId":"0.1.2","runtimeId":"node-22",
            "port":8080,"autoPort":false,"profile":"web","createdAt":"now","env":{
                "DEEPSEEK_API_KEY":"sk-should-not-travel","APP_MODE":"prod"
            },"args":[],"managementMode":"managed-copy","source":"created"
        },
        "plugins": [],
        "credentials": ["DEEPSEEK_API_KEY"]
    });
    let mut manifest: crate::pack::PhlPackManifest = serde_json::from_value(json!({
        "formatVersion": 1,
        "pack": {"id":"demo","name":"Demo","version":"1.0.0"},
        "dsh": {"version":"0.1.2"},
        "runtime": {"kind":"node","nodeVersion":"node-22"},
        "plugins": [{"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}],
        "content": {"sessionsIncluded": true, "sessionCount": 1, "secretsExcluded": true}
    }))
    .unwrap();
    manifest.environment = Some(crate::pack::EnvironmentSection(env));
    b.finish(manifest).unwrap();
}

#[test]
fn unpack_lands_plugins_sessions_and_confines_writes() {
    let dir = root("unpack");
    let pack = dir.join("s.phlpack");
    build_sample_pack(&pack);
    let validated = crate::pack::read_pack_from_path(&pack).unwrap();
    assert_eq!(validated.embedded_plugins, vec!["mine".to_string()]);

    let home = dir.join("home");
    let profile = home.join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules")).unwrap();
    let out = unpack_pack(pack.to_str().unwrap(), &profile, &home).unwrap();
    assert_eq!(out.sessions, 1, "the session artifact counted");
    assert!(profile
        .join("node_modules")
        .join("mine")
        .join("package.json")
        .is_file());
    assert!(profile
        .join("node_modules")
        .join("mine")
        .join("index.js")
        .is_file());
    assert!(home
        .join("sessions")
        .join("--P--")
        .join("session-abc")
        .join("session.jsonl")
        .is_file());
    // phlpack.json is NOT extracted into the home.
    assert!(!home.join("phlpack.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn core_extractor_refuses_a_map_escaping_its_root() {
    // The extraction confinement rule now lives in `phl-pack-core` (spec §22);
    // this is the host-side proof that the installer's chosen roots are not
    // just convention: even a (hypothetically) buggy map that names a dest
    // outside its root is refused before any byte is written.
    let dir = root("confine");
    let pack = dir.join("c.phlpack");
    build_sample_pack(&pack);
    let escaped = dir.join("escape-target");
    let outside = dir.parent().unwrap().join("definitely-outside-phl-install");
    let result = super::super::unpack::unpack_entries_to(&pack, |rel| {
        // Map everything, but under a root that is NOT the dest's container.
        let _ = rel;
        Some((outside.join("victim"), escaped.clone()))
    });
    assert!(
        matches!(&result, Err(phl_pack_core::PackError::PathTraversal(_))),
        "escaping destination must be refused: {result:?}",
    );
    assert!(!outside.join("victim").exists(), "nothing was written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn resolver_classifies_embedded_and_registry_dependencies() {
    let dir = root("resolver");
    let pack = dir.join("r.phlpack");
    {
        let mut b = PackBuilder::create(&pack).unwrap();
        b.add_bytes("embedded/plugins/mine/package.json", b"{}")
            .unwrap();
        let m: crate::pack::PhlPackManifest = serde_json::from_value(json!({
            "formatVersion": 1,
            "pack": {"id":"d","name":"D","version":"1"},
            "dsh": {"version":"0.1.2"},
            "runtime": {"nodeVersion":"node-22"},
            "plugins": [
                {"id":"mine","version":"0.1","source":{"type":"embedded","path":"embedded/plugins/mine"}},
                {"id":"remote-a","version":"1.2","source":{"type":"registry","registryId":"remote-a"}}
            ],
            "content": {"sessionsIncluded": false}
        }))
        .unwrap();
        b.finish(m).unwrap();
    }
    let validated = crate::pack::read_pack_from_path(&pack).unwrap();
    let (deps, blocked) = resolve_dependencies(&dir, &validated).await;
    // A well-formed pack is never blocked: embedded plugins carry their bytes
    // and every registry plugin has a registryId, so both are resolvable.
    assert!(!blocked);
    assert!(deps
        .iter()
        .any(|d| d.id == "mine" && d.status == DependencyStatus::Embedded));
    assert!(deps
        .iter()
        .any(|d| d.id == "remote-a" && d.status == DependencyStatus::Downloadable));
    // The missing DSH version surfaces as Downloadable with a note — the spec
    // §19-20 UX, not a hard block on creating the instance.
    assert!(deps
        .iter()
        .any(|d| d.kind == "dsh" && d.status == DependencyStatus::Downloadable));
    assert!(deps.iter().any(|d| d.kind == "dsh" && d.note.is_some()));
    let _ = std::fs::remove_dir_all(&dir);
}
