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

/// Deserialize the `plugins` array exactly as a pack's manifest carries it.
fn manifest_plugins(json: serde_json::Value) -> Vec<PackPlugin> {
    serde_json::from_value(json).unwrap()
}

/// Write one embedded plugin folder into a staging profile the way unpack would
/// (package.json + a source file), and return that folder path.
fn stage_plugin(profile: &Path, folder: &str, package_json: &str) -> PathBuf {
    let dir = profile.join("node_modules").join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.json"), package_json).unwrap();
    std::fs::write(dir.join("index.js"), b"export default 1").unwrap();
    dir
}

#[tokio::test]
async fn embedded_plugin_gets_a_marker_and_cordis_registration() {
    // A CLI-built pack ships only package.json + sources — no PHL marker — yet
    // after install it must show up as installed and be Cordis-registered, or
    // `scan_plugins` silently drops it (§R3 acceptance 1 + 2).
    let dir = root("register");
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules")).unwrap();
    let mine = stage_plugin(&profile, "mine", r#"{"name":"mine","version":"0.1.0"}"#);

    let n = register_embedded_plugins(
        &manifest_plugins(json!([
            {"id":"who/mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}
        ])),
        &profile,
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert!(!dir.join("cordis.patch.yml").exists());

    let marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(mine.join("phl-plugin.json")).unwrap())
            .unwrap();
    // Registry id + version come from the folder's own package.json; pluginId
    // from the manifest; trust honestly `unverified` for a pack-carried plugin.
    assert_eq!(marker["registryId"], "mine");
    assert_eq!(marker["pluginId"], "who/mine");
    assert_eq!(marker["version"], "0.1.0");
    assert_eq!(marker["trust"], "unverified");

    let cordis = std::fs::read_to_string(profile.join("cordis.patch.yml")).unwrap();
    let _: Vec<serde_yaml::Value> =
        serde_yaml::from_str(&cordis).expect("DSH must parse the active profile patch");
    assert!(
        cordis.contains("- id: mine"),
        "cordis entry missing: {cordis}"
    );
    assert!(
        !cordis.contains("disabled: true"),
        "pack-carried plugins default to enabled, not a fabricated state"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn scoped_embedded_plugin_registers_under_its_true_npm_name() {
    // `@scope/name` unpacks to a two-segment folder; registration must key on
    // the real npm name, not the flattened pack folder (§R3 scoped-name case).
    let dir = root("scoped");
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules/@acme")).unwrap();
    let folder = std::path::Path::new("@acme").join("toolkit");
    let staged = stage_plugin(
        &profile,
        &folder.to_string_lossy(),
        r#"{"name":"@acme/toolkit","version":"2.3.4"}"#,
    );

    let n = register_embedded_plugins(
        &manifest_plugins(json!([
            {"id":"acme/toolkit","version":"2.3.4","source":{"type":"embedded","path":"embedded/plugins/@acme/toolkit"}}
        ])),
        &profile,
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert!(!dir.join("cordis.patch.yml").exists());
    let marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(staged.join("phl-plugin.json")).unwrap())
            .unwrap();
    assert_eq!(marker["registryId"], "@acme/toolkit");
    assert_eq!(marker["version"], "2.3.4");
    let cordis = std::fs::read_to_string(profile.join("cordis.patch.yml")).unwrap();
    let _: Vec<serde_yaml::Value> =
        serde_yaml::from_str(&cordis).expect("DSH must parse the active profile patch");
    assert!(
        cordis.contains("- id: '@acme/toolkit'"),
        "scoped entry missing: {cordis}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_embedded_plugin_without_a_manifest_is_a_hard_error() {
    // A declared embedded payload whose folder never landed a package.json is
    // refused — the staging caller then rolls back instead of committing an
    // instance whose plugin DSH would not load (§R3 "登记失败不留半成品").
    let dir = root("missing");
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules").join("ghost")).unwrap();
    // `ghost` folder exists (so it is not a NotFound-canonicalize) but has no
    // package.json at all.
    let result = register_embedded_plugins(
        &manifest_plugins(json!([
            {"id":"ghost","version":"1.0","source":{"type":"embedded","path":"embedded/plugins/ghost"}}
        ])),
        &profile,
    )
    .await;
    assert!(
        result.is_err(),
        "must refuse an embedded plugin with no package.json"
    );
    // Nothing got registered: no marker, and no cordis file was even created.
    assert!(!profile
        .join("node_modules")
        .join("ghost")
        .join("phl-plugin.json")
        .exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_stale_carried_marker_is_overwritten_not_trusted() {
    // A desktop-export pack may carry the source machine's marker (with a bogus
    // trust / registry id). Registration must rebuild it from this import's
    // facts (package.json name), never trust the carried marker (§R3).
    let dir = root("stale");
    let profile = dir.join("dsh-home").join("profiles").join("web");
    std::fs::create_dir_all(profile.join("node_modules")).unwrap();
    let mine = stage_plugin(&profile, "mine", r#"{"name":"mine","version":"0.1.0"}"#);
    std::fs::write(
        mine.join("phl-plugin.json"),
        br#"{"pluginId":"WRONG","registryId":"WRONG","version":"9.9.9","trust":"verified"}"#,
    )
    .unwrap();

    register_embedded_plugins(
        &manifest_plugins(json!([
            {"id":"who/mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}
        ])),
        &profile,
    )
    .await
    .unwrap();
    let marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(mine.join("phl-plugin.json")).unwrap())
            .unwrap();
    assert_eq!(
        marker["registryId"], "mine",
        "carried WRONG id must not survive"
    );
    assert_ne!(
        marker["trust"], "verified",
        "carried trust must not be inherited"
    );
    assert_eq!(marker["trust"], "unverified");
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
