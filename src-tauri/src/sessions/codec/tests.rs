use super::*;
use serde_json::json;

fn header_line(id: &str, extra: Map<String, Value>) -> String {
    let mut m = Map::new();
    m.insert("type".into(), json!("session"));
    m.insert("version".into(), json!(0));
    m.insert("id".into(), json!(id));
    m.insert("createdAt".into(), json!(1_700_000_000_000u64));
    m.insert("cwd".into(), json!("D:\\work\\proj"));
    m.insert("delegationDepth".into(), json!(0));
    for (k, v) in extra {
        m.insert(k, v);
    }
    serde_json::to_string(&Value::Object(m)).unwrap()
}

fn event_line(seq: usize) -> String {
    json!({"type": "user/message", "seq": seq, "data": {"text": format!("m{seq}")}}).to_string()
}

/// A packed Assistant row: **one** stored row carrying `texts.len()` logical
/// events (DSH's `text-chunks`, whose `seq0` is the first of them). The gap
/// between a log's row count and its event count is what a copy must not
/// confuse when it stamps the inherited-prefix length.
fn packed_event_line(seq0: usize, texts: &[&str]) -> String {
    json!({
        "type": "text-chunks",
        "seq0": seq0,
        "time0": 1_700_000_000_000u64,
        "data": {
            "turn": 0,
            "step": 0,
            "index": 0,
            "dt": vec![1u64; texts.len().saturating_sub(1)],
            "texts": texts,
        }
    })
    .to_string()
}

fn zstd_all(lines: &[&str]) -> Vec<u8> {
    let joined = lines.join("\n") + "\n";
    zstd::encode_all(joined.as_bytes(), 3).unwrap()
}

fn multi_frame(frames: &[Vec<String>]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in frames {
        let joined = f.iter().map(|l| format!("{l}\n")).collect::<String>();
        out.extend(zstd::encode_all(joined.as_bytes(), 3).unwrap());
    }
    out
}

/// Decode a fixture as generation 0 — the generation `header_line` writes and
/// the one every pre-existing case here exercises. Generation-specific
/// behaviour has its own cases that pass the generation explicitly, because
/// that argument is exactly what a mismatch is caught by.
fn decode(buf: &[u8]) -> Result<DecodedLog, CodecError> {
    super::decode(buf, 0)
}

/// `header_line` for another format generation, with that generation's own
/// required fields filled (`isSeeded` is required from v2 on), so a fixture is
/// legal for its generation rather than rejected for an unrelated reason.
fn header_line_v(version: u64, id: &str, extra: Map<String, Value>) -> String {
    let mut m = Map::new();
    m.insert("type".into(), json!("session"));
    m.insert("version".into(), json!(version));
    m.insert("id".into(), json!(id));
    m.insert("createdAt".into(), json!(1_700_000_000_000u64));
    m.insert("cwd".into(), json!("D:\\work\\proj"));
    if version > HEADER_FORK_MAX_GENERATION {
        m.insert("isSeeded".into(), json!(false));
    }
    m.insert("delegationDepth".into(), json!(0));
    for (k, v) in extra {
        m.insert(k, v);
    }
    serde_json::to_string(&Value::Object(m)).unwrap()
}

/// DSH's rule, checked through the same gate the copy publishes through — a
/// fixture that would break a real target fails a test first.
fn assert_header_only_first_frame(bytes: &[u8]) {
    assert!(
        header_frame_is_alone(bytes).is_ok(),
        "first frame must hold exactly one newline-terminated header line"
    );
}

/// The publish gate must reject the exact shape that made instances unbootable,
/// so a re-encode regression fails the copy loudly instead of landing an
/// unbootable artifact in the target home.
#[test]
fn publish_gate_rejects_a_single_frame_log() {
    let header = header_line("session-m", Map::new());
    let single_frame = zstd_all(&[&header, &event_line(0)]);
    assert!(
        header_frame_is_alone(&single_frame).is_err(),
        "a log whose first frame also carries events must be rejected"
    );
    let framed = multi_frame(&[vec![header], vec![event_line(0)]]);
    assert!(header_frame_is_alone(&framed).is_ok());

    // The rule is about zstd framing: a plaintext log has none to violate, so
    // the gate must not reject it (that would break every `compression: none`
    // home's copies).
    let plain = format!(
        "{}\n{}\n",
        header_line("session-n", Map::new()),
        event_line(0)
    );
    assert!(header_frame_is_alone(plain.as_bytes()).is_ok());
}

