//! Operation journal & report tests (acceptance E01/E03 shapes).

use super::*;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-ops-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample(id: &str, status: &str) -> OperationRecord {
    OperationRecord {
        operation_id: id.into(),
        kind: "trial-create".into(),
        source_id: "daily".into(),
        target_id: "trial-daily".into(),
        label: "复制并试用新版 → 副本".into(),
        status: status.into(),
        detail: {
            let mut d = serde_json::Map::new();
            d.insert(
                "dshVersion".into(),
                serde_json::Value::String("0.1.7-rc.2".into()),
            );
            d
        },
        error: None,
        started_at: "2026-09-25T00:00:00Z".into(),
        finished_at: None,
        retry_of: None,
    }
}

#[test]
fn journal_roundtrips_and_skips_torn_lines() {
    let root = scratch("roundtrip");
    append_record(&root, &sample("op-1", "committed")).unwrap();
    append_record(&root, &sample("op-2", "failed")).unwrap();

    // Simulate a crash mid-append: a torn line without JSON terminator.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(journal_path(&root))
        .unwrap();
    f.write_all(b"{\"operationId\":\"op-3\",\"stat").unwrap();

    let records = load_records(&root);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].operation_id, "op-1");
    assert_eq!(records[1].status, "failed");
    let _ = std::fs::remove_dir_all(&root);
}

/// E01 shape: a crash leaves `running`; the next boot marks it interrupted
/// and finished — never a fake success.
#[test]
fn boot_recovery_marks_running_rows_interrupted() {
    let root = scratch("recover");
    append_record(&root, &sample("op-run", "running")).unwrap();
    append_record(&root, &sample("op-done", "committed")).unwrap();

    let recovered = recover_interrupted(&root);
    assert_eq!(recovered, 1);
    let records = load_records(&root);
    assert_eq!(records.len(), 3);
    let interrupted: Vec<_> = records
        .iter()
        .filter(|r| r.status == "interrupted")
        .collect();
    assert_eq!(interrupted.len(), 1);
    assert!(interrupted[0].finished_at.is_some());
    assert!(interrupted[0]
        .error
        .as_deref()
        .unwrap_or("")
        .contains("退出"));
    let _ = std::fs::remove_dir_all(&root);
}

/// F09/E03 shape: the report scrubber removes token-shaped values from log
/// text — query tokens, Bearer values and sk- keys — while leaving normal
/// text alone.
#[test]
fn report_scrubber_removes_credential_shaped_values() {
    let log = concat!(
        "GET /web?token=vF9xTESTTOKEN123&next=1 200\n",
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9TEST\n",
        "config api_key = sk-test-abcdef012345\n",
        "profile web started on port 8123\n",
    );
    let scrubbed = scrub_all(log);
    assert!(!scrubbed.contains("vF9xTESTTOKEN123"), "{scrubbed}");
    assert!(!scrubbed.contains("eyJhbGciOiJIUzI1NiJ9TEST"), "{scrubbed}");
    assert!(!scrubbed.contains("sk-test-abcdef012345"), "{scrubbed}");
    assert!(scrubbed.contains("[已脱敏]"));
    // Non-credential lines survive verbatim.
    assert!(scrubbed.contains("profile web started on port 8123"));
}

/// Retention: finished rows past the soft limit drop; failed rows and
/// running rows survive even past the limit.
#[test]
fn pruner_keeps_failures_and_active_rows() {
    let root = scratch("prune");
    // A failure far in the past, some finished rows, one running row.
    append_record(&root, &sample("op-fail", "failed")).unwrap();
    for i in 0..(JOURNAL_SOFT_LIMIT + 20) {
        append_record(&root, &sample(&format!("op-{i:03}"), "committed")).unwrap();
    }
    append_record(&root, &sample("op-running", "running")).unwrap();

    let dropped = prune_journal(&root);
    assert!(dropped > 0);
    let records = load_records(&root);
    assert!(
        records.iter().any(|r| r.operation_id == "op-fail"),
        "failure evidence must survive the pruner"
    );
    assert!(
        records.iter().any(|r| r.operation_id == "op-running"),
        "active rows must survive the pruner"
    );
    assert!(records.len() <= JOURNAL_SOFT_LIMIT + 2);
    let _ = std::fs::remove_dir_all(&root);
}

/* ---------------- Codex review probes, promoted (CR-04/05) ---------------- */

