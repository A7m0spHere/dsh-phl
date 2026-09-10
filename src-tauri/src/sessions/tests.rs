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
    let header =
        json!({"type":"session","version":0,"id":dir_name,"createdAt":1,"cwd":"C:/p"}).to_string();
    let ev = json!({"type":"user/message","seq":0}).to_string();
    let bytes = format!("{header}\n{ev}\n");
    std::fs::write(
        sess_dir.join(ZSTD_ARTIFACT),
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
    let _ = std::fs::remove_dir_all(&r);
}