fn reid<'a>(new_id: &'a str, parent: &'a str, inherited: usize) -> Reidentification<'a> {
    Reidentification {
        new_id,
        parent,
        inherited,
    }
}

#[test]
fn decodes_plaintext_and_reports_encoding_and_event_count() {
    let src = format!(
        "{}\n{}\n{}\n",
        header_line("session-a", Map::new()),
        event_line(0),
        event_line(1)
    );
    let log = decode(src.as_bytes()).unwrap();
    assert_eq!(log.encoding, LogEncoding::Plain);
    assert_eq!(log.header.id, "session-a");
    assert_eq!(log.header.cwd(), Some("D:\\work\\proj"));
    assert_eq!(log.event_count, 2);
    assert_eq!(log.event_values().len(), 2);
}

#[test]
fn decodes_single_and_multi_frame_zstd_identically() {
    let lines = [
        header_line("session-b", Map::new()),
        event_line(0),
        event_line(1),
        event_line(2),
    ];
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let single = decode(&zstd_all(&refs)).unwrap();
    let split = decode(&multi_frame(&[
        vec![lines[0].clone(), lines[1].clone()],
        vec![lines[2].clone()],
        vec![lines[3].clone()],
    ]))
    .unwrap();
    assert_eq!(single.encoding, LogEncoding::Zstd);
    assert_eq!(single.event_count, 3);
    assert_eq!(split.event_count, 3);
    assert_eq!(split.header.id, "session-b");
}

#[test]
fn rewrite_preserves_other_fields_and_only_touches_identity_keys() {
    let mut extra = Map::new();
    extra.insert("agentPreset".into(), json!("standard"));
    extra.insert("origin".into(), json!("subagent"));
    let src = format!("{}\n{}\n", header_line("session-c", extra), event_line(0));
    let log = decode(src.as_bytes()).unwrap();
    let rewritten = rewrite_header_line(
        &log.header,
        &Reidentification {
            new_id: "session-NEW",
            parent: "session-c",
            inherited: 1,
        },
    )
    .unwrap();
    let v: Value = serde_json::from_str(&rewritten).unwrap();
    assert_eq!(v["id"], "session-NEW");
    assert_eq!(v["parentSession"], "session-c");
    assert_eq!(v["seedLength"], 1);
    assert_eq!(v["version"], 0);
    // Untouched fields survive verbatim (spec §5.1 cwd + agentPreset).
    assert_eq!(v["cwd"], "D:\\work\\proj");
    assert_eq!(v["agentPreset"], "standard");
    assert_eq!(v["origin"], "subagent");
    assert_eq!(v["createdAt"], 1_700_000_000_000u64);
}

#[test]
fn copy_roundtrips_through_the_decoder_and_preserves_events() {
    let header = header_line("session-d", Map::new());
    let log = decode(&zstd_all(&[&header, &(event_line(0)), &(event_line(1))])).unwrap();
    let rewritten = rewrite_header_line(
        &log.header,
        &Reidentification {
            new_id: "session-D2",
            parent: "session-d",
            inherited: 2,
        },
    )
    .unwrap();
    let copied = log.build_copy(&rewritten).unwrap();
    // The copy is itself a valid zstd log the codec can re-read.
    let back = decode(&copied).unwrap();
    assert_eq!(back.header.id, "session-D2");
    assert_eq!(back.header.parent(), Some("session-d"));
    assert_eq!(back.event_count, 2);
    // Events are byte-identical to the source's decoded stream.
    assert_eq!(back.event_values(), log.event_values());
    assert_eq!(back.encoding, LogEncoding::Zstd);
}

#[test]
fn plaintext_copy_stays_plaintext() {
    let src = format!(
        "{}\n{}\n{}\n",
        header_line("session-e", Map::new()),
        event_line(0),
        event_line(1)
    );
    let log = decode(src.as_bytes()).unwrap();
    let rewritten = rewrite_header_line(
        &log.header,
        &Reidentification {
            new_id: "session-E2",
            parent: "session-e",
            inherited: 2,
        },
    )
    .unwrap();
    let out = log.build_copy(&rewritten).unwrap();
    assert_eq!(detect_encoding(&out), LogEncoding::Plain);
    assert_eq!(decode(&out).unwrap().header.id, "session-E2");
}

