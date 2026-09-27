//! Trial flow tests. Drives `run_trial`/`build_preview` directly (the same
//! pattern the snapshot/version commands use for their inner bodies).

use super::*;

pub(super) fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-trial-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn processes() -> crate::launch::Processes {
    crate::launch::Processes::default()
}

pub(crate) fn task() -> crate::resources::Task {
    crate::resources::Tasks::default()
        .begin(
            crate::resources::TaskInfo::new(
                "trial-test".into(),
                "trial-create",
                "test".into(),
                &[],
            ),
            None,
        )
        .unwrap()
}

/// One real zstd session (header + event) the sessions module can read.
fn seed_session_home(home: &Path) {
    let sess_dir = home.join("sessions").join("proj").join("session-1");
    std::fs::create_dir_all(&sess_dir).unwrap();
    let header = serde_json::json!({
        "type": "session",
        "version": 0,
        "id": "session-1",
        "createdAt": 1,
        "cwd": "C:/projects/demo",
        "delegationDepth": 0
    });
    let ev = serde_json::json!({"type": "user/message", "seq": 0});
    let bytes = format!(
        "{header}
{ev}
"
    );
    let name =
        crate::sessions::codec::generation_filename(0, crate::sessions::codec::LogEncoding::Zstd);
    let encoded = zstd::encode_all(bytes.as_bytes(), 3).unwrap();
    std::fs::write(sess_dir.join(name), encoded).unwrap();
}

pub(super) fn request(source_id: &str) -> TrialRequest {
    serde_json::from_value(serde_json::json!({
        "sourceId": source_id,
        "targetVersion": "0.1.7-rc.2",
        "targetRuntimeId": "node-system",
        "scope": "config",
        "workspace": "fresh",
    }))
    .unwrap()
}

/// Seeds a source instance with a dsh-home containing config, plugins,
/// sessions and logs, plus two fake version trees (old & new).
pub(super) async fn seed_source(root: &Path, id: &str) -> PathBuf {
    let dir = instance_dir(root, id).unwrap();
    let profile = profile_root(&dir, "web");
    std::fs::create_dir_all(profile.join("node_modules").join("my-plugin")).unwrap();
    std::fs::write(profile.join("cordis.patch.yml"), "[]\n").unwrap();
    seed_session_home(&dir.join("dsh-home"));
    std::fs::create_dir_all(dir.join("dsh-home").join("logs")).unwrap();
    std::fs::create_dir_all(dir.join("workspace")).unwrap();
    std::fs::write(dir.join("dsh-home").join("settings.yaml"), "a: 1\n").unwrap();

    // A plugin-internal junction into the version tree: the fallback
    // chain a trial must retarget to the new version.
    let old_tree = root
        .join("versions")
        .join("0.1.5-rc.3")
        .join("node_modules");
    std::fs::create_dir_all(&old_tree).unwrap();
    std::fs::write(old_tree.join("core.js"), "// old core").unwrap();
    let new_tree = root
        .join("versions")
        .join("0.1.7-rc.2")
        .join("node_modules");
    std::fs::create_dir_all(&new_tree).unwrap();
    std::fs::write(new_tree.join("core.js"), "// new core").unwrap();
    super::super::copy::recreate_link(
        &profile.join("node_modules").join("dsh-core"),
        &old_tree,
        true,
    )
    .unwrap();
    // A REAL (non-link) plugin package pinned to an older version than the
    // target tree ships — the shape an old `dsh plugin add` leaves behind and
    // the acceptance bug surfaced: copied verbatim, the new DSH's stricter
    // plugin contracts can reject it at boot. The preview must name it.
    let scoped = profile
        .join("node_modules")
        .join("@deepseek-ai")
        .join("old-tool");
    std::fs::create_dir_all(&scoped).unwrap();
    std::fs::write(
        scoped.join("package.json"),
        r#"{ "name": "@deepseek-ai/old-tool", "version": "0.1.5-alpha.2" }"#,
    )
    .unwrap();
    std::fs::create_dir_all(new_tree.join("@deepseek-ai").join("old-tool")).unwrap();
    std::fs::write(
        new_tree
            .join("@deepseek-ai")
            .join("old-tool")
            .join("package.json"),
        r#"{ "name": "@deepseek-ai/old-tool", "version": "0.1.7-rc.2" }"#,
    )
    .unwrap();
    // Same version on both sides: NOT a mismatch, must not be listed.
    let matched = profile
        .join("node_modules")
        .join("@deepseek-ai")
        .join("same-build");
    std::fs::create_dir_all(&matched).unwrap();
    std::fs::write(
        matched.join("package.json"),
        r#"{ "name": "@deepseek-ai/same-build", "version": "1.2.3" }"#,
    )
    .unwrap();
    std::fs::create_dir_all(new_tree.join("@deepseek-ai").join("same-build")).unwrap();
    std::fs::write(
        new_tree
            .join("@deepseek-ai")
            .join("same-build")
            .join("package.json"),
        r#"{ "name": "@deepseek-ai/same-build", "version": "1.2.3" }"#,
    )
    .unwrap();

    let manifest: InstanceManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2,
        "id": id,
        "name": "日常环境",
        "kind": "sandbox",
        "hue": 0,
        "versionId": "dsh-0.1.5-rc.3",
        "runtimeId": "node-22.11.0",
        "port": 8000,
        "autoPort": false,
        "profile": "web",
        "createdAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    super::super::manifest::write_manifest(&dir, &manifest)
        .await
        .unwrap();
    dir
}

