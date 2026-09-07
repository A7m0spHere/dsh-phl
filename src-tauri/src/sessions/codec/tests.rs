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