#[test]
fn unsupported_version_is_a_loud_refusal_not_a_guess() {
    let bad = json!({"type":"session","version":9,"id":"session-f"}).to_string();
    let log = decode(&zstd_all(&[&bad])).unwrap_err();
    assert_eq!(log, CodecError::UnsupportedVersion { found: 9 });
}

#[test]
fn rejects_non_session_first_line_and_garbage() {
    let wrong = json!({"type":"event","seq":0}).to_string();
    assert_eq!(
        decode(&zstd_all(&[&wrong])).unwrap_err(),
        CodecError::NotASessionLog
    );
    assert_eq!(decode(&[]).unwrap_err(), CodecError::MissingHeader);
    // Zstd magic but non-decodable body → FrameDecode, not a silent empty log.
    let mut corrupt = Vec::from(ZSTD_MAGIC);
    corrupt.extend_from_slice(&[0, 1, 2, 3, 4, 5]);
    assert!(matches!(
        decode(&corrupt).unwrap_err(),
        CodecError::FrameDecode(_)
    ));
}

#[test]
fn torn_final_line_is_dropped_matching_a_committed_prefix_read() {
    // No trailing newline on the last event → not counted (never corrupts seedLength).
    let src = format!(
        "{}\n{}\n{}",
        header_line("session-g", Map::new()),
        event_line(0),
        "{\"type\":\"partial\",\"seq\""
    );
    let log = decode(src.as_bytes()).unwrap();
    assert_eq!(log.event_count, 1);
}

#[test]
fn reidentification_overwrites_an_existing_lineage() {
    // Copying a session that was itself a fork must repoint parentSession to the
    // DIRECT source (the lineage the user copied), not keep a stale grandparent.
    let mut extra = Map::new();
    extra.insert("parentSession".into(), json!("session-grand"));
    extra.insert("seedLength".into(), json!(5));
    let src = format!("{}\n", header_line("session-h", extra));
    let log = decode(src.as_bytes()).unwrap();
    let rewritten = rewrite_header_line(
        &log.header,
        &Reidentification {
            new_id: "session-H2",
            parent: "session-h",
            inherited: 0,
        },
    )
    .unwrap();
    let v: Value = serde_json::from_str(&rewritten).unwrap();
    assert_eq!(v["parentSession"], "session-h");
    assert_eq!(v["seedLength"], 0);
}

/// Regression (2026-09-18): the old copy path compressed the whole log into one
/// frame, which DSH rejects at boot — `dsh-workspace` fails its scan and the
/// instance never starts. A single-frame source is exactly the shape an earlier
/// PHL copy produced, so this is the shape that must be handled.
#[test]
fn copy_frames_the_header_alone_even_when_the_source_is_one_frame() {
    let header = header_line("session-i", Map::new());
    let src = zstd_all(&[&header, &event_line(0), &event_line(1)]);
    let log = decode(&src).unwrap();
    let rewritten = rewrite_header_line(&log.header, &reid("session-I2", "session-i", 2)).unwrap();
    let copied = log.build_copy(&rewritten).unwrap();

    assert_eq!(
        split_frames(&src).len(),
        1,
        "fixture must be a single frame"
    );
    assert!(
        split_frames(&copied).len() >= 2,
        "copy must not stay a single frame"
    );
    assert_header_only_first_frame(&copied);

    let back = decode(&copied).unwrap();
    assert_eq!(back.header.id, "session-I2");
    assert_eq!(back.header.parent(), Some("session-i"));
    assert_eq!(back.event_count, 2);
    assert_eq!(back.event_bytes(), log.event_bytes());
}

