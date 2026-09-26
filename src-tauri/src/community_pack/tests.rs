//! Community pack detection/plan/install tests.
//!
//! `packs/` holds the real market fixtures (see
//! `internal/acceptance/next-release/sources.json` for provenance and
//! hashes); synthetic packs are built in-memory to cover the rejection
//! matrix the market has no sample for.

use super::install::{install_inner, CommunityInstallOutcome, CommunityInstallRequest};
use super::plan::build_preview;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../internal/acceptance/next-release/fixtures/community")
        .join(name);
    if p.exists() {
        Some(p)
    } else {
        // Loud, greppable skip: a public mirror (internal/ excluded) must
        // never show these as silent passes (Codex review §4).
        eprintln!("SKIP (fixture absent from public tree): {name}");
        None
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-packc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a synthetic pack in memory: root marker + manifest + arbitrary
/// extra entries.
fn build_zip(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in &entries {
            zip.start_file((*name).to_string(), opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn marker_json(version: u32) -> Vec<u8> {
    format!(r#"{{"format":"dspack","version":{version}}}"#).into_bytes()
}

fn profile_manifest(version: u32) -> Vec<u8> {
    serde_json::json!({
        "manifestVersion": version,
        "type": "profile",
        "name": "synthetic",
        "version": "1.0.0",
        "profileName": "synthetic",
        "dshVersion": "0.1.7-rc.2",
        "bundles": ["@deepseek-ai/dsh-base", "dsh-fake-plugin"],
        "dependencies": { "dsh-fake-plugin": "1.2.3" },
        "patch": "# patch\n[]\n"
    })
    .to_string()
    .into_bytes()
}

/// A profile manifest with no dependencies: exercises the install path
/// offline (no npm solve needed).
fn profile_manifest_no_deps(version: u32) -> Vec<u8> {
    serde_json::json!({
        "manifestVersion": version,
        "type": "profile",
        "name": "synthetic",
        "version": "1.0.0",
        "profileName": "synthetic",
        "dshVersion": "0.1.7-rc.2",
        "bundles": ["@deepseek-ai/dsh-base"],
        "dependencies": {},
        "patch": "# patch\n[]\n"
    })
    .to_string()
    .into_bytes()
}

fn write_pack(tag: &str, bytes: &[u8]) -> PathBuf {
    let dir = scratch(tag);
    let path = dir.join("test.dspack");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn real_codex_pack_parses_as_v3_v5_profile() {
    let Some(path) = fixture("codex-1.0.0.dspack") else {
        println!("skipped: codex fixture not present");
        return;
    };
    let preview = build_preview(&path).unwrap();
    assert_eq!(preview.container_version, 3);
    assert_eq!(preview.manifest_version, 5);
    assert_eq!(preview.pack_type, "profile");
    assert_eq!(preview.name, "codex");
    assert_eq!(preview.dsh_version.as_deref(), Some("0.1.5-alpha.2"));
    // Bundle order is preserved (order is semantics, B03/D05).
    assert_eq!(preview.bundles.first().unwrap(), "@deepseek-ai/dsh-base");
    assert!(preview
        .bundles
        .contains(&"dsh-skills-manager-plus".to_string()));
    // Bundle membership is per-dependency: dsh-im-connect is only inserted
    // by the patch layer, so it sits in dependencies but not in bundles.
    assert!(preview
        .dependencies
        .iter()
        .any(|d| d.coordinate == "@michengai/dsh-im-connect" && !d.in_bundles));
    assert!(preview
        .dependencies
        .iter()
        .any(|d| d.coordinate == "dsh-skills-manager-plus" && d.in_bundles));
    assert!(preview.blocked.is_empty(), "{:?}", preview.blocked);
    // Matches the recorded source hash.
    assert_eq!(
        preview.pack_sha256,
        "1578e230f627320a1ad515fd28c4eb41d2c2b32badfcc7de8e46992adc03373c"
    );
    // The home/ content was recognised.
    assert!(preview.home_count > 0, "codex carries home/skills");
}

#[test]
fn real_better_sidebar_manifest_v4_empty_dsh_version_needs_choice() {
    let Some(path) = fixture("better-sidebar-1.0.0.dspack") else {
        println!("skipped: better-sidebar fixture not present");
        return;
    };
    let preview = build_preview(&path).unwrap();
    assert_eq!(preview.container_version, 3);
    assert_eq!(preview.manifest_version, 4);
    // dshVersion "" is treated as absent: the user must pick, nothing is
    // silently resolved to "latest".
    assert_eq!(preview.dsh_version, None);
    assert!(preview.blocked.is_empty(), "{:?}", preview.blocked);
}

#[test]
fn synthetic_v2_v4_container_is_accepted() {
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(2)),
        ("manifest.json", profile_manifest(4)),
        ("overrides/cordis.patch.yml", b"[]\n".to_vec()),
    ]);
    let path = write_pack("v2", &bytes);
    let preview = build_preview(&path).unwrap();
    assert_eq!(preview.container_version, 2);
    assert_eq!(preview.manifest_version, 4);
    assert!(preview.blocked.is_empty(), "{:?}", preview.blocked);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn rejection_matrix_names_the_reason() {
    type Case = (String, Vec<(&'static str, Vec<u8>)>, &'static str);
    let cases: Vec<Case> = vec![
        (
            "missing marker".into(),
            vec![("manifest.json", profile_manifest(5))],
            "dspack.json",
        ),
        (
            "wrong format".into(),
            vec![
                ("dspack.json", br#"{"format":"other","version":3}"#.to_vec()),
                ("manifest.json", profile_manifest(5)),
            ],
            "format",
        ),
        (
            "future container".into(),
            vec![
                ("dspack.json", marker_json(9)),
                ("manifest.json", profile_manifest(5)),
            ],
            "容器版本",
        ),
        (
            "manifest v3".into(),
            vec![
                ("dspack.json", marker_json(3)),
                ("manifest.json", profile_manifest(3)),
            ],
            "manifest 版本",
        ),
        (
            "dshhome".into(),
            vec![
                ("dspack.json", marker_json(3)),
                (
                    "manifest.json",
                    serde_json::json!({
                        "manifestVersion": 5, "type": "dshhome",
                        "name": "x", "version": "1.0.0",
                        "defaultProfile": "a",
                        "profiles": {"a": {"bundles": [], "dependencies": {}}}
                    })
                    .to_string()
                    .into_bytes(),
                ),
            ],
            "dshhome",
        ),
        (
            "collection".into(),
            vec![
                ("dspack.json", marker_json(3)),
                (
                    "manifest.json",
                    serde_json::json!({
                        "manifestVersion": 5, "type": "collection",
                        "name": "x", "version": "1.0.0"
                    })
                    .to_string()
                    .into_bytes(),
                ),
            ],
            "collection",
        ),
        (
            "non-empty files[]".into(),
            vec![
                ("dspack.json", marker_json(3)),
                (
                    "manifest.json",
                    serde_json::json!({
                        "manifestVersion": 5, "type": "profile",
                        "name": "x", "version": "1.0.0",
                        "files": [{"path": "models/big.bin", "sha256": "ab", "size": 1, "urls": []}]
                    })
                    .to_string()
                    .into_bytes(),
                ),
            ],
            "files[]",
        ),
        (
            "env file in overrides".into(),
            vec![
                ("dspack.json", marker_json(3)),
                ("manifest.json", profile_manifest(5)),
                ("overrides/.env", b"SECRET=1".to_vec()),
            ],
            ".env",
        ),
        (
            "nested archive".into(),
            vec![
                ("dspack.json", marker_json(3)),
                ("manifest.json", profile_manifest(5)),
                ("overrides/inner.zip", b"PK".to_vec()),
            ],
            "嵌套压缩包",
        ),
        (
            "path traversal".into(),
            vec![
                ("dspack.json", marker_json(3)),
                ("manifest.json", profile_manifest(5)),
                ("overrides/../evil.txt", b"x".to_vec()),
            ],
            "不安全",
        ),
        (
            "node_modules in overrides".into(),
            vec![
                ("dspack.json", marker_json(3)),
                ("manifest.json", profile_manifest(5)),
                ("overrides/node_modules/x/index.js", b"x".to_vec()),
            ],
            "node_modules",
        ),
    ];
    for (label, entries, needle) in cases {
        let bytes = build_zip(entries);
        let path = write_pack("reject", &bytes);
        // Matrix rejections surface as blocked entries on an Ok preview or
        // as a hard error — both are refusals with a reason.
        let text = match build_preview(&path) {
            Err(e) => e,
            Ok(p) if !p.blocked.is_empty() => p.blocked.join("; "),
            Ok(_) => panic!("{label}: expected rejection, got a clean preview"),
        };
        assert!(
            text.contains(needle),
            "{label}: expected mention of {needle}, got: {text}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

/// A change to the pack bytes must invalidate the previewed plan (D12).
#[tokio::test]
async fn install_refuses_when_pack_bytes_changed_since_preview() {
    let root = scratch("hash");
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest(5)),
        ("overrides/cordis.patch.yml", b"[]\n".to_vec()),
    ]);
    let pack = root.join("test.dspack");
    std::fs::write(&pack, &bytes).unwrap();
    let preview = build_preview(&pack).unwrap();

    // Seed the target version marker the install requires.
    let vdir = root.join("versions").join("0.1.7-rc.2");
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(
        vdir.join("phl-install.json"),
        r#"{"installedAt":"2026-01-01T00:00:00Z","version":"0.1.7-rc.2"}"#,
    )
    .unwrap();

    let req = CommunityInstallRequest {
        instance: serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": "packc-1",
            "name": "社区包",
            "kind": "sandbox",
            "hue": 0,
            "versionId": "",
            "runtimeId": "node-system",
            "port": 8100,
            "autoPort": false,
            "profile": "web",
            "createdAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
        path: pack.to_string_lossy().into_owned(),
        pack_sha256: format!("{:0>64}", "0"),
        dsh_version: "0.1.7-rc.2".into(),
        install_dependencies: false,
        allow_missing: false,
    };
    let err = install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        req,
        &|_| {},
    )
    .await
    .unwrap_err();
    assert!(err.contains("不一致") || err.contains("重新预览"), "{err}");
    // Nothing landed.
    assert!(!crate::instances::instance_dir(&root, "packc-1")
        .unwrap()
        .exists());
    assert!(!crate::instances::instances_root(&root)
        .join(".phl-packc-packc-1")
        .exists());
    let _ = std::fs::remove_dir_all(&root);
    drop(preview);
}

/// Full install of a dependency-free pack: overrides land in the web
/// profile, home/ lands in the HOME root, the archive's machine snapshot
/// (package.json) never bypasses the plan, and the instance is committed.
#[tokio::test]
async fn install_maps_overrides_and_home_without_dependencies() {
    let root = scratch("install");
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest_no_deps(5)),
        (
            "overrides/cordis.patch.yml",
            b"- insert:\n    - id: dsh-fake-plugin\n      name: 'dsh-fake-plugin'\n".to_vec(),
        ),
        ("home/skills/fake/SKILL.md", b"# skill".to_vec()),
        (
            "package.json",
            br#"{"name":"evil","dependencies":{"evil":"1.0.0"}}"#.to_vec(),
        ),
        ("pnpm-lock.yaml", b"lockfileVersion: '9.0'".to_vec()),
    ]);
    let pack = root.join("test.dspack");
    std::fs::write(&pack, &bytes).unwrap();
    let preview = build_preview(&pack).unwrap();
    assert!(preview.blocked.is_empty(), "{:?}", preview.blocked);

    let vdir = root.join("versions").join("0.1.7-rc.2");
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(
        vdir.join("phl-install.json"),
        r#"{"installedAt":"2026-01-01T00:00:00Z","version":"0.1.7-rc.2"}"#,
    )
    .unwrap();

    let req = CommunityInstallRequest {
        instance: serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": "packc-2",
            "name": "社区包",
            "kind": "sandbox",
            "hue": 0,
            "versionId": "",
            "runtimeId": "node-22.12.0",
            "port": 8101,
            "autoPort": false,
            "profile": "web",
            "createdAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
        path: pack.to_string_lossy().into_owned(),
        pack_sha256: preview.pack_sha256.clone(),
        dsh_version: "0.1.7-rc.2".into(),
        install_dependencies: true,
        allow_missing: false,
    };
    let outcome = install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        req,
        &|_| {},
    )
    .await
    .unwrap();

    assert!(outcome.committed_record_exists());
    let dest = crate::instances::instance_dir(&root, "packc-2").unwrap();
    // Overrides → profile.
    let patch = dest
        .join("dsh-home")
        .join("profiles")
        .join("web")
        .join("cordis.patch.yml");
    let patch_text = std::fs::read_to_string(&patch).unwrap();
    assert!(patch_text.contains("dsh-fake-plugin"));
    // Manifest-written patch was NOT duplicated over an existing file.
    // home/ → HOME root.
    assert!(dest
        .join("dsh-home")
        .join("skills")
        .join("fake")
        .join("SKILL.md")
        .exists());
    // The archive's machine snapshot never reached the instance.
    assert!(!dest.join("dsh-home").join("package.json").exists());
    assert!(!dest.join("dsh-home").join("pnpm-lock.yaml").exists());
    // Import marker recorded the plan binding.
    let marker = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    assert!(marker.contains(&preview.pack_sha256));
    // Dependency-free pack: nothing to install, ready.
    assert_eq!(outcome.dependencies_installed, 0);
    assert_eq!(outcome.readiness, "readyToLaunch");
    let _ = std::fs::remove_dir_all(&root);
}

impl CommunityInstallOutcome {
    fn committed_record_exists(&self) -> bool {
        !self.record.manifest.id.is_empty()
    }
}

/* ---------------- Codex review probes, promoted (CR-01..03) ---------------- */

/// CR-01: the bundle stack must survive into the installed profile's
/// package.json, in manifest order — not only inside PHL's private marker.
#[tokio::test]
async fn bundle_stack_survives_install_into_profile_package_json() {
    let root = scratch("bundle-stack");
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest(5)),
        ("overrides/cordis.patch.yml", b"[]\n".to_vec()),
    ]);
    let pack = root.join("test.dspack");
    std::fs::write(&pack, &bytes).unwrap();
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    let outcome = install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        request_for(&pack, &preview, "packc-bundles"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-bundles").unwrap();
    let pkg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dest.join("dsh-home")
                .join("profiles")
                .join("web")
                .join("package.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        pkg["dsh"]["profile"]["bundles"],
        serde_json::json!(["@deepseek-ai/dsh-base", "dsh-fake-plugin"]),
        "bundles land in manifest order"
    );
    // The dependency set keys on the resolved package names.
    assert_eq!(
        pkg["dependencies"]["dsh-fake-plugin"],
        serde_json::json!("1.2.3")
    );
    assert!(outcome.committed_record_exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-02: an overrides/package.json can never inject a dependency the
/// manifest does not name — the machine file is rebuilt from the plan.
#[tokio::test]
async fn override_package_json_cannot_inject_unplanned_dependency() {
    let root = scratch("inject");
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest_no_deps(5)),
        (
            "overrides/package.json",
            br#"{"name":"evil","dependencies":{"unplanned-review-package":"9.9.9"}}"#.to_vec(),
        ),
    ]);
    let pack = root.join("test.dspack");
    std::fs::write(&pack, &bytes).unwrap();
    let preview = build_preview(&pack).unwrap();
    assert!(
        preview.blocked.is_empty(),
        "a plain override package.json is not itself a rejection: {:?}",
        preview.blocked
    );
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        request_for(&pack, &preview, "packc-inject"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-inject").unwrap();
    let pkg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dest.join("dsh-home")
                .join("profiles")
                .join("web")
                .join("package.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        pkg["dependencies"].as_object().unwrap().len(),
        0,
        "the archive's package.json must not reach the profile: {pkg}"
    );
    assert!(pkg["dependencies"]
        .get("unplanned-review-package")
        .is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-03: overrides/ and home/ entries mapping to the same target refuse
/// the whole import instead of racing the traversal order.
#[test]
fn home_cannot_replace_authoritative_patch() {
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest(5)),
        (
            "overrides/cordis.patch.yml",
            b"- id: from-overrides\n".to_vec(),
        ),
        (
            "home/profiles/web/cordis.patch.yml",
            b"- id: from-home\n".to_vec(),
        ),
    ]);
    let path = write_pack("collide", &bytes);
    let text = match build_preview(&path) {
        Err(e) => e,
        Ok(p) if !p.blocked.is_empty() => p.blocked.join("; "),
        Ok(_) => panic!("expected the collision to refuse the import"),
    };
    assert!(text.contains("同一目标文件"), "{text}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// CR-01 identity rule: a git coordinate whose repo name is not in the
/// bundle stack and has no unique unclaimed bundle is refused, not guessed.
#[test]
fn git_dependency_package_name_is_resolved_not_guessed() {
    // Repo last segment matches a bundle → that bundle name is the key.
    let with_match = build_zip(vec![
        ("dspack.json", marker_json(3)),
        (
            "manifest.json",
            serde_json::json!({
                "manifestVersion": 5, "type": "profile",
                "name": "gitok", "version": "1.0.0",
                "bundles": ["dsh-wildmon"],
                "dependencies": {"github:swaylq/dsh-wildmon": "a2c7df00b27c2de9f2b0915d3bd5f1acc4d02c68"}
            })
            .to_string()
            .into_bytes(),
        ),
    ]);
    let path = write_pack("git-ok", &with_match);
    let preview = build_preview(&path).unwrap();
    assert!(preview.blocked.is_empty(), "{:?}", preview.blocked);
    assert_eq!(preview.dependencies[0].package_name, "dsh-wildmon");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    // Repo name absent from bundles, two unclaimed bundles → ambiguous.
    let ambiguous = build_zip(vec![
        ("dspack.json", marker_json(3)),
        (
            "manifest.json",
            serde_json::json!({
                "manifestVersion": 5, "type": "profile",
                "name": "gitbad", "version": "1.0.0",
                "bundles": ["dsh-alpha", "dsh-beta"],
                "dependencies": {"github:someone/other-repo": "a2c7df00b27c2de9f2b0915d3bd5f1acc4d02c68"}
            })
            .to_string()
            .into_bytes(),
        ),
    ]);
    let path = write_pack("git-bad", &ambiguous);
    let text = match build_preview(&path) {
        Err(e) => e,
        Ok(p) if !p.blocked.is_empty() => p.blocked.join("; "),
        Ok(_) => panic!("expected ambiguous git identity to refuse"),
    };
    assert!(text.contains("npm 包名"), "{text}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// CR-02 phase-B side: a drifted package.json fails the verification and
/// npm never runs — the marker stage stays needsDependencies.
#[tokio::test]
async fn prepare_dependencies_rejects_a_drifted_package_json() {
    let root = scratch("drift");
    seed_version_marker(&root, "0.1.7-rc.2");
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        ("manifest.json", profile_manifest(5)),
        ("overrides/cordis.patch.yml", b"[]\n".to_vec()),
    ]);
    let pack = root.join("test.dspack");
    std::fs::write(&pack, &bytes).unwrap();
    let preview = build_preview(&pack).unwrap();
    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        request_for(&pack, &preview, "packc-drift"),
        &|_| {},
    )
    .await
    .unwrap();

    // Someone (or something) edits the machine file after the install.
    let dest = crate::instances::instance_dir(&root, "packc-drift").unwrap();
    let profile = dest.join("dsh-home").join("profiles").join("web");
    let drifted = serde_json::json!({
        "name": "dsh-profile-synthetic",
        "private": true,
        "dependencies": {"dsh-fake-plugin": "1.2.3", "unplanned": "0.0.1"},
        "dsh": {"profile": {"bundles": ["@deepseek-ai/dsh-base", "dsh-fake-plugin"]}}
    });
    std::fs::write(profile.join("package.json"), drifted.to_string()).unwrap();

    let failures = super::install::prepare_dependencies_for_test(&root, &dest, &|_| {})
        .await
        .unwrap_err();
    assert!(
        failures.iter().any(|f| f.contains("不一致")),
        "drift must fail verification, not run npm: {failures:?}"
    );
    let marker = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    assert!(marker.contains("needsDependencies"), "{marker}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Shared fixtures for the promoted probes.
fn seed_version_marker(root: &Path, bare: &str) {
    let vdir = root.join("versions").join(bare);
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(
        vdir.join("phl-install.json"),
        format!(r#"{{"installedAt":"2026-01-01T00:00:00Z","version":"{bare}"}}"#),
    )
    .unwrap();
}

fn request_for(
    pack: &Path,
    preview: &super::CommunityPackPreview,
    id: &str,
) -> CommunityInstallRequest {
    CommunityInstallRequest {
        instance: serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": id,
            "name": "社区包",
            "kind": "sandbox",
            "hue": 0,
            "versionId": "",
            "runtimeId": "node-system",
            "port": 8100,
            "autoPort": false,
            "profile": "web",
            "createdAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
        path: pack.to_string_lossy().into_owned(),
        pack_sha256: preview.pack_sha256.clone(),
        dsh_version: "0.1.7-rc.2".into(),
        install_dependencies: false,
        allow_missing: false,
    }
}

/* ------------------- R2-01/02 promotion tests ------------------- */

const FULL_SHA: &str = "a2c7df00b27c2de9f2b0915d3bd5f1acc4d02c68";
const SHORT_SHA: &str = "a2c7df0";

fn git_pack(tag: &str, pin: &str) -> PathBuf {
    let bytes = build_zip(vec![
        ("dspack.json", marker_json(3)),
        (
            "manifest.json",
            serde_json::json!({
                "manifestVersion": 5, "type": "profile",
                "name": "gitdep", "version": "1.0.0",
                "bundles": ["dsh-wildmon"],
                "dependencies": {"github:swaylq/dsh-wildmon": pin}
            })
            .to_string()
            .into_bytes(),
        ),
        ("overrides/cordis.patch.yml", b"[]\n".to_vec()),
    ]);
    let dir = scratch(tag);
    let path = dir.join("git.dspack");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn git_install_request(
    pack: &Path,
    preview: &super::CommunityPackPreview,
    id: &str,
) -> CommunityInstallRequest {
    let mut req = request_for(pack, preview, id);
    req.install_dependencies = false;
    req
}

/// R2-01: the generated git spec is the npm git form
/// `github:owner/repo#<sha>` — never `name:github:…` (which npm parses as
/// a local directory).
#[tokio::test]
async fn git_dependency_spec_is_the_npm_git_form() {
    let root = scratch("gitspec");
    let pack = git_pack("gitspec", FULL_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, "packc-gitspec"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-gitspec").unwrap();
    let pkg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            dest.join("dsh-home")
                .join("profiles")
                .join("web")
                .join("package.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        pkg["dependencies"]["dsh-wildmon"],
        serde_json::Value::String(format!("github:swaylq/dsh-wildmon#{FULL_SHA}")),
        "npm must see a git spec, not a directory: {pkg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

struct StubResolver {
    full: Option<String>,
    calls: std::sync::Mutex<u32>,
}

impl super::install::ShortShaResolver for StubResolver {
    fn resolve<'a>(
        &'a self,
        _repo: &'a str,
        _pin: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        let full = self.full.clone();
        *self.calls.lock().unwrap() += 1;
        Box::pin(async move { full })
    }
}

/// R2-01: a short SHA resolves to the unique full commit, the resolution
/// lands in package.json AND the marker, and a retry after a failed npm
/// install does not mistake its own resolution for user drift.
#[tokio::test]
async fn short_sha_resolution_syncs_the_marker_and_survives_retry() {
    let root = scratch("shortsha");
    let pack = git_pack("shortsha", SHORT_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, "packc-short"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-short").unwrap();
    let resolver = StubResolver {
        full: Some(FULL_SHA.into()),
        calls: std::sync::Mutex::new(0),
    };

    // First prepare: resolves the short SHA, then npm fails offline — the
    // failure must be the npm one, not a drift refusal.
    let first =
        super::install::prepare_dependencies_for_test_with(&root, &dest, &|_| {}, &resolver).await;
    let marker_raw = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    // The SHORT pin (as a complete pin, delimited by the closing quote)
    // must be gone; the full commit recorded in its place.
    let short_pin = format!("#{SHORT_SHA}\"");
    let full_pin = format!("#{FULL_SHA}\"");
    assert!(
        marker_raw.contains(&full_pin) && !marker_raw.contains(&short_pin),
        "the resolved full commit is recorded in the marker: {marker_raw}"
    );
    // The failure must be environmental (npm), never a drift refusal: the
    // resolver's own output must not be mistaken for user tampering.
    assert!(
        matches!(&first, Err(f) if !f.iter().any(|e| e.contains("不一致"))),
        "first prepare fails at npm, not drift: {first:?}"
    );

    // Second prepare: pins are already full — the resolver must NOT be
    // called again, and the same npm failure repeats (a retry, not drift).
    let second =
        super::install::prepare_dependencies_for_test_with(&root, &dest, &|_| {}, &resolver).await;
    assert_eq!(
        *resolver.calls.lock().unwrap(),
        1,
        "resolution happens once"
    );
    assert!(
        matches!(&second, Err(f) if !f.iter().any(|e| e.contains("不一致"))),
        "retry is a plain npm retry, not a drift refusal: {second:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-01: an unresolvable short SHA blocks phase B and leaves the instance
/// needsDependencies.
#[tokio::test]
async fn unresolvable_short_sha_blocks_and_stays_pending() {
    let root = scratch("sha-fail");
    let pack = git_pack("sha-fail", SHORT_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, "packc-shafail"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-shafail").unwrap();
    let resolver = StubResolver {
        full: None,
        calls: std::sync::Mutex::new(0),
    };
    let err = super::install::prepare_dependencies_for_test_with(&root, &dest, &|_| {}, &resolver)
        .await
        .unwrap_err();
    assert!(err.iter().any(|e| e.contains("无法解析")), "{err:?}");
    let marker = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    assert!(marker.contains("needsDependencies"));
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-02: deleting the machine file when the plan HAS dependencies is an
/// explicit failure — never a silent promotion to readyToLaunch.
#[tokio::test]
async fn missing_package_json_with_pending_deps_fails_not_ready() {
    let root = scratch("missing-pkg");
    let pack = git_pack("missing-pkg", FULL_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, "packc-misspkg"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-misspkg").unwrap();
    std::fs::remove_file(
        dest.join("dsh-home")
            .join("profiles")
            .join("web")
            .join("package.json"),
    )
    .unwrap();

    let err = super::install::prepare_dependencies_for_test(&root, &dest, &|_| {})
        .await
        .unwrap_err();
    assert!(err.iter().any(|e| e.contains("缺失")), "{err:?}");
    let marker = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    assert!(
        marker.contains("needsDependencies"),
        "the gate must still block: {marker}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-02: a corrupted import marker is an explicit failure — the readiness
/// projection must never read a torn marker as "ready".
#[tokio::test]
async fn corrupted_import_marker_fails_explicitly() {
    let root = scratch("corrupt-marker");
    let pack = git_pack("corrupt-marker", FULL_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(&root, "0.1.7-rc.2");

    install_inner(
        &root,
        &crate::instances::trial::tests::task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, "packc-corrupt"),
        &|_| {},
    )
    .await
    .unwrap();

    let dest = crate::instances::instance_dir(&root, "packc-corrupt").unwrap();
    std::fs::write(dest.join("phl-import.json"), b"{\"stage\": \"rea").unwrap();
    let err = super::install::prepare_dependencies_for_test(&root, &dest, &|_| {})
        .await
        .unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.contains("损坏") || e.contains("不可读取")),
        "{err:?}"
    );
    // The torn marker reads as a damaged import record — never as "ready",
    // and never as "not an import" (R3-03).
    let manifest = crate::instances::load_manifest(&dest, "packc-corrupt")
        .await
        .unwrap();
    let state = crate::instances::read_import_state(&dest, Some(&manifest)).await;
    assert!(
        matches!(state, crate::instances::ImportState::Corrupt(_)),
        "{state:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/* ------------------- R2-06 retry gate promotion tests ------------------- */

fn retry_task() -> crate::resources::Task {
    crate::instances::trial::tests::task()
}

/// Installs a dependency-carrying community instance without installing
/// dependencies, then returns (root, instance_dir).
async fn community_instance_pending(root: &Path, id: &str) -> std::path::PathBuf {
    // Tag the fixture with the instance id: several of these tests run in
    // parallel in one process and must not share (or delete) one scratch dir.
    let pack = git_pack(id, FULL_SHA);
    let preview = build_preview(&pack).unwrap();
    seed_version_marker(root, "0.1.7-rc.2");
    install_inner(
        root,
        &retry_task(),
        &Arc::new(AtomicBool::new(false)),
        git_install_request(&pack, &preview, id),
        &|_| {},
    )
    .await
    .unwrap();
    crate::instances::instance_dir(root, id).unwrap()
}

/// R2-06: a RUNNING instance refuses the dependency rebuild, and nothing is
/// written.
#[tokio::test]
async fn running_instance_refuses_dependency_retry() {
    let root = scratch("retry-running");
    let dest = community_instance_pending(&root, "packc-run").await;
    let before = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();

    let processes = crate::launch::Processes::default();
    processes.0.lock().unwrap().insert(
        "packc-run".into(),
        crate::launch::ProcessEntry {
            pid: 4321,
            port: 8200,
        },
    );
    let err = super::install::prepare_pack_dependencies_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &processes,
        format!("t{}", crate::versions::now_millis()),
        "packc-run".into(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("运行"), "{err}");
    let after = std::fs::read_to_string(dest.join("phl-import.json")).unwrap();
    assert_eq!(before, after, "a refused retry writes nothing");
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-06: an external instance refuses — PHL must not write into a home it
/// does not own.
#[tokio::test]
async fn external_instance_refuses_dependency_retry() {
    let root = scratch("retry-ext");
    let dest = community_instance_pending(&root, "packc-ext").await;
    // Flip the manifest to external after the fact.
    let mut manifest = crate::instances::read_manifest(&dest).await.unwrap();
    manifest.management_mode = crate::instances::ManagementMode::External;
    manifest.external_home = Some(dest.join("dsh-home").to_string_lossy().into_owned());
    crate::instances::manifest::write_manifest(&dest, &manifest)
        .await
        .unwrap();

    let err = super::install::prepare_pack_dependencies_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &crate::launch::Processes::default(),
        format!("t{}", crate::versions::now_millis()),
        "packc-ext".into(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("不受 PHL 管理"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-06: an ordinary (non-community) instance refuses — no import record
/// means no plan to rebuild against.
#[tokio::test]
async fn non_community_instance_refuses_dependency_retry() {
    let root = scratch("retry-plain");
    // A plain instance: valid manifest, no import marker.
    let dir = crate::instances::instance_dir(&root, "daily").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    crate::instances::manifest::write_manifest(
        &dir,
        &serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": "daily",
            "name": "日常",
            "kind": "sandbox",
            "hue": 0,
            "versionId": "dsh-0.1.7-rc.2",
            "runtimeId": "node-system",
            "port": 8000,
            "autoPort": false,
            "profile": "web",
            "createdAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let err = super::install::prepare_pack_dependencies_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &crate::launch::Processes::default(),
        format!("t{}", crate::versions::now_millis()),
        "daily".into(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("不是社区整合包导入目标"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-06: a STOPPED import target passes the gate and reaches the npm
/// phase (which fails environmentally in tests, not as a refusal).
#[tokio::test]
async fn stopped_import_target_passes_the_gate() {
    let root = scratch("retry-pass");
    let _dest = community_instance_pending(&root, "packc-pass").await;
    let outcome = super::install::prepare_pack_dependencies_inner(
        &root,
        &crate::versions::Transfers::default(),
        &crate::resources::ResourceLocks::default(),
        &crate::resources::Tasks::default(),
        &crate::launch::Processes::default(),
        format!("t{}", crate::versions::now_millis()),
        "packc-pass".into(),
        tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    // The env has no npm-capable runtime: the phase reports the failure
    // through the outcome (needsDependencies), not as a gate refusal.
    assert_eq!(outcome.readiness, "needsDependencies");
    assert!(!outcome.dependency_failures.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