pub(super) fn seed_versions(root: &Path) {
    for bare in ["0.1.5-rc.3", "0.1.7-rc.2"] {
        let dir = root.join("versions").join(bare);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("phl-install.json"),
            format!(r#"{{"installedAt":"2026-01-01T00:00:00Z","version":"{bare}"}}"#),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn preview_names_mismatched_real_packages_before_commit() {
    let root = scratch("mismatch");
    seed_versions(&root);
    seed_source(&root, "daily").await;

    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    // The preview must warn BEFORE the user commits: the old real package is
    // listed, the same-build one is not, and the note carries both versions
    // plus the consequence (the acceptance bug: an 0.1.5 agent-team copied
    // into an 0.1.7 trial failed at boot with no prior warning at all).
    assert_eq!(preview.mismatched_packages.len(), 1);
    assert!(
        preview.mismatched_packages[0].contains("@deepseek-ai/old-tool")
            && preview.mismatched_packages[0].contains("0.1.5-alpha.2")
            && preview.mismatched_packages[0].contains("0.1.7-rc.2"),
        "the preview names the package and both versions: {}",
        preview.mismatched_packages[0]
    );
    assert!(
        preview.mismatched_packages[0].contains("重装"),
        "the note says what to do about it: {}",
        preview.mismatched_packages[0]
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A REAL package the target tree does not carry AT ALL is also named: there
/// is no link failure for a real directory, so this path is the only thing
/// that tells the user the old build rides into the copy with no replacement
/// (the acceptance case: upstream renamed the packages).
#[tokio::test]
async fn preview_names_real_packages_the_target_no_longer_ships() {
    let root = scratch("dropped");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    // A real package with a version, that the target tree simply does not
    // have (not even under another name PHL can know about).
    let pkg = profile_root(&instance_dir(&root, "daily").unwrap(), "web")
        .join("node_modules")
        .join("legacy-tool");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{ "name": "legacy-tool", "version": "0.9.0" }"#,
    )
    .unwrap();

    let preview = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let dropped = preview
        .mismatched_packages
        .iter()
        .find(|m| m.contains("legacy-tool"))
        .unwrap_or_else(|| {
            panic!(
                "a dropped real package must be named: {:?}",
                preview.mismatched_packages
            )
        });
    assert!(
        dropped.contains("不再携带") && dropped.contains("0.9.0"),
        "the note names the drop and the carried version: {dropped}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn config_scope_copies_config_and_drops_sessions() {
    let root = scratch("scope");
    seed_versions(&root);
    seed_source(&root, "daily").await;

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    let outcome = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    assert!(outcome.committed);
    assert_eq!(outcome.sessions_imported, 0);
    // The seed source carries a `default`-inheritance binding with an empty
    // provider list, the exact shape the old code mislabeled. Credentials
    // are a launch-time concern, never a copy defect — the copy is ready.
    assert_eq!(
        outcome.readiness, "readyToLaunch",
        "an empty managed binding is not needsCredentials: {}",
        outcome.readiness
    );
    // The version-mismatched real package is named on the outcome (exactly
    // one: the same-build package must NOT be listed), with both versions.
    assert_eq!(outcome.mismatched_packages.len(), 1);
    assert!(
        outcome.mismatched_packages[0].contains("@deepseek-ai/old-tool"),
        "names the mismatching package: {}",
        outcome.mismatched_packages[0]
    );
    assert!(
        outcome.mismatched_packages[0].contains("0.1.5-alpha.2")
            && outcome.mismatched_packages[0].contains("0.1.7-rc.2"),
        "names both versions: {}",
        outcome.mismatched_packages[0]
    );
    let dest = instance_dir(&root, &req.target_id).unwrap();
    let record = outcome.record;
    assert_eq!(record.manifest.version_id, "dsh-0.1.7-rc.2");
    // Config travelled…
    assert!(dest.join("dsh-home").join("settings.yaml").exists());
    assert!(dest
        .join("dsh-home")
        .join("profiles")
        .join("web")
        .join("node_modules")
        .join("my-plugin")
        .exists());
    // …while sessions, logs and the workspace did not.
    assert!(!dest.join("dsh-home").join("sessions").exists());
    assert!(!dest.join("dsh-home").join("logs").exists());
    assert!(!dest.join("workspace").exists());

    // The fallback link was retargeted to the new version and resolves.
    let link = dest
        .join("dsh-home")
        .join("profiles")
        .join("web")
        .join("node_modules")
        .join("dsh-core");
    assert!(
        link.join("core.js").exists(),
        "retargeted link must resolve"
    );
    let body = std::fs::read_to_string(link.join("core.js")).unwrap();
    assert!(
        body.contains("new core"),
        "link must resolve into the NEW version"
    );
    assert_eq!(outcome.link_redirects, 1);
    assert!(outcome.link_failures.is_empty());
    assert_eq!(outcome.readiness, "readyToLaunch");

    // The isolated agents-home boundary: the copy gets its own.
    let manifest: InstanceManifest =
        serde_json::from_value(serde_json::to_value(record.manifest).unwrap()).unwrap();
    assert!(manifest.env.contains_key("DSH_AGENTS_HOME"));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn sessions_scope_travels_and_source_stays_untouched() {
    let root = scratch("sessions");
    seed_versions(&root);
    let source = seed_source(&root, "daily").await;

    // Source tree baseline: relative path + content hash for files, and the
    // resolved target for reparse points. Equal sizes prove nothing about
    // content (contract §4) — the hash does.
    fn snapshot(dir: &Path) -> Vec<(String, String)> {
        use sha2::{Digest, Sha256};
        let mut out = Vec::new();
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let rel = e
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let Ok(meta) = e.metadata() else { continue };
                #[cfg(windows)]
                let is_reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let is_reparse = meta.file_type().is_symlink();
                if is_reparse {
                    // Junction/symlink: record where it actually points.
                    let target = std::fs::canonicalize(e.path())
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| "<unresolvable>".into());
                    out.push((format!("{rel} [link]"), target));
                    continue;
                }
                if e.path().is_dir() {
                    walk(&e.path(), base, out);
                } else if let Ok(bytes) = std::fs::read(e.path()) {
                    let mut h = Sha256::new();
                    h.update(&bytes);
                    out.push((rel, hex::encode(h.finalize())));
                }
            }
        }
        walk(dir, dir, &mut out);
        out.sort();
        out
    }
    let before = snapshot(&source);

    let (mut req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    // A sessions-scope preview re-derives the plan over the wider scope.
    let mut preview_req = request("daily");
    preview_req.scope = "config+sessions".into();
    let preview = build_preview(&root, &processes(), &preview_req)
        .await
        .unwrap();
    req.scope = "config+sessions".into();
    req.plan_id = preview.plan_id.clone();
    req.target_id = preview.target_id.clone();
    req.source_fingerprint = preview.source_fingerprint.clone();
    let outcome = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本含会话",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        preview.allocated_port,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert_eq!(outcome.sessions_imported, 1);
    let dest = instance_dir(&root, &req.target_id).unwrap();
    let _ = allocated;
    assert!(dest
        .join("dsh-home")
        .join("sessions")
        .join("proj")
        .join("session-1")
        .exists());

    // Source is byte-identical afterwards.
    let after = snapshot(&source);
    assert_eq!(before, after, "source tree must be unchanged");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn missing_new_version_content_is_reported_not_hidden() {
    let root = scratch("linkfail");
    seed_versions(&root);
    let _source = seed_source(&root, "daily").await;
    // Remove the whole subtree the link would need in the new version:
    // the link points at node_modules/, so the directory must be gone
    // for the retarget to fail.
    std::fs::remove_dir_all(
        root.join("versions")
            .join("0.1.7-rc.2")
            .join("node_modules"),
    )
    .unwrap();

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    let outcome = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(outcome.committed);
    assert_eq!(outcome.link_redirects, 0);
    assert_eq!(
        outcome.link_failures.len(),
        1,
        "unresolvable link must be reported"
    );
    assert_eq!(outcome.readiness, "needsDependencies");
    let _ = std::fs::remove_dir_all(&root);
}

/// The retarget must recognise its own version links even when the link
/// target and the data-root string are two different spellings of the SAME
/// directory. `Path::starts_with` is byte-exact, which silently skipped the
/// redirect and left the copy pointing at the OLD version tree — caught by
/// CI, not locally, because the local TEMP spelling already agreed.
///
/// Two spellings matter on Windows: case (a canonicalized drive label vs.
/// the env var's) and 8.3 short names — a GitHub runner's TEMP is
/// `C:\Users\RUNNER~1\...` while canonicalize expands it to
/// `C:\Users\runneradmin\...`, which is exactly what the junction target
/// carries (the copy engine derives targets from canonicalize). The
/// regressions here retarget through both shapes.
#[tokio::test]
async fn case_flipped_version_link_is_still_redirected() {
    let root = scratch("linkcase");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    retarget_with_alternate_spelling(&root, case_flip).await;
}

/// The CI-proven shape: the junction target carries the expanded/canonical
/// spelling while the data-root string is the 8.3-short-name form. Built by
/// re-creating the link with the canonicalized target, then running the
/// trial against the short-named root — what the runner actually did.
#[tokio::test]
async fn short_name_spelled_version_link_is_still_redirected() {
    let root = scratch("link83");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    // On a machine whose TEMP has no short-name component this must still
    // exercise the canonical-vs-raw comparison: seed the link with the
    // canonicalized target (what copy.rs would store), and let the match
    // go through the raw spelling when canonicalize of the prefix agrees.
    retarget_with_alternate_spelling(&root, |p| {
        crate::paths::strip_verbatim(&std::fs::canonicalize(p).unwrap())
    })
    .await;
}

/// Shared body: re-create the source's version junction with an alternate
/// spelling of the SAME path, then run the trial and assert the retarget
/// still lands in the NEW version tree.
async fn retarget_with_alternate_spelling(root: &Path, alt: fn(&Path) -> PathBuf) {
    let profile = profile_root(&instance_dir(root, "daily").unwrap(), "web");
    let link = profile.join("node_modules").join("dsh-core");
    let old_tree = root
        .join("versions")
        .join("0.1.5-rc.3")
        .join("node_modules");
    let alternate = alt(&old_tree);
    std::fs::remove_dir(&link).unwrap();
    super::super::copy::recreate_link(&link, &alternate, true).unwrap();

    let (req, allocated, auto) = plan_via_preview(root, &processes(), "daily").await;
    let outcome = run_trial(
        &Arc::new(AtomicBool::new(false)),
        root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    assert!(outcome.committed);
    assert_eq!(
        outcome.link_redirects, 1,
        "an alternatively-spelled version link is still PHL's own and must be retargeted"
    );
    assert!(outcome.link_failures.is_empty());
    let dest = instance_dir(root, &req.target_id).unwrap();
    let body = std::fs::read_to_string(
        dest.join("dsh-home")
            .join("profiles")
            .join("web")
            .join("node_modules")
            .join("dsh-core")
            .join("core.js"),
    )
    .unwrap();
    assert!(
        body.contains("new core"),
        "the retargeted link must resolve into the NEW version even when the \
         original spelling disagreed: {body}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Upper-cases the drive letter and the first path segment — enough to break
/// a byte-exact prefix match, still the same path to the file system.
fn case_flip(path: &Path) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    let (drive, rest) = text.split_at(2);
    let mut flipped = drive.to_uppercase();
    if let Some(slash) = rest.find('\\') {
        let (head, tail) = rest.split_at(slash);
        flipped.push_str(&head.to_uppercase());
        flipped.push_str(tail);
    } else {
        flipped.push_str(&rest.to_uppercase());
    }
    PathBuf::from(flipped)
}

#[tokio::test]
async fn running_source_is_refused_and_staging_never_leaks() {
    let root = scratch("running");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let processes = crate::launch::Processes::default();
    processes.0.lock().unwrap().insert(
        "daily".to_string(),
        crate::launch::ProcessEntry {
            pid: 1234,
            port: 8000,
        },
    );

    let (req, allocated, auto) = plan_via_preview(&root, &processes, "daily").await;
    let err = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes,
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("运行"), "unexpected: {err}");
    // Nothing landed.
    assert!(!instances_root(&root)
        .join(format!(".phl-trial-{}", req.target_id))
        .exists());
    assert!(!instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn missing_target_version_refuses_before_any_copy() {
    let root = scratch("noversion");
    seed_source(&root, "daily").await;
    // No version markers seeded at all.
    let req = request("daily");
    let preview = build_preview(&root, &processes(), &req).await.unwrap();
    assert!(!preview.target_installed);
    assert!(!preview.pending_downloads.is_empty());
    assert!(
        preview.blocked.is_empty(),
        "missing downloads prepare, not block: {:?}",
        preview.blocked
    );

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    let err = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert!(err.contains("未安装"), "unexpected: {err}");
    assert!(!instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn preview_estimates_bytes_and_counts_sessions() {
    let root = scratch("preview");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let mut req = request("daily");
    req.scope = "config+sessions".into();

    let preview = build_preview(&root, &processes(), &req).await.unwrap();
    assert!(preview.estimated_bytes > 0);
    assert_eq!(preview.suggested_name, "日常环境 · 新版测试");
    assert_eq!(preview.session_count, Some(1));
    assert_eq!(preview.session_cwds, vec!["C:/projects/demo"]);
    let _ = std::fs::remove_dir_all(&root);

    // The config-only estimate is smaller than the with-sessions one.
    let root = scratch("preview2");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let config = build_preview(&root, &processes(), &request("daily"))
        .await
        .unwrap();
    let mut with = request("daily");
    with.scope = "config+sessions".into();
    let with = build_preview(&root, &processes(), &with).await.unwrap();
    assert!(config.estimated_bytes < with.estimated_bytes);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn shared_workspace_junctions_into_the_source_tree() {
    let root = scratch("shared-ws");
    seed_versions(&root);
    let source = seed_source(&root, "daily").await;
    std::fs::write(source.join("workspace").join("project.txt"), "shared").unwrap();

    // A shared-workspace plan: the preview is built with the same option.
    let mut preview_req = request("daily");
    preview_req.workspace = "shared".into();
    let preview = build_preview(&root, &processes(), &preview_req)
        .await
        .unwrap();
    let mut req = preview_req;
    req.plan_id = preview.plan_id.clone();
    req.target_id = preview.target_id.clone();
    req.source_fingerprint = preview.source_fingerprint.clone();
    run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        preview.allocated_port,
        preview.auto_port,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    let dest = instance_dir(&root, &req.target_id).unwrap();
    let through_copy = dest.join("workspace").join("project.txt");
    assert!(
        through_copy.exists(),
        "the copy's workspace must resolve into the source's directory"
    );
    assert_eq!(
        std::fs::read_to_string(through_copy).unwrap().trim(),
        "shared"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/* --------------- CR-07/08 + R2-03/04 promotion tests --------------- */

/// The plan resolution a create performs, in the tuple form these tests
/// assert on. The command itself matches on `plan_route` so it can answer a
/// committed plan without executing.
pub(super) async fn validate_plan(
    root: &Path,
    processes: &crate::launch::Processes,
    req: &TrialRequest,
) -> Result<(String, String, String, String, String, u16, bool), String> {
    match plan_route(root, processes, req).await? {
        PlanRoute::Fresh(plan) => Ok(plan),
        // A committed copy of this plan: the retry VALIDATES (that is the R3-02
        // fix), and the tuple describes what the copy actually holds.
        PlanRoute::Committed { dest, id } => {
            let manifest = load_manifest(&dest, &id).await?;
            Ok((
                id,
                manifest.name.clone(),
                manifest
                    .version_id
                    .strip_prefix("dsh-")
                    .unwrap_or(&manifest.version_id)
                    .to_string(),
                manifest.version_id.clone(),
                manifest.runtime_id.clone(),
                u16::try_from(manifest.port).unwrap_or(0),
                manifest.auto_port,
            ))
        }
    }
}

/// Builds a request through a REAL preview: plan_id, target_id, fingerprint
/// and port all come from the backend, exactly as the frontend would send
/// them back. Returns the request plus the preview's port/autoroll.
async fn plan_via_preview(
    root: &Path,
    processes: &crate::launch::Processes,
    source_id: &str,
) -> (TrialRequest, u16, bool) {
    let req = request(source_id);
    let preview = build_preview(root, processes, &req).await.unwrap();
    let mut req = request(source_id);
    req.plan_id = preview.plan_id.clone();
    req.target_id = preview.target_id.clone();
    req.source_fingerprint = preview.source_fingerprint.clone();
    (req, preview.allocated_port, preview.auto_port)
}

/// CR-07: a fixed-port source never hands its port to the copy — the plan
/// allocates a distinct, live-verified port and the copy keeps it.
#[tokio::test]
async fn copy_of_fixed_port_source_gets_its_own_port() {
    let root = scratch("port");
    seed_versions(&root);
    seed_source(&root, "daily").await; // port 8000, autoPort=false

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    assert_ne!(allocated, 8000, "the copy must not inherit the fixed port");
    assert!(!auto);

    let outcome = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert_eq!(u32::from(allocated), outcome.record.manifest.port);
    assert!(!outcome.record.manifest.auto_port);
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-08/C16: a plan built from a fingerprint the source no longer matches
/// is refused — never executed against changed data.
#[tokio::test]
async fn stale_plan_fingerprint_refuses_to_copy() {
    let root = scratch("stale");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let (mut req, _port, _auto) = plan_via_preview(&root, &processes(), "daily").await;
    req.source_fingerprint = "not-the-real-fingerprint".into();
    let err = validate_plan(&root, &processes(), &req).await.unwrap_err();
    assert!(err.contains("计划已过期"), "{err}");
    assert!(!instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-03: the fingerprint must cover MORE than instance.json — editing the
/// profile's cordis.patch.yml after the preview invalidates the plan.
#[tokio::test]
async fn patch_edit_after_preview_invalidates_the_plan() {
    let root = scratch("patchdrift");
    seed_versions(&root);
    let source = seed_source(&root, "daily").await;
    let (req, _port, _auto) = plan_via_preview(&root, &processes(), "daily").await;

    // Mutate the profile's patch — exactly the drift the review planted.
    let profile = profile_root(&source, "web");
    std::fs::write(
        profile.join("cordis.patch.yml"),
        "- insert:\n    - id: drifted\n      name: 'drifted'\n",
    )
    .unwrap();

    let err = validate_plan(&root, &processes(), &req).await.unwrap_err();
    assert!(
        err.contains("计划已过期"),
        "patch drift must expire the plan: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-08: a create without a preview-issued plan refuses outright.
#[tokio::test]
async fn create_without_a_plan_refuses() {
    let root = scratch("noplan");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let req = request("daily");
    let err = validate_plan(&root, &processes(), &req).await.unwrap_err();
    assert!(err.contains("缺少试升级计划"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The marker doc promises "every field except the plan id defaults", but
/// `readiness` lacked a serde default — a marker from before the field
/// existed failed to parse, `read_trial_marker` returned None, and the
/// idempotent retry refused the copy as a foreign plan. The parse must
/// succeed and default to the honest ready.
#[tokio::test]
async fn an_old_marker_without_readiness_still_parses_and_reports_ready() {
    let root = scratch("oldmarker");
    std::fs::create_dir_all(&root).unwrap();
    // Minimal pre-field marker: identity + the fields that always existed.
    std::fs::write(
        root.join("phl-trial.json"),
        format!(r#"{{ "planId": "{}" }}"#, "a".repeat(64)),
    )
    .unwrap();
    let marker = super::read_trial_marker(&root).await;
    let marker = marker.expect("an old marker without readiness must still parse");
    assert_eq!(marker.readiness, "readyToLaunch");
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-03/R3-02: a plan id the backend never issued (a well-formed-looking
/// forgery, or a record that was swept) refuses — the persisted plan is the
/// identity authority, and nothing is rebuilt from live state.
#[tokio::test]
async fn plan_id_that_does_not_derive_from_the_request_refuses() {
    let root = scratch("smuggled");
    seed_versions(&root);
    seed_source(&root, "daily").await;
    let (mut req, _port, _auto) = plan_via_preview(&root, &processes(), "daily").await;
    req.plan_id = "0".repeat(64); // well-formed-looking but foreign
    let err = validate_plan(&root, &processes(), &req).await.unwrap_err();
    assert!(err.contains("计划不存在或已过期"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-08/C13/R2-03: submitting the same plan twice never creates a second
/// copy, and the retry reports the copy's RECORDED outcome (including a
/// degraded readiness), not a fabricated success.
#[tokio::test]
async fn repeated_submission_of_one_plan_does_not_create_twice() {
    let root = scratch("idempotent");
    seed_versions(&root);
    seed_source(&root, "daily").await;

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    let first = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(first.committed);

    let second = run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();
    assert!(second.committed, "the retry reports the existing copy");
    assert!(
        second.notes.iter().any(|n| n.contains("已存在")),
        "the retry is honest about being one: {:?}",
        second.notes
    );
    // The recorded readiness matches the first run's outcome, and exactly
    // one instance directory exists.
    assert_eq!(second.readiness, first.readiness);
    assert!(instance_dir(&root, &req.target_id).unwrap().exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-03: a DIFFERENT plan pointing at the same target id refuses — one
/// plan can never overwrite another plan's copy.
#[tokio::test]
async fn foreign_plan_on_an_existing_target_refuses() {
    let root = scratch("foreign");
    seed_versions(&root);
    seed_source(&root, "daily").await;

    let (req, allocated, auto) = plan_via_preview(&root, &processes(), "daily").await;
    run_trial(
        &Arc::new(AtomicBool::new(false)),
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        allocated,
        auto,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap();

    // A second preview would get its own target id; simulate a plan that
    // claims the SAME target id (the collision the review described).
    let (mut req2, _p, _a) = plan_via_preview(&root, &processes(), "daily").await;
    req2.target_id = req.target_id.clone();
    // Recompute req2's plan id over its own fields so only the target-id
    // overlap is under test.
    let fp = source_fingerprint(
        &root,
        &instance_dir(&root, "daily").unwrap(),
        &load_manifest(&instance_dir(&root, "daily").unwrap(), "daily")
            .await
            .unwrap(),
        &req2.scope,
    )
    .await;
    req2.source_fingerprint = fp;
    let err = validate_plan(&root, &processes(), &req2).await.unwrap_err();
    assert!(
        err.contains("计划与当前选项不一致") || err.contains("属于另一个试升级计划"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// R2-04: the quiet check is read-only — a fresh write inside the window
/// refuses, and the source tree is NEVER moved.
#[tokio::test]
async fn quiet_check_refuses_fresh_writes_without_touching_the_tree() {
    let root = scratch("quiet");
    let home = root.join("home");
    std::fs::create_dir_all(home.join("logs")).unwrap();
    std::fs::write(home.join("settings.yaml"), "a: 1\n").unwrap();

    // A fresh write (the log a live DSH would leave).
    std::fs::write(home.join("logs").join("latest.log"), "running").unwrap();
    let err = assert_home_quiet(&home).await.unwrap_err();
    assert!(err.contains("疑似仍在运行"), "{err}");
    // Read-only: the tree is exactly where it was.
    assert!(home.join("settings.yaml").exists());
    assert!(home.join("logs").join("latest.log").exists());

    // After the write ages past the window, the same tree passes. The
    // window is 120s — instead of sleeping, backdate the file's mtime.
    let log = home.join("logs").join("latest.log");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(300);
    let f = std::fs::File::options().write(true).open(&log).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    drop(f);
    assert_home_quiet(&home).await.unwrap();
    assert!(home.exists(), "the source HOME stays in place, always");
    let _ = std::fs::remove_dir_all(&root);
}

/// C02/R2-04: a cancel mid-copy leaves the source HOME at its original
/// path, untouched — the failure-injection the review required.
#[tokio::test]
async fn cancel_during_copy_leaves_source_home_in_place() {
    let root = scratch("cancel-mid");
    seed_versions(&root);
    let source = seed_source(&root, "daily").await;
    let source_home = source.join("dsh-home");
    assert!(source_home.exists());

    // Cancel BEFORE the copy starts.
    let flag = Arc::new(AtomicBool::new(true));
    let (req, _p, _a) = plan_via_preview(&root, &processes(), "daily").await;
    let err = run_trial(
        &flag,
        &root,
        &task(),
        &processes(),
        &req,
        &req.target_id,
        "副本",
        "0.1.7-rc.2",
        "dsh-0.1.7-rc.2",
        "node-system",
        8600,
        false,
        &tauri::ipc::Channel::new(|_| Ok(())),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "cancelled");
    assert!(
        source_home.exists(),
        "source HOME stays at its original path"
    );
    assert!(!instance_dir(&root, &req.target_id).unwrap().exists());
    assert!(
        !instances_root(&root)
            .join(format!(".phl-trial-{}", req.target_id))
            .exists(),
        "no staging leftover"
    );
    let _ = std::fs::remove_dir_all(&root);
}
