//! Command-level tests for the copy guards the spec calls out (§5.4
//! running-session protection, §3.4 external refuse, multi-target fan-out).
//! These drive `copy_sessions_inner` directly (a plain async fn over root +
//! Processes), so no Tauri runtime is needed.

use super::*;
use crate::instances::manifest::write_manifest;
use crate::instances::ManagementMode as MM;
use crate::launch::ProcessEntry;
use serde_json::json;

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-sess-cmd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("instances")).unwrap();
    dir
}

async fn make_instance(root: &Path, id: &str, mode: MM, external_home: Option<PathBuf>) {
    let dir = instance_dir(root, id).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = InstanceManifest {
        schema_version: 2,
        id: id.into(),
        name: id.into(),
        note: None,
        kind: "sandbox".into(),
        hue: 0,
        version_id: "0.1.2".into(),
        runtime_id: "node-22".into(),
        port: 1,
        auto_port: true,
        profile: "web".into(),
        created_at: "now".into(),
        last_run_at: None,
        total_runtime: 0,
        favorite: false,
        env: Default::default(),
        args: Vec::new(),
        api: None,
        management_mode: mode,
        source: crate::instances::InstanceSource::Created,
        external_home: external_home.map(|h| h.to_string_lossy().into_owned()),
        adopted_from: None,
    };
    write_manifest(&dir, &manifest).await.unwrap();
    std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
}