/// A source that already carries the canonical shape keeps its event frames
/// byte-for-byte; only the header frame is rewritten.
#[test]
fn canonical_source_event_frames_are_carried_verbatim() {
    let header = header_line("session-j", Map::new());
    let src = multi_frame(&[
        vec![header.clone()],
        vec![event_line(0)],
        vec![event_line(1), event_line(2)],
    ]);
    let log = decode(&src).unwrap();
    let rewritten = rewrite_header_line(&log.header, &reid("session-J2", "session-j", 3)).unwrap();
    let copied = log.build_copy(&rewritten).unwrap();

    let src_ranges = split_frames(&src);
    let out_ranges = split_frames(&copied);
    assert_eq!(
        out_ranges.len(),
        src_ranges.len(),
        "frame count is preserved"
    );
    assert_header_only_first_frame(&copied);
    for (i, (s, e)) in src_ranges.iter().enumerate().skip(1) {
        assert_eq!(
            &copied[out_ranges[i].0..out_ranges[i].1],
            &src[*s..*e],
            "event frame {i} must travel byte-for-byte"
        );
    }
    assert_eq!(decode(&copied).unwrap().event_bytes(), log.event_bytes());
}

/// The framing-reuse path must not undo the committed-prefix rule: a torn tail
/// the decoder drops may never reappear in the copy.
#[test]
fn a_dropped_torn_tail_never_travels_into_the_copy() {
    let header = header_line("session-k", Map::new());
    // Built by hand (not via `multi_frame`, which newline-terminates every line)
    // so the last frame really is a crash-torn, uncommitted tail.
    let mut src = Vec::new();
    src.extend(zstd::encode_all(format!("{header}\n").as_bytes(), 3).unwrap());
    src.extend(zstd::encode_all(format!("{}\n", event_line(0)).as_bytes(), 3).unwrap());
    src.extend(zstd::encode_all("{\"type\":\"partial\",\"seq\"".as_bytes(), 3).unwrap());

    let log = decode(&src).unwrap();
    assert_eq!(log.event_count, 1, "the torn record is uncommitted");
    let rewritten = rewrite_header_line(&log.header, &reid("session-K2", "session-k", 1)).unwrap();
    let copied = log.build_copy(&rewritten).unwrap();

    assert_header_only_first_frame(&copied);
    let back = decode(&copied).unwrap();
    assert_eq!(back.event_count, 1);
    assert_eq!(
        String::from_utf8_lossy(back.event_bytes()),
        format!("{}\n", event_line(0)),
        "the copy holds exactly the committed prefix"
    );
}

/// A header-only log copies into a header-only log — no empty event frame.
#[test]
fn header_only_source_copies_without_an_empty_event_frame() {
    let header = header_line("session-l", Map::new());
    let log = decode(&zstd_all(&[&header])).unwrap();
    assert_eq!(log.event_count, 0);
    let rewritten = rewrite_header_line(&log.header, &reid("session-L2", "session-l", 0)).unwrap();
    let copied = log.build_copy(&rewritten).unwrap();

    assert_header_only_first_frame(&copied);
    assert_eq!(
        split_frames(&copied).len(),
        1,
        "no events → no second frame"
    );
    let back = decode(&copied).unwrap();
    assert_eq!(back.event_count, 0);
    assert_eq!(back.header.id, "session-L2");
}

/* ------------------------- format generations ------------------------- */

/// The canonical naming rule, both directions: a generation names a file and a
/// file names a generation. This is the pairing DSH scans a home by.
#[test]
fn generation_filenames_round_trip_through_canonical_parsing() {
    for (generation, plain, zstd) in [
        (0u64, "session.jsonl", "session.jsonl.zstd"),
        (1, "session.v1.jsonl", "session.v1.jsonl.zstd"),
        (3, "session.v3.jsonl", "session.v3.jsonl.zstd"),
        (42, "session.v42.jsonl", "session.v42.jsonl.zstd"),
    ] {
        assert_eq!(generation_filename(generation, LogEncoding::Plain), plain);
        assert_eq!(generation_filename(generation, LogEncoding::Zstd), zstd);
        assert_eq!(
            generation_of_filename(plain, LogEncoding::Plain),
            Some(generation)
        );
        assert_eq!(
            generation_of_filename(zstd, LogEncoding::Zstd),
            Some(generation)
        );
    }
}

