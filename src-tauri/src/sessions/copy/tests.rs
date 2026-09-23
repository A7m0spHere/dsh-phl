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

/// A legal header of a newer generation: `isSeeded` is required from v2 on and
/// the key set is an exact whitelist, so these fixtures are what DSH itself
/// writes rather than a shape the parser should reject for another reason.
fn header_v3(id: &str, cwd: &str) -> String {
    json!({
        "type": "session", "version": 3, "id": id, "createdAt": 1u64,
        "cwd": cwd, "isSeeded": false, "delegationDepth": 0
    })
    .to_string()
}

/// Write one artifact under its exact canonical name — the escape hatch the
/// fixtures need to build a directory that holds several generations at once.
fn write_named(home: &Path, project: &str, dir: &str, filename: &str, lines: &[&str]) {
    let d = home.join("sessions").join(project).join(dir);
    std::fs::create_dir_all(&d).unwrap();
    let joined = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    let bytes = if filename.ends_with(".zstd") {
        zstd::encode_all(joined.as_bytes(), 3).unwrap()
    } else {
        joined.into_bytes()
    };
    std::fs::write(d.join(filename), bytes).unwrap();
}

fn write_session(home: &Path, project: &str, dir: &str, lines: &[&str], zstd: bool) -> PathBuf {
    let d = home.join("sessions").join(project).join(dir);
    std::fs::create_dir_all(&d).unwrap();
    let joined = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    if zstd {
        std::fs::write(
            d.join(codec::generation_filename(0, codec::LogEncoding::Zstd)),
            zstd::encode_all(joined.as_bytes(), 3).unwrap(),
        )
        .unwrap();
    } else {
        std::fs::write(
            d.join(codec::generation_filename(0, codec::LogEncoding::Plain)),
            joined.as_bytes(),
        )
        .unwrap();
    }
    d.join(codec::generation_filename(
        0,
        if zstd {
            codec::LogEncoding::Zstd
        } else {
            codec::LogEncoding::Plain
        },
    ))
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

    let outcome = copy_one_sync(&located, &target, located.encoding).unwrap();
    assert!(outcome.new_id.starts_with("session-"));
    assert_ne!(outcome.new_id, "session-src");

    // Read back the landed copy through the codec.
    let landed = dst_home
        .join("sessions")
        .join("--D-work--")
        .join(&outcome.new_id)
        .join(codec::generation_filename(0, codec::LogEncoding::Zstd));
    assert!(
        landed.is_file(),
        "copy must live beside the source project group"
    );
    let log = codec::decode(&std::fs::read(&landed).unwrap(), 0).unwrap();
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
    let one = copy_one_sync(&located, &a, located.encoding)
        .unwrap()
        .new_id;
    let two = copy_one_sync(&located, &a, located.encoding)
        .unwrap()
        .new_id;
    assert_ne!(one, two, "two copies must not collide");
    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst);
}

/// A copy must land an artifact DSH accepts, in both physical encodings: a zstd
/// log whose first frame is not the header line alone makes the target instance
/// unbootable, while a plaintext log has no frames to violate. This is the
/// publish gate's end-to-end coverage (the codec unit tests cover the shapes).
#[test]
fn landed_copy_passes_the_publish_gate_for_both_encodings() {
    for zstd in [true, false] {
        let tag = if zstd { "gate-zstd" } else { "gate-plain" };
        let src_home = temp(&format!("{tag}-src"));
        let dst_home = temp(&format!("{tag}-dst"));
        write_session(
            &src_home,
            "--D-work--",
            "session-gate",
            &[
                &header("session-gate", "D:\\work"),
                &json!({"type":"user/message","seq":0}).to_string(),
            ],
            zstd,
        );

        let located = locate_source(&src_home, "session-gate").unwrap();
        assert_eq!(located.encoding == codec::LogEncoding::Zstd, zstd);
        let target = managed_endpoint(
            dst_home.clone(),
            dst_home.clone(),
            "t-gate",
            ManagementMode::ManagedCopy,
        );
        let outcome = copy_one_sync(&located, &target, located.encoding).unwrap();

        let landed = dst_home
            .join("sessions")
            .join("--D-work--")
            .join(&outcome.new_id)
            .join(codec::generation_filename(
                0,
                if zstd {
                    codec::LogEncoding::Zstd
                } else {
                    codec::LogEncoding::Plain
                },
            ));
        let bytes = std::fs::read(&landed).unwrap();
        codec::header_frame_is_alone(&bytes)
            .unwrap_or_else(|e| panic!("landed copy is unpublishable (zstd={zstd}): {e:?}"));
        let log = codec::decode(&bytes, 0).unwrap();
        assert_eq!(log.header.parent(), Some("session-gate"));
        assert_eq!(log.event_count, 1);

        let _ = std::fs::remove_dir_all(src_home);
        let _ = std::fs::remove_dir_all(dst_home);
    }
}