/// Seed one zstd session under an instance's home; returns the encoded dir name.
fn seed_session(home: &Path, dir_name: &str) {
    let sess_dir = home.join("sessions").join("--P--").join(dir_name);
    std::fs::create_dir_all(&sess_dir).unwrap();
    let header = json!({
        "type": "session",
        "version": 0,
        "id": dir_name,
        "createdAt": 1,
        "cwd": "C:/p",
        "delegationDepth": 0
    })
    .to_string();
    let ev = json!({"type":"user/message","seq":0}).to_string();
    let bytes = format!("{header}\n{ev}\n");
    std::fs::write(
        sess_dir.join(codec::generation_filename(0, codec::LogEncoding::Zstd)),
        zstd::encode_all(bytes.as_bytes(), 3).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn copies_into_a_managed_target_and_reports_the_fork() {
    let r = root("happy");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    make_instance(&r, "dst", MM::ManagedCopy, None).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-src",
    );
    let procs = Processes::default();

    let out = copy_sessions_inner(&r, &procs, "src", &["session-src".into()], &["dst".into()])
        .await
        .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].target_id, "dst");
    assert!(out[0].new_id.starts_with("session-"));
    assert_ne!(out[0].new_id, "session-src");
    let landed = r.join("instances").join("dst").join("dsh-home");
    let (list, _) = super::list_in_home(&landed);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].parent.as_deref(), Some("session-src"));
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn fan_out_progress_counts_every_landed_pair() {
    // The select-all path: 2 sessions × 2 targets must stream (1,4)..(4,4).
    // The old per-copy event carried a constant done:0/total:0, which made a
    // four-unit copy read as if nothing had landed.
    use tauri::ipc::InvokeResponseBody;
    let r = root("progress");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    make_instance(&r, "dst-a", MM::ManagedCopy, None).await;
    make_instance(&r, "dst-b", MM::ManagedCopy, None).await;
    let src_home = r.join("instances").join("src").join("dsh-home");
    seed_session(&src_home, "session-one");
    seed_session(&src_home, "session-two");
    let procs = Processes::default();

    let seen: std::sync::Arc<std::sync::Mutex<Vec<(usize, usize)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        tauri::ipc::Channel::new(move |body| {
            // Channel::send serializes via IpcResponse — capture whichever
            // encoding this build uses.
            let raw = match body {
                InvokeResponseBody::Raw(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                InvokeResponseBody::Json(text) => text,
            };
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            seen.lock().unwrap().push((
                v["done"].as_u64().unwrap() as usize,
                v["total"].as_u64().unwrap() as usize,
            ));
            Ok(())
        })
    };
    let out = copy_sessions_inner_with(
        &r,
        &procs,
        "src",
        &["session-one".into(), "session-two".into()],
        &["dst-a".into(), "dst-b".into()],
        Some(&sink),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.len(), 4);
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![(1, 4), (2, 4), (3, 4), (4, 4)]
    );
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn external_target_is_refused_before_any_write() {
    let r = root("ext");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    let ext_home = r.join("user-home");
    std::fs::create_dir_all(&ext_home).unwrap();
    make_instance(&r, "ext", MM::External, Some(ext_home.clone())).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-src",
    );
    let procs = Processes::default();

    let err = copy_sessions_inner(&r, &procs, "src", &["session-src".into()], &["ext".into()])
        .await
        .unwrap_err();
    assert!(err.contains("原地接入"), "got {err}");
    assert!(
        !ext_home.join("sessions").exists(),
        "external home must be untouched"
    );
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn running_source_or_target_is_refused() {
    let r = root("running");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    make_instance(&r, "dst", MM::ManagedCopy, None).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-src",
    );

    let procs = Processes::default();
    procs.set("src", ProcessEntry { pid: 999, port: 1 });
    let err = copy_sessions_inner(&r, &procs, "src", &["session-src".into()], &["dst".into()])
        .await
        .unwrap_err();
    assert!(
        err.contains("运行") || err.contains("正在"),
        "source running refused: {err}"
    );
    procs.remove_if_pid("src", 999);
    procs.set("dst", ProcessEntry { pid: 999, port: 1 });
    let err = copy_sessions_inner(&r, &procs, "src", &["session-src".into()], &["dst".into()])
        .await
        .unwrap_err();
    assert!(
        err.contains("运行") || err.contains("正在"),
        "target running refused: {err}"
    );
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn self_copy_is_refused() {
    let r = root("self");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-src",
    );
    let procs = Processes::default();
    let err = copy_sessions_inner(&r, &procs, "src", &["session-src".into()], &["src".into()])
        .await
        .unwrap_err();
    assert!(err.contains("相同"), "got {err}");
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn multi_target_fanout_copies_to_each() {
    let r = root("fanout");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    make_instance(&r, "a", MM::ManagedCopy, None).await;
    make_instance(&r, "b", MM::ManagedCopy, None).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-src",
    );
    let procs = Processes::default();
    let out = copy_sessions_inner(
        &r,
        &procs,
        "src",
        &["session-src".into()],
        &["a".into(), "b".into()],
    )
    .await
    .unwrap();
    assert_eq!(out.len(), 2);
    // Two distinct new ids, one per target, both children of the same source.
    assert_ne!(out[0].new_id, out[1].new_id);
    for t in ["a", "b"] {
        let (list, _) = super::list_in_home(&r.join("instances").join(t).join("dsh-home"));
        assert_eq!(list.len(), 1, "one fork landed in {t}");
        assert_eq!(list[0].parent.as_deref(), Some("session-src"));
    }
    let _ = std::fs::remove_dir_all(&r);
}