/// Names DSH's own parser rejects are not generations here either: accepting
/// them would read artifacts the owner never sees, or read the wrong encoding.
#[test]
fn non_canonical_artifact_names_are_not_generations() {
    for name in [
        ".session.jsonl.zstd",         // temporary stage
        ".session.v3.jsonl.zstd.part", // temporary stage
        "SESSION.jsonl.zstd",          // uppercase
        "session.v0.jsonl.zstd",       // `.v0` is spelled without the segment
        "session.v01.jsonl.zstd",      // leading zero
        "session.v.jsonl.zstd",        // no digits
        "session.v3.jsonl.bak",        // not a log
        "session.v3.jsonl",            // wrong suffix for a zstd home
        "session.jsonl",               // wrong suffix for a zstd home
    ] {
        assert_eq!(
            generation_of_filename(name, LogEncoding::Zstd),
            None,
            "{name} must not resolve as a zstd generation"
        );
    }
    // The plain half of the same rule: a zstd-suffixed name is not plaintext.
    assert_eq!(
        generation_of_filename("session.jsonl.zstd", LogEncoding::Plain),
        None
    );
}

/// DSH refuses a directory whose filename generation and header version
/// disagree ("session generation filename identifies vN, but its header
/// identifies vM"), so reading must refuse it too — never silently trust one
/// half of the pair.
#[test]
fn a_header_must_agree_with_its_artifact_generation() {
    let v0_header = header_line("session-g", Map::new());
    assert_eq!(
        super::decode(&zstd_all(&[&v0_header]), 3).unwrap_err(),
        CodecError::GenerationMismatch {
            filename: 3,
            header: 0
        }
    );
    let v3_header = header_line_v(3, "session-h", Map::new());
    assert_eq!(
        super::decode(&zstd_all(&[&v3_header]), 0).unwrap_err(),
        CodecError::GenerationMismatch {
            filename: 0,
            header: 3
        }
    );
}

/// The regression this generation work exists for: a copy stamps `seedLength`
/// into the header, and from v2 on that key is outside the generation's exact
/// whitelist. DSH reads such a header as corrupt, fails the home's whole scan
/// and refuses to boot — so the parser must reject the shape long before a
/// copy could publish it.
#[test]
fn v2_and_v3_headers_reject_the_v0_fork_marker() {
    let mut extra = Map::new();
    extra.insert("parentSession".into(), json!("session-p"));
    extra.insert("seedLength".into(), json!(4));
    let stamped = header_line_v(3, "session-c", extra);
    let err = super::decode(&zstd_all(&[&stamped]), 3).unwrap_err();
    assert!(
        matches!(err, CodecError::MalformedHeader(ref m) if m.contains("seedLength")),
        "a v3 header carrying seedLength must be refused, got {err:?}"
    );
}

/// `isSeeded` is required from v2 on — its absence is a malformed header, not a
/// default to assume.
#[test]
fn v2_and_v3_headers_require_is_seeded() {
    let hand_rolled = json!({
        "type": "session", "version": 3, "id": "session-b",
        "createdAt": 1_700_000_000_000u64, "cwd": "D:\\work\\proj", "delegationDepth": 0
    })
    .to_string();
    let err = super::decode(&zstd_all(&[&hand_rolled]), 3).unwrap_err();
    assert!(
        matches!(err, CodecError::MalformedHeader(ref m) if m.contains("isSeeded")),
        "v3 without isSeeded must be refused, got {err:?}"
    );
}

/// The positive control: a legal v2/v3 header decodes, and so does the fork
/// shape DSH itself writes for a subagent child (a parent, no seed marker).
#[test]
fn legal_v2_and_v3_headers_decode_including_a_subagent_fork() {
    for version in [2u64, 3] {
        let plain = header_line_v(version, "session-a", Map::new());
        let log = super::decode(&zstd_all(&[&plain]), version).unwrap();
        assert_eq!(log.header.id, "session-a");
        assert_eq!(log.header.generation, version);
        assert!(!log.header.supports_header_fork());

        let mut extra = Map::new();
        extra.insert("parentSession".into(), json!("session-p"));
        extra.insert("origin".into(), json!("subagent"));
        extra.insert("delegationDepth".into(), json!(1));
        let child = header_line_v(version, "session-a2", extra);
        let log = super::decode(&zstd_all(&[&child]), version).unwrap();
        assert_eq!(log.header.parent(), Some("session-p"));
    }
}

