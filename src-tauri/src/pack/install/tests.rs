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

/// The §E full chain against real exporter bytes, in two scratch roots that
/// never touch the developer's PHL data:
/// export → whole-archive secret scan → install into a fresh root →
/// listing agrees → corrupted archive refuses with zero residue.
#[tokio::test]
async fn export_scan_install_roundtrip_keeps_secrets_out_and_residue_clean() {
    use crate::pack::export::{export_pack_inner, PackExportOptions};
    use crate::resources::{TaskInfo, Tasks};
    use std::sync::atomic::AtomicBool;
    use tauri::ipc::Channel;

    const SECRET: &str = "PHL_TEST_SECRET_9F8A7B";

    fn env_manifest(id: &str) -> InstanceManifest {
        let mut env = HashMap::new();
        env.insert("PHL_TEST_KEY".to_string(), SECRET.to_string());
        env.insert("APP_MODE".to_string(), "prod".to_string());
        InstanceManifest {
            schema_version: 2,
            id: id.into(),
            name: "Packable".into(),
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
        }
    }

    fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else {
                out.push(p);
            }
        }
    }

    let a = root("e2e-a");
    let b = root("e2e-b");
    std::fs::create_dir_all(a.join("instances")).unwrap();
    std::fs::create_dir_all(b.join("instances")).unwrap();
    let src = a.join("instances").join("exp-a1");
    std::fs::create_dir_all(
        src.join("dsh-home")
            .join("profiles")
            .join("web")
            .join("node_modules"),
    )
    .unwrap();
    write_manifest(&src, &env_manifest("exp-a1")).await.unwrap();
    let dest = a.join("out.phlpack");

    export_pack_inner(
        &a,
        "exp-a1".into(),
        dest.to_string_lossy().into_owned(),
        PackExportOptions {
            embed_registry_ids: vec![],
            include_sessions: false,
            sessions_privacy_ack: false,
        },
    )
    .await
    .unwrap();

    // Whole-archive scan: unpack EVERY payload byte plus the manifest text
    // and refuse the secret value anywhere (names may travel, values may not).
    let scan = root("e2e-scan");
    let pack = crate::pack::read_pack_from_path(&dest).unwrap();
    crate::pack::unpack::unpack_entries_to(&dest, |rel| {
        let p = scan.join(rel);
        Some((p, scan.clone()))
    })
    .unwrap();
    let mut files = Vec::new();
    collect_files(&scan, &mut files);
    assert!(
        !files.is_empty(),
        "the archive carries payload entries to scan"
    );
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "secret value leaked into {}",
            f.display()
        );
    }
    assert!(
        !serde_json::to_string(&pack.manifest)
            .unwrap()
            .contains(SECRET),
        "secret value leaked into the manifest"
    );
    let _ = std::fs::remove_dir_all(&scan);

    // Install into the fresh root B.
    let tasks = Tasks::default();
    let flag = Arc::new(AtomicBool::new(false));
    let task = tasks
        .begin(
            TaskInfo::new("e2e-install".into(), "pack-install", "e2e".into(), &[]),
            Some(flag.clone()),
        )
        .unwrap();
    let outcome = install_inner(
        &b,
        task,
        &dest.to_string_lossy(),
        PackInstallRequest {
            manifest: env_manifest("imp-b1"),
            allow_missing: false,
        },
        &Channel::new(|_| Ok(())),
        flag,
    )
    .await
    .unwrap();
    assert_eq!(outcome.credential_names, vec!["PHL_TEST_KEY".to_string()]);
    assert_eq!(outcome.record.manifest.version_id, "0.1.2-rc.1");
    assert_eq!(
        outcome
            .record
            .manifest
            .env
            .get("APP_MODE")
            .map(String::as_str),
        Some("prod")
    );
    assert!(
        !outcome.record.manifest.env.contains_key("PHL_TEST_KEY"),
        "the credential value must not survive into the installed env"
    );
    assert_eq!(
        outcome.record.manifest.management_mode,
        ManagementMode::PackInstalled
    );

    let listed = crate::instances::list_instances_inner(&b).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].manifest.id, "imp-b1");
    // Persisted bytes must also be clean, not just the in-memory record.
    let on_disk = std::fs::read(b.join("instances").join("imp-b1").join("instance.json")).unwrap();
    assert!(!on_disk
        .windows(SECRET.len())
        .any(|w| w == SECRET.as_bytes()));

    // A corrupted archive refuses before staging exists, with zero residue.
    let bad = b.join("corrupt.phlpack");
    let good = std::fs::read(&dest).unwrap();
    std::fs::write(&bad, &good[..good.len() / 2]).unwrap();
    let tasks = Tasks::default();
    let flag2 = Arc::new(AtomicBool::new(false));
    let task2 = tasks
        .begin(
            TaskInfo::new("e2e-bad".into(), "pack-install", "bad".into(), &[]),
            Some(flag2.clone()),
        )
        .unwrap();
    let err = install_inner(
        &b,
        task2,
        &bad.to_string_lossy(),
        PackInstallRequest {
            manifest: env_manifest("imp-bad"),
            allow_missing: false,
        },
        &Channel::new(|_| Ok(())),
        flag2,
    )
    .await
    .unwrap_err();
    assert!(!err.is_empty());
    assert!(!b.join("instances").join("imp-bad").exists());
    assert_eq!(
        crate::instances::list_instances_inner(&b)
            .await
            .unwrap()
            .len(),
        1,
        "only the good install remains"
    );
    assert!(
        !std::fs::read_dir(b.join("instances"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(".phl")),
        "no staging residue from the refused install"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