#[tokio::test]
async fn list_and_inspect_roundtrip() {
    let r = root("list");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-aaa",
    );
    seed_session(
        &r.join("instances").join("src").join("dsh-home"),
        "session-bbb",
    );
    let (list, skipped) = super::list_in_home(&r.join("instances").join("src").join("dsh-home"));
    assert_eq!(list.len(), 2);
    assert_eq!(skipped, 0);
    assert!(list.iter().all(|s| s.id.starts_with("session-")));
    assert!(
        list.iter().all(|s| s.format_version == 0),
        "the seeded fixtures are generation 0"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// The list must be generation-blind in the right direction: a session whose
/// only artifact is a newer generation is **listed** (it used to be invisible,
/// which is what made the migration panel show 43 of 97 conversations), and a
/// directory holding two generations is listed once — from its highest one.
#[tokio::test]
async fn list_reports_every_generation_from_its_highest_artifact() {
    let r = root("list-gen");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    let home = r.join("instances").join("src").join("dsh-home");

    // A v3-only session: the shape a conversation takes once a current DSH
    // migrates it.
    let v3_only = home.join("sessions").join("--P--").join("session-v3only");
    std::fs::create_dir_all(&v3_only).unwrap();
    let header = json!({
        "type": "session", "version": 3, "id": "session-v3only", "createdAt": 5u64,
        "cwd": "C:/p", "isSeeded": false, "delegationDepth": 0
    })
    .to_string();
    std::fs::write(
        v3_only.join(codec::generation_filename(3, codec::LogEncoding::Zstd)),
        zstd::encode_all(format!("{header}\n").as_bytes(), 3).unwrap(),
    )
    .unwrap();

    // A migrated session: both generations on disk, the v3 one being the truth.
    let both = home.join("sessions").join("--P--").join("session-migrated");
    std::fs::create_dir_all(&both).unwrap();
    let v3 = json!({
        "type": "session", "version": 3, "id": "session-migrated", "createdAt": 9u64,
        "cwd": "C:/new", "isSeeded": false, "delegationDepth": 0
    })
    .to_string();
    let v0 = json!({
        "type": "session", "version": 0, "id": "session-migrated", "createdAt": 9u64,
        "cwd": "C:/old", "delegationDepth": 0
    })
    .to_string();
    std::fs::write(
        both.join(codec::generation_filename(3, codec::LogEncoding::Zstd)),
        zstd::encode_all(format!("{v3}\n").as_bytes(), 3).unwrap(),
    )
    .unwrap();
    std::fs::write(
        both.join(codec::generation_filename(0, codec::LogEncoding::Zstd)),
        zstd::encode_all(format!("{v0}\n").as_bytes(), 3).unwrap(),
    )
    .unwrap();

    let (list, skipped) = super::list_in_home(&home);
    assert_eq!(skipped, 0, "both sessions are readable: {list:?}");
    assert_eq!(list.len(), 2);
    let v3_only = list
        .iter()
        .find(|s| s.id == "session-v3only")
        .expect("a v3-only session must be listed");
    assert_eq!(v3_only.format_version, 3);
    let migrated = list
        .iter()
        .find(|s| s.id == "session-migrated")
        .expect("the migrated session is listed once");
    assert_eq!(migrated.format_version, 3);
    assert_eq!(
        migrated.cwd.as_deref(),
        Some("C:/new"),
        "the header read must come from the highest generation"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// A directory is a session because it holds an artifact, not because its name
/// starts with `session-`: a subagent child's directory is its **bare uuid**
/// (that is its id), and DSH lists it like any other. The old prefix filter hid
/// every one of them from the migration panel.
#[tokio::test]
async fn subagent_child_sessions_are_listed_from_their_bare_uuid_directory() {
    let r = root("list-subagent");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    let home = r.join("instances").join("src").join("dsh-home");

    let child = home
        .join("sessions")
        .join("--P--")
        .join("3996cc7d-be3f-4a5a-bb24-93258dbe037c");
    std::fs::create_dir_all(&child).unwrap();
    let header = json!({
        "type": "session", "version": 3, "id": "3996cc7d-be3f-4a5a-bb24-93258dbe037c",
        "createdAt": 7u64, "cwd": "C:/p", "parentSession": "session-parent",
        "isSeeded": false, "origin": "subagent", "delegationDepth": 1
    })
    .to_string();
    std::fs::write(
        child.join(codec::generation_filename(3, codec::LogEncoding::Zstd)),
        zstd::encode_all(format!("{header}\n").as_bytes(), 3).unwrap(),
    )
    .unwrap();

    // A directory with no artifact at all is not a session — DSH skips it
    // silently, so it must be neither listed nor counted as broken.
    std::fs::create_dir_all(home.join("sessions").join("--P--").join("attachments")).unwrap();

    let (list, skipped) = super::list_in_home(&home);
    assert_eq!(
        skipped, 0,
        "an artifact-less directory is not a broken session"
    );
    assert_eq!(list.len(), 1, "the subagent child must be listed: {list:?}");
    let child = &list[0];
    assert_eq!(child.session_dir, "3996cc7d-be3f-4a5a-bb24-93258dbe037c");
    assert_eq!(child.parent.as_deref(), Some("session-parent"));
    assert!(child.origin_subagent);
    assert!(super::sanitize_session_dir(&child.session_dir).is_ok());
    let _ = std::fs::remove_dir_all(&r);
}

/// A home whose session directories disagree about the physical encoding is one
/// DSH refuses to load (`encodingMismatch` leaves the first session operation,
/// i.e. the instance never boots). PHL infers the encoding from the artifacts,
/// so **which** directory is the odd one depends on readdir order — the
/// assertion here is the order-independent half: both directories are
/// accounted for, exactly one resolves, and the other is reported rather than
/// silently dropped. Aggregating this into a home-level verdict is a known gap
/// (the listing has no home-health channel yet).
#[tokio::test]
async fn a_home_that_mixes_encodings_never_resolves_both_directories() {
    let r = root("list-mixed-enc");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    let home = r.join("instances").join("src").join("dsh-home");
    seed_session(&home, "session-zstd");
    let mixed = home.join("sessions").join("--P--").join("session-mixed");
    std::fs::create_dir_all(&mixed).unwrap();
    let header = json!({
        "type": "session", "version": 0, "id": "session-mixed",
        "createdAt": 2u64, "cwd": "C:/p", "delegationDepth": 0
    })
    .to_string();
    std::fs::write(
        mixed.join(codec::generation_filename(0, codec::LogEncoding::Plain)),
        format!("{header}\n"),
    )
    .unwrap();

    let (list, skipped) = super::list_in_home(&home);
    assert_eq!(
        list.len(),
        1,
        "only one encoding can be the home's: {list:?}"
    );
    assert_eq!(skipped, 1, "the other directory is reported as unreadable");
    let _ = std::fs::remove_dir_all(&r);
}

/// Seed one plaintext session (a `compression: none` home) for the encoding gate.
fn seed_plaintext_session(home: &Path, dir_name: &str) {
    let sess_dir = home.join("sessions").join("--P--").join(dir_name);
    std::fs::create_dir_all(&sess_dir).unwrap();
    let header = json!({
        "type": "session",
        "version": 0,
        "id": dir_name,
        "createdAt": 1,
        "cwd": "C:/p",
        "delegationDepth": 0
    })
    .to_string();
    let ev = json!({"type":"user/message","seq":0}).to_string();
    std::fs::write(
        sess_dir.join(codec::generation_filename(0, codec::LogEncoding::Plain)),
        format!("{header}\n{ev}\n"),
    )
    .unwrap();
}

/// A cross-encoding copy is refused by the flow itself, before any write:
/// landing the opposite generation's artifact fails DSH's home check and makes
/// the target unbootable, so the refusal has to happen up front rather than at
/// publish time.
#[tokio::test]
async fn cross_encoding_copy_is_refused_before_any_write() {
    let r = root("enc-refuse");
    make_instance(&r, "src", MM::ManagedCopy, None).await;
    make_instance(&r, "dst", MM::ManagedCopy, None).await;
    let src_home = r.join("instances").join("src").join("dsh-home");
    seed_plaintext_session(&src_home, "session-plain");
    let procs = Processes::default();

    let err = copy_sessions_inner(
        &r,
        &procs,
        "src",
        &["session-plain".into()],
        &["dst".into()],
    )
    .await
    .unwrap_err();
    assert!(err.contains("跨编码"), "unexpected refusal: {err}");

    let dst_home = r.join("instances").join("dst").join("dsh-home");
    assert!(
        !dst_home.join("sessions").join("--P--").exists(),
        "a refused copy must not write anything into the target"
    );
    let _ = std::fs::remove_dir_all(&r);
}