/// v0/v1 are the generations whose fork marker *is* in the header, so the copy
/// rewrite is legal there and refused everywhere else — the rewrite is the one
/// place the marker is written, so it is the last line of defence.
#[test]
fn the_copy_rewrite_is_refused_for_generations_without_a_header_marker() {
    let v0 = super::decode(&zstd_all(&[&header_line("session-n", Map::new())]), 0).unwrap();
    assert!(v0.header.supports_header_fork());
    assert!(rewrite_header_line(&v0.header, &reid("session-N2", "session-n", 0)).is_ok());

    let v1 = super::decode(&zstd_all(&[&header_line_v(1, "session-n1", Map::new())]), 1).unwrap();
    assert!(v1.header.supports_header_fork());
    assert!(rewrite_header_line(&v1.header, &reid("session-N3", "session-n1", 0)).is_ok());

    let v3 = super::decode(&zstd_all(&[&header_line_v(3, "session-n3", Map::new())]), 3).unwrap();
    assert!(!v3.header.supports_header_fork());
    assert!(rewrite_header_line(&v3.header, &reid("session-N4", "session-n3", 0)).is_err());
}

/// A generation newer than the installed catalog is a compatibility refusal —
/// never a guess about bytes this build has no codec for.
#[test]
fn versions_beyond_the_installed_catalog_are_refused() {
    let future = header_line_v(MAX_READABLE_GENERATION + 1, "session-z", Map::new());
    assert_eq!(
        super::decode(&zstd_all(&[&future]), MAX_READABLE_GENERATION + 1).unwrap_err(),
        CodecError::UnsupportedVersion {
            found: MAX_READABLE_GENERATION + 1
        }
    );
}

/* ------------------- inherited prefix (the fork's cut) ------------------- */

/// The cut a fork records is the log's **logical event count**: a packed
/// Assistant row advances it by its payload length, a normal row by one. The
/// row count (what the pre-fix copy stamped) is a different number.
#[test]
fn the_inherited_count_expands_packed_assistant_rows() {
    let header = header_line("session-pack", Map::new());
    let log = decode(&zstd_all(&[
        &header,
        &event_line(0),
        &packed_event_line(1, &["a", "b", "c"]),
        &event_line(4),
    ]))
    .unwrap();

    assert_eq!(log.event_count, 3, "three stored rows");
    assert_eq!(
        log.inherited_event_count().unwrap(),
        5,
        "five logical events: 1 + 3 packed + 1"
    );
}

/// The same rule through the copy: the landed fork's `seedLength` is the
/// logical count, and DSH re-reads that same number from the copy's rows.
#[test]
fn a_copy_of_a_packed_log_records_the_logical_event_count() {
    let header = header_line("session-pack-copy", Map::new());
    let log = decode(&zstd_all(&[
        &header,
        &event_line(0),
        &packed_event_line(1, &["a", "b", "c"]),
        &event_line(4),
    ]))
    .unwrap();
    let cut = log.inherited_event_count().unwrap();
    let rewritten = rewrite_header_line(
        &log.header,
        &Reidentification {
            new_id: "session-P2",
            parent: "session-pack-copy",
            inherited: cut,
        },
    )
    .unwrap();
    let copied = log.build_copy(&rewritten).unwrap();
    let back = decode(&copied).unwrap();

    assert_eq!(back.event_count, 3, "rows travel verbatim");
    assert_eq!(
        back.inherited_event_count().unwrap(),
        5,
        "and so does the event count"
    );
    assert_eq!(
        back.header.value.get("seedLength").and_then(Value::as_u64),
        Some(5),
        "the fork records the cut DSH expects, not the row count"
    );
}

/// A row DSH itself refuses (a seq gap) cannot be counted, so it must refuse
/// the copy rather than produce a fork with a cut nobody can vouch for.
#[test]
fn the_inherited_count_refuses_the_seq_gap_dsh_refuses() {
    let header = header_line("session-gap", Map::new());
    let log = decode(&zstd_all(&[&header, &event_line(0), &event_line(7)])).unwrap();
    let err = log.inherited_event_count().unwrap_err();
    assert!(
        matches!(err, CodecError::RowShape(ref detail) if detail.contains("seq gap")),
        "a seq gap must be refused, got {err:?}"
    );

    let header = header_line("session-noseq", Map::new());
    let log = decode(&zstd_all(&[
        &header,
        &json!({"type": "user/message"}).to_string(),
    ]))
    .unwrap();
    assert!(matches!(
        log.inherited_event_count().unwrap_err(),
        CodecError::RowShape(_)
    ));
}