/// CR-04: begin→committed then a restart must recover nothing and project
/// the operation as committed.
#[test]
fn committed_operation_survives_restart_without_being_marked_interrupted() {
    let root = scratch("lifecycle");
    let span = begin(&root, sample("op-life", "running")).unwrap();
    span.finish("committed", None, {
        let mut d = serde_json::Map::new();
        d.insert(
            "readiness".into(),
            serde_json::Value::String("readyToLaunch".into()),
        );
        d
    });

    assert_eq!(
        recover_interrupted(&root),
        0,
        "a committed operation is not running"
    );
    let latest: Vec<_> = load_latest_records(&root);
    assert_eq!(latest.len(), 1, "one operation, projected from its events");
    assert_eq!(latest[0].status, "committed");

    // Second restart after recovery: still nothing to do.
    assert_eq!(recover_interrupted(&root), 0);
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-04: a torn append between begin and finish must not break the
/// projection of the finished operation.
#[test]
fn torn_tail_does_not_shadow_the_latest_event() {
    use std::io::Write;
    let root = scratch("torn-lifecycle");
    let span = begin(&root, sample("op-torn", "running")).unwrap();
    span.finish("committed", None, Default::default());
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(journal_path(&root))
        .unwrap();
    f.write_all(b"{\"operationId\":\"op-torn\",\"sta").unwrap();

    assert_eq!(recover_interrupted(&root), 0);
    let latest: Vec<_> = load_latest_records(&root);
    assert_eq!(latest[0].status, "committed");
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-04: the finishing event must merge its fields into the plan facts
/// recorded at begin, not replace them.
#[test]
fn finish_merges_detail_instead_of_replacing_it() {
    let root = scratch("merge");
    let mut begin_detail = serde_json::Map::new();
    begin_detail.insert(
        "dshVersion".into(),
        serde_json::Value::String("0.1.7-rc.2".into()),
    );
    begin_detail.insert("scope".into(), serde_json::Value::String("config".into()));
    let mut record = sample("op-merge", "running");
    record.detail = begin_detail;
    let span = begin(&root, record).unwrap();
    let mut updates = serde_json::Map::new();
    updates.insert(
        "readiness".into(),
        serde_json::Value::String("readyToLaunch".into()),
    );
    span.finish("committed", None, updates);

    let latest = load_latest_records(&root);
    assert_eq!(latest[0].detail.get("dshVersion").unwrap(), "0.1.7-rc.2");
    assert_eq!(latest[0].detail.get("readiness").unwrap(), "readyToLaunch");
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-05: the exported report must show the operation's latest status, and
/// credential-shaped values anywhere in the tree (error text included,
/// upper-case env names included) must be gone from both files.
#[tokio::test]
async fn exported_report_projects_latest_state_and_scrubs_the_whole_tree() {
    let root = scratch("export");
    let mut record = sample("op-export", "running");
    record.error =
        Some("API_KEY=review-only-fake-secret at auth url ?token=FakeProbeToken123".into());
    begin(&root, record).unwrap();
    // Finish with an error that itself carries a secret: the projection
    // must pick this event AND scrub it.
    let mut updates = serde_json::Map::new();
    updates.insert(
        "readiness".into(),
        serde_json::Value::String("readyToLaunch".into()),
    );
    let span = begin(&root, sample("op-export", "running")).unwrap();
    span.finish(
        "committed",
        Some("committed with API_KEY=review-only-fake-secret in ?token=FakeProbeToken123".into()),
        updates,
    );

    let out_dir = root.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let ReportResult {
        json_path,
        markdown_path,
        ..
    } = export_operation_report_inner(
        &root,
        "op-export",
        out_dir.join("report").to_string_lossy().as_ref(),
    )
    .unwrap();

    let json = std::fs::read_to_string(&json_path).unwrap();
    let md = std::fs::read_to_string(&markdown_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("scrubbed JSON stays valid");
    assert_eq!(
        parsed["operation"]["status"], "committed",
        "latest event wins"
    );
    // The projected error is the scrubbed version of the latest event's.
    assert!(parsed["operation"]["error"]
        .as_str()
        .unwrap()
        .contains("[已脱敏]"));
    for secret in ["review-only-fake-secret", "FakeProbeToken123", "API_KEY="] {
        assert!(!json.contains(secret), "JSON leaked {secret}");
        assert!(!md.contains(secret), "Markdown leaked {secret}");
    }
    assert!(json.contains("[已脱敏]"));
    // Honest markdown: committed shows verified claims.
    assert!(md.contains("已完成"));
    let _ = std::fs::remove_dir_all(&root);
}

/// CR-05: a failed operation's report must not claim verified completion.
#[tokio::test]
async fn failed_report_does_not_claim_verified_items() {
    let root = scratch("failed-report");
    let mut record = sample("op-fail", "running");
    record.error = Some("复制失败: 磁盘空间不足".into());
    let span = begin(&root, record).unwrap();
    span.finish(
        "failed",
        Some("复制失败: 磁盘空间不足".into()),
        Default::default(),
    );

    let out_dir = root.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let ReportResult { markdown_path, .. } = export_operation_report_inner(
        &root,
        "op-fail",
        out_dir.join("report").to_string_lossy().as_ref(),
    )
    .unwrap();
    let md = std::fs::read_to_string(&markdown_path).unwrap();
    assert!(md.contains("失败"));
    assert!(md.contains("未确认任何完成项"));
    let _ = std::fs::remove_dir_all(&root);
}

/* ------------------- R2-05 promotion tests ------------------- */

/// R2-05: the probe input that panicked the old implementation — a short
/// value right at the end of the text. Must not panic, must scrub.
#[test]
fn scrubber_handles_short_values_at_end_of_text() {
    let result = std::panic::catch_unwind(|| scrub_all("token=x"));
    let out = result.unwrap_or_else(|_| "(panic)".into());
    assert_eq!(out, "[已脱敏]", "got: {out}");
}

/// R2-05: several credentials on one line, the same needle twice, mixed
/// case, and CJK text adjacent to values — all in one pass.
#[test]
fn scrubber_handles_multiple_and_repeated_fields() {
    let out = scrub_all("token=a1&API_KEY=b2&api_key=c3&token=d4 secret=e5 密码password=f6 tail");
    for secret in ["a1", "b2", "c3", "d4", "e5", "f6"] {
        assert!(!out.contains(secret), "{secret} leaked: {out}");
    }
    assert_eq!(out.matches("[已脱敏]").count(), 6, "{out}");
    assert!(out.contains("密码"), "CJK text outside values survives");
    assert!(out.contains("tail"));
}

/// R2-05: a needle inside a longer identifier is not a credential
/// (`monkey=1` stays, `key=1` goes).
#[test]
fn scrubber_respects_word_boundaries() {
    let out = scrub_all("monkey=1 and key=2");
    assert!(out.contains("monkey=1"), "{out}");
    assert!(!out.contains("key=2"), "{out}");
}

/// R2-05: unicode case expansion (İ lowercases to two chars) must not
/// break the byte-offset math — the scan is ASCII-only and in-place.
#[test]
fn scrubber_is_safe_around_unicode() {
    let out = scrub_all("İİ token=abc key=v İİ");
    assert!(!out.contains("abc"), "{out}");
    assert!(!out.contains("İİ token"), "prefix before a match must stay");
    assert!(out.contains("İİ"), "trailing unicode survives: {out}");
}

/// R2-05 end-to-end: the exported files must not panic, must stay valid
/// JSON, and must not leak — with all probe shapes planted at once.
#[tokio::test]
async fn export_scrubs_every_probe_shape_without_panicking() {
    let root = scratch("r2scrub");
    let mut record = sample("op-r2", "running");
    record.error = Some(
        "token=x Bearer abc.def.ghi API_KEY=UppercaseSecret sk-short 授权key=中文值 done".into(),
    );
    let span = begin(&root, record).unwrap();
    span.finish("committed", None, Default::default());

    let out_dir = root.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let ReportResult {
        json_path,
        markdown_path,
        ..
    } = export_operation_report_inner(
        &root,
        "op-r2",
        out_dir.join("report").to_string_lossy().as_ref(),
    )
    .unwrap();
    let json = std::fs::read_to_string(&json_path).unwrap();
    serde_json::from_str::<serde_json::Value>(&json).expect("scrubbed JSON stays valid");
    for secret in ["UppercaseSecret", "abc.def.ghi", "中文值", "sk-short"] {
        assert!(!json.contains(secret), "JSON leaked {secret}");
        assert!(!std::fs::read_to_string(&markdown_path)
            .unwrap()
            .contains(secret));
    }
    let _ = std::fs::remove_dir_all(&root);
}
