use super::*;
use crate::instances::{InstanceManifest, InstanceSource, ManagementMode};
use serde_json::{json, Value};
use std::path::PathBuf;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-sess-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn header(id: &str, cwd: &str) -> String {
    json!({"type":"session","version":0,"id":id,"createdAt":1u64,"cwd":cwd,"delegationDepth":0})
        .to_string()
}

fn write_session(home: &Path, project: &str, dir: &str, lines: &[&str], zstd: bool) -> PathBuf {
    let d = home.join("sessions").join(project).join(dir);
    std::fs::create_dir_all(&d).unwrap();
    let joined = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    if zstd {
        std::fs::write(
            d.join(ZSTD_ARTIFACT),
            zstd::encode_all(joined.as_bytes(), 3).unwrap(),
        )
        .unwrap();
    } else {
        std::fs::write(d.join("session.jsonl"), joined.as_bytes()).unwrap();
    }
    d.join(if zstd { ZSTD_ARTIFACT } else { "session.jsonl" })
}

fn managed_endpoint(
    dir: PathBuf,
    home: PathBuf,
    id: &str,
    mode: ManagementMode,
) -> SessionEndpoint {
    SessionEndpoint {
        instance_id: id.into(),
        manifest: InstanceManifest {
            schema_version: 2,
            id: id.into(),
            name: format!("inst-{id}"),
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
            source: InstanceSource::Created,
            external_home: None,
            adopted_from: None,
        },
        dir,
        home,
    }
}

#[test]
fn copy_fork_reparents_lineage_and_preserves_cwd_and_events() {
    let src_home = temp("fork-src");
    let dst_home = temp("fork-dst");
    write_session(
        &src_home,
        "--D-work--",
        "session-src",
        &[
            &header("session-src", "D:\\work"),
            &json!({"type":"user/message","seq":0}).to_string(),
            &json!({"type":"turn/end","seq":1}).to_string(),
        ],
        true,
    );

    let located = locate_source(&src_home, "session-src").unwrap();
    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.join("dsh-home"),
        "t1",
        ManagementMode::ManagedCopy,
    );
    // The endpoint's home is what copy writes into; align it.
    let target = SessionEndpoint {
        home: dst_home.clone(),
        ..target
    };

    let outcome = copy_one_sync(&located, &target).unwrap();
    assert!(outcome.new_id.starts_with("session-"));
    assert_ne!(outcome.new_id, "session-src");

    // Read back the landed copy through the codec.
    let landed = dst_home
        .join("sessions")
        .join("--D-work--")
        .join(&outcome.new_id)
        .join(ZSTD_ARTIFACT);
    assert!(
        landed.is_file(),
        "copy must live beside the source project group"
    );
    let log = codec::decode(&std::fs::read(&landed).unwrap()).unwrap();
    assert_eq!(log.header.id, outcome.new_id);
    assert_eq!(log.header.parent(), Some("session-src")); // lineage → source id
    assert_eq!(log.header.cwd(), Some("D:\\work")); // cwd preserved (spec §5.1)
    assert_eq!(log.event_count, 2); // events carried verbatim
    let seed = log.header.value.get("seedLength").and_then(Value::as_u64);
    assert_eq!(seed, Some(2));

    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

#[test]
fn copy_produces_distinct_ids_for_repeated_runs() {
    let src_home = temp("rep-src");
    write_session(
        &src_home,
        "--P--",
        "session-a",
        &[&header("session-a", "C:/p")],
        true,
    );
    let located = locate_source(&src_home, "session-a").unwrap();
    let dst = temp("rep-dst");
    let a = managed_endpoint(dst.clone(), dst.clone(), "t", ManagementMode::ManagedCopy);
    let one = copy_one_sync(&located, &a).unwrap().new_id;
    let two = copy_one_sync(&located, &a).unwrap().new_id;
    assert_ne!(one, two, "two copies must not collide");
    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst);
}

#[test]
fn new_session_id_is_uuidv4_shaped() {
    for _ in 0..50 {
        let id = new_session_id();
        let body = id.strip_prefix("session-").unwrap();
        let groups: Vec<&str> = body.split('-').collect();
        assert_eq!(groups.len(), 5, "8-4-4-4-12 shape: {id}");
        // Variant nibble is 10xx → the hex digit is 8, 9, a, or b.
        assert!(
            matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "variant nibble: {id}"
        );
        // Version nibble is exactly 4.
        assert_eq!(groups[2].chars().next(), Some('4'), "version nibble: {id}");
        assert!(body.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}

#[test]
fn locate_source_reads_plaintext_and_rejects_missing() {
    let home = temp("locate");
    write_session(
        &home,
        "--P--",
        "session-plain",
        &[&header("session-plain", "C:/x")],
        false,
    );
    let located = locate_source(&home, "session-plain").unwrap();
    assert_eq!(located.encoding, codec::LogEncoding::Plain);
    assert!(located.artifact.extension().unwrap() == "jsonl");
    let err = locate_source(&home, "session-nope").unwrap_err();
    assert!(err.contains("not-found"), "got {err}");
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn copy_rejects_incompatible_source_version_before_writing() {
    let src_home = temp("ver-src");
    let dst_home = temp("ver-dst");
    let bad = json!({"type":"session","version":7,"id":"session-x","createdAt":1}).to_string();
    write_session(&src_home, "--P--", "session-x", &[&bad], true);
    let located = locate_source(&src_home, "session-x").unwrap();
    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.clone(),
        "t",
        ManagementMode::ManagedCopy,
    );
    let err = copy_one_sync(&located, &target).unwrap_err();
    assert!(err.contains("state"), "compat refusal: {err}");
    // Nothing published into the target.
    assert!(!dst_home.join("sessions").exists());
    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

#[test]
fn sanitize_session_dir_keeps_dsh_shape_and_rejects_traversal() {
    assert!(super::super::sanitize_session_dir("session-abc123").is_ok());
    assert!(super::super::sanitize_session_dir("../evil").is_err());
    assert!(super::super::sanitize_session_dir("not-a-session").is_err());
}