#[test]
fn home_session_encoding_reads_the_existing_artifacts() {
    let zstd_home = temp("enc-zstd");
    write_session(
        &zstd_home,
        "--P--",
        "session-z",
        &[&header("session-z", "C:/p")],
        true,
    );
    assert_eq!(
        home_session_encoding(&zstd_home),
        Some(codec::LogEncoding::Zstd)
    );

    let plain_home = temp("enc-plain");
    write_session(
        &plain_home,
        "--P--",
        "session-p",
        &[&header("session-p", "C:/p")],
        false,
    );
    assert_eq!(
        home_session_encoding(&plain_home),
        Some(codec::LogEncoding::Plain)
    );

    let empty_home = temp("enc-empty");
    assert_eq!(
        home_session_encoding(&empty_home),
        None,
        "a home with no sessions cannot reveal its backend"
    );

    for dir in [zstd_home, plain_home, empty_home] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Cross-encoding copies are refused before any write: DSH fails the whole boot
/// on a home holding the opposite generation's artifact, and an empty target
/// home cannot be observed — so DSH's own default (`zstd`) is assumed and a
/// plaintext source is refused rather than guessed into a zstd home.
#[test]
fn cross_encoding_copies_are_refused() {
    use codec::LogEncoding::{Plain, Zstd};
    assert!(encoding_conflict(Zstd, Some(Zstd)).is_none());
    assert!(encoding_conflict(Plain, Some(Plain)).is_none());
    assert!(encoding_conflict(Zstd, None).is_none());
    assert!(encoding_conflict(Plain, Some(Zstd)).is_some());
    assert!(encoding_conflict(Zstd, Some(Plain)).is_some());
    assert!(
        encoding_conflict(Plain, None).is_some(),
        "a plaintext source into an unobservable target home must be refused"
    );
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
    let err = copy_one_sync(&located, &target, located.encoding).unwrap_err();
    assert!(err.contains("state"), "compat refusal: {err}");
    // Nothing published into the target.
    assert!(!dst_home.join("sessions").exists());
    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

#[test]
fn sanitize_session_dir_keeps_dsh_shape_and_rejects_traversal() {
    // Both shapes DSH writes: a root session and a subagent child (a bare uuid).
    assert!(super::super::sanitize_session_dir("session-abc123").is_ok());
    assert!(super::super::sanitize_session_dir("3996cc7d-be3f-4a5a-bb24-93258dbe037c").is_ok());
    // Path-hostile input never reaches the filesystem: separators, traversal,
    // dot-names and the Windows-unsafe forms are all rejected by the shared
    // segment whitelist, which is the real invariant here.
    for bad in ["../evil", "a/b", "..", ".", "", "evil*", "session-a b"] {
        assert!(
            super::super::sanitize_session_dir(bad).is_err(),
            "{bad:?} must not be accepted as a session handle"
        );
    }
}

/* ------------------------- format generations ------------------------- */

/// DSH resolves a session directory to its **highest** generation
/// (`resolveGenerationInDirectory`). Resolving by fixed name would read the
/// pre-migration v0 log of a session DSH has already migrated — here it holds a
/// different cwd, so reading the wrong one is visible rather than subtle.
#[test]
fn source_resolution_picks_the_highest_generation() {
    let home = temp("gen-pick");
    write_named(
        &home,
        "--P--",
        "session-both",
        "session.jsonl.zstd",
        &[&header("session-both", "C:\\old")],
    );
    write_named(
        &home,
        "--P--",
        "session-both",
        "session.v3.jsonl.zstd",
        &[&header_v3("session-both", "C:\\new")],
    );

    let located = locate_source(&home, "session-both").unwrap();
    assert_eq!(located.generation, 3, "the highest generation is the truth");
    assert!(located
        .artifact
        .to_string_lossy()
        .ends_with("session.v3.jsonl.zstd"));

    let _ = std::fs::remove_dir_all(home);
}

/// A session whose only artifact is a newer generation is *found* — the old
/// fixed-name resolution could not see it at all, which is why the migration
/// panel reported nothing for recently used conversations.
#[test]
fn a_session_that_only_has_a_newer_generation_is_still_located() {
    let home = temp("gen-only-new");
    write_named(
        &home,
        "--P--",
        "session-v3only",
        "session.v3.jsonl.zstd",
        &[&header_v3("session-v3only", "C:\\p")],
    );

    let located = locate_source(&home, "session-v3only").unwrap();
    assert_eq!(located.generation, 3);
    assert_eq!(
        home_session_encoding(&home),
        Some(codec::LogEncoding::Zstd),
        "encoding detection must read `.vN` artifacts too"
    );

    let _ = std::fs::remove_dir_all(home);
}

/// A v3 source cannot be forked by a header rewrite: its fork marker lives in
/// the event body. The refusal must be loud, name the reason, and leave the
/// target home untouched — publishing a `seedLength` header there is exactly
/// what makes an instance stop booting.
#[test]
fn copying_a_newer_generation_is_refused_before_any_write() {
    let src_home = temp("gen-refuse-src");
    let dst_home = temp("gen-refuse-dst");
    write_named(
        &src_home,
        "--P--",
        "session-new",
        "session.v3.jsonl.zstd",
        &[
            &header_v3("session-new", "C:\\p"),
            &json!({"type":"user/message","seq":0}).to_string(),
        ],
    );

    let located = locate_source(&src_home, "session-new").unwrap();
    let reason = unsupported_copy_reason(located.generation).expect("v3 must not be copyable");
    assert!(
        reason.contains("end-seed") && reason.contains("v0/v1"),
        "the refusal must name the cause and the escape hatch: {reason}"
    );

    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.clone(),
        "t-gen",
        ManagementMode::ManagedCopy,
    );
    let err = copy_one_sync(&located, &target, located.encoding).unwrap_err();
    assert_eq!(err, reason, "the copy itself must refuse with that reason");
    assert!(
        !dst_home.join("sessions").exists(),
        "a refused copy writes nothing"
    );

    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

/// v0 and v1 share the physical contract that carries `seedLength`, so both
/// stay copyable — the refusal is about generations, not about "old files".
#[test]
fn v0_and_v1_sources_are_copyable_generations() {
    let src_home = temp("gen-copyable-src");
    let dst_home = temp("gen-copyable-dst");
    write_named(
        &src_home,
        "--P--",
        "session-v1",
        "session.v1.jsonl.zstd",
        &[
            &json!({"type":"session","version":1,"id":"session-v1","createdAt":1u64,
                    "cwd":"C:\\p","delegationDepth":0})
            .to_string(),
            &json!({"type":"user/message","seq":0}).to_string(),
        ],
    );

    let located = locate_source(&src_home, "session-v1").unwrap();
    assert_eq!(located.generation, 1);
    assert_eq!(unsupported_copy_reason(1), None);

    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.clone(),
        "t-v1",
        ManagementMode::ManagedCopy,
    );
    let outcome = copy_one_sync(&located, &target, located.encoding).unwrap();

    // The copy keeps its generation: DSH pairs the filename with the header's
    // own version, so a v1 source must land as `session.v1.jsonl.zstd`.
    let landed = dst_home
        .join("sessions")
        .join("--P--")
        .join(&outcome.new_id)
        .join("session.v1.jsonl.zstd");
    assert!(landed.is_file(), "the copy keeps its own generation");
    let log = codec::decode(&std::fs::read(&landed).unwrap(), 1).unwrap();
    assert_eq!(log.header.parent(), Some("session-v1"));

    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

/// One stored row that carries several logical events (DSH's `text-chunks`).
fn packed_row(seq0: usize, texts: &[&str]) -> String {
    json!({
        "type": "text-chunks", "seq0": seq0, "time0": 1u64,
        "data": { "turn": 0, "step": 0, "index": 0,
                  "dt": vec![1u64; texts.len().saturating_sub(1)], "texts": texts }
    })
    .to_string()
}

/// The fork's inherited-prefix length must be the source's **logical event
/// count**, not its row count: DSH refuses a cut that lands inside an Assistant
/// attempt ("inherited Session cut N splits one Assistant attempt"), and every
/// copy PHL made before this was fixed carries that defect — 37 of the 43 in
/// the maintainer's instance 2, verified against DSH's own catalog.
#[test]
fn a_copy_of_a_packed_log_records_the_event_count_not_the_row_count() {
    let src_home = temp("packed-src");
    let dst_home = temp("packed-dst");
    write_session(
        &src_home,
        "--P--",
        "session-packed",
        &[
            &header("session-packed", "C:\\p"),
            &json!({"type":"user/message","seq":0}).to_string(),
            &packed_row(1, &["a", "b", "c"]),
            &json!({"type":"turn/end","seq":4}).to_string(),
        ],
        true,
    );

    let located = locate_source(&src_home, "session-packed").unwrap();
    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.clone(),
        "t-packed",
        ManagementMode::ManagedCopy,
    );
    let outcome = copy_one_sync(&located, &target, located.encoding).unwrap();

    let landed = std::fs::read(
        dst_home
            .join("sessions")
            .join("--P--")
            .join(&outcome.new_id)
            .join("session.jsonl.zstd"),
    )
    .unwrap();
    let copy = codec::decode(&landed, 0).unwrap();
    assert_eq!(copy.event_count, 3, "three stored rows travel verbatim");
    assert_eq!(copy.inherited_event_count().unwrap(), 5);
    assert_eq!(
        copy.header.value.get("seedLength").and_then(Value::as_u64),
        Some(5),
        "the cut is the logical event count (1 + 3 packed + 1), never the row count"
    );

    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}

/// The publisher refuses the opposite physical encoding on its own account: the
/// orchestrator checks it with the target's name in the message, but a caller
/// that reached the publisher directly would otherwise land an artifact that
/// stops the target instance from booting.
#[test]
fn the_publisher_refuses_a_target_of_the_other_encoding() {
    let src_home = temp("enc-backstop-src");
    let dst_home = temp("enc-backstop-dst");
    write_session(
        &src_home,
        "--P--",
        "session-enc",
        &[&header("session-enc", "C:/p")],
        true,
    );

    let located = locate_source(&src_home, "session-enc").unwrap();
    let target = managed_endpoint(
        dst_home.clone(),
        dst_home.clone(),
        "t-enc",
        ManagementMode::ManagedCopy,
    );
    let err = copy_one_sync(&located, &target, codec::LogEncoding::Plain).unwrap_err();
    assert!(
        err.contains("state"),
        "encoding refusal is a state error: {err}"
    );
    assert!(
        !dst_home.join("sessions").exists(),
        "a refused copy writes nothing"
    );

    let _ = std::fs::remove_dir_all(src_home);
    let _ = std::fs::remove_dir_all(dst_home);
}
