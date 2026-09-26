//! Operation records & diagnostic reports (R1 · M5).
//!
//! Scope is deliberately narrow: only the R1 flows (trial copies, community
//! pack installs, dependency preparation) and the launches of the instances
//! they created are journaled. The journal is an append-only JSONL beside
//! the data root's pointer file — not a second source of truth for instances
//! (the manifests stay that), just the trace of *operations*.
//!
//! A crash or hard kill leaves `running` rows behind: the next boot marks
//! them `interrupted` instead of pretending they succeeded. Exported reports
//! scrub credential-shaped values from the bounded log tail they carry.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::paths::PhlState;

/// Records older than the window are prunable — but only finished,
/// non-evidence rows (the pruner enforces the pairing).
pub(crate) const JOURNAL_MAX_AGE_DAYS: i64 = 90;
pub(crate) const JOURNAL_SOFT_LIMIT: usize = 100;

fn journal_path(root: &Path) -> PathBuf {
    root.join("operations.jsonl")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub operation_id: String,
    /// `trial-create` | `community-pack-install` | `pack-dependencies` | `launch-verify`
    pub kind: String,
    /// The instance the operation reads/copies.
    #[serde(default)]
    pub source_id: String,
    /// The instance the operation creates or acts on.
    #[serde(default)]
    pub target_id: String,
    pub label: String,
    /// `running` | `committed` | `failed` | `cancelled` | `interrupted`
    pub status: String,
    /// Free-form, non-secret plan summary: versions, scope, counts.
    #[serde(default)]
    pub detail: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    /// Present when this operation retries an earlier one.
    #[serde(default)]
    pub retry_of: Option<String>,
}

/// Appends one record line. The journal tolerates a torn last line (a crash
/// mid-write): readers skip malformed rows instead of failing.
pub(crate) fn append_record(root: &Path, record: &OperationRecord) -> Result<(), String> {
    use std::io::BufWriter;
    let path = journal_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, record).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(())
}

/// Loads every well-formed record, newest last; a torn trailing line from a
/// crashed append is skipped rather than failing the whole journal.
pub(crate) fn load_records(root: &Path) -> Vec<OperationRecord> {
    let Ok(file) = std::fs::File::open(journal_path(root)) else {
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    reader
        .lines()
        .map_while(|l| l.ok())
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

/// The current state of every operation: the journal is append-only, so a
/// projection takes the LAST well-formed event per operation id, in file
/// order. Every consumer — listing, recovery, export, retention — works on
/// this projection, never on the raw event stream (CR-04).
pub(crate) fn load_latest_records(root: &Path) -> Vec<OperationRecord> {
    let events = load_records(root);
    let mut latest: std::collections::BTreeMap<String, OperationRecord> =
        std::collections::BTreeMap::new();
    for record in events {
        latest.insert(record.operation_id.clone(), record);
    }
    latest.into_values().collect()
}

/// Boot-time reconciliation: operations whose LATEST state is still
/// `running` when the app starts were orphaned by a crash — mark them
/// interrupted so the history never claims success it cannot prove (E01).
/// An operation that already recorded `committed` stays committed.
pub(crate) fn recover_interrupted(root: &Path) -> usize {
    let records = load_latest_records(root);
    let mut recovered = 0usize;
    for record in records {
        if record.status != "running" {
            continue;
        }
        let closed = OperationRecord {
            status: "interrupted".into(),
            finished_at: Some(crate::versions::now_iso()),
            error: Some("应用在操作进行中退出；结果未知，请通过详情页核对".into()),
            ..record
        };
        if append_record(root, &closed).is_ok() {
            recovered += 1;
        }
    }
    recovered
}

/// `YYYY-MM-DD` `days` days ago (ISO-8601 prefix compare is a total order
/// for same-format timestamps — no date library needed for a 90-day rule).
fn iso_days_ago(days: i64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days_epoch = secs.div_euclid(86_400) - days;
    // Civil-from-days (Howard Hinnant's algorithm), Gregorian.
    let z = days_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Retention: drop finished records beyond the soft limit / age window.
/// Active (`running`) and failure-evidence rows survive both rules.
pub(crate) fn prune_journal(root: &Path) -> usize {
    let records = load_latest_records(root);
    if records.len() <= JOURNAL_SOFT_LIMIT {
        return 0;
    }
    // Newest-first, but failures/active rows never count against the limit:
    // only finished, non-evidence rows are eligible for removal.
    let mut keep: Vec<&OperationRecord> = Vec::new();
    let mut dropped = 0usize;
    let cutoff_prefix = iso_days_ago(JOURNAL_MAX_AGE_DAYS);
    for record in records.iter().rev() {
        let finished_ok = record.status == "committed" || record.status == "cancelled";
        let stale = record.started_at.as_str() < cutoff_prefix.as_str();
        if finished_ok && (stale || keep.len() >= JOURNAL_SOFT_LIMIT) {
            dropped += 1;
        } else {
            keep.push(record);
        }
    }
    keep.reverse();
    if dropped == 0 {
        return 0;
    }
    let path = journal_path(root);
    let tmp = path.with_extension("jsonl.tmp");
    if let Ok(mut out) = std::fs::File::create(&tmp) {
        use std::io::Write;
        for record in keep {
            if let Ok(line) = serde_json::to_string(record) {
                let _ = writeln!(out, "{line}");
            }
        }
        let _ = std::fs::rename(&tmp, &path);
    }
    dropped
}

/* ------------------------------- commands ------------------------------ */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub operation_id: String,
    pub kind: String,
    pub source_id: String,
    pub target_id: String,
    pub label: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub detail: serde_json::Map<String, serde_json::Value>,
}

#[tauri::command]
pub async fn list_operations(
    phl: State<'_, PhlState>,
    instance_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<OperationSummary>, String> {
    let root = phl.root().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut records = load_latest_records(&root);
        records.reverse(); // newest first
        if let Some(id) = &instance_id {
            records.retain(|r| r.source_id == *id || r.target_id == *id);
        }
        let limit = limit.unwrap_or(50).clamp(1, 200) as usize;
        Ok(records
            .into_iter()
            .take(limit)
            .map(|r| OperationSummary {
                operation_id: r.operation_id,
                kind: r.kind,
                source_id: r.source_id,
                target_id: r.target_id,
                label: r.label,
                status: r.status,
                started_at: r.started_at,
                finished_at: r.finished_at,
                error: r.error,
                detail: r.detail,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResult {
    pub json_path: String,
    pub markdown_path: String,
    pub sha256: String,
}

/// Token-shaped sequences and credential values must not travel with a
/// shared report (E03). This scrubs the *text* layer of a report: anything
/// that looks like an auth token or key material is replaced.
///
/// Implementation is a **single left-to-right scan over the original text**
/// collecting non-overlapping replacement spans, then one assembly pass —
/// no index is ever reused against a mutated string (R2-05: the old
/// find-on-lowercase/replace-on-original loop panicked on short values and
/// desynced after every replacement). Case-insensitivity is ASCII-only and
/// length-preserving, so match positions are valid in the original.
pub(crate) fn scrub_all(text: &str) -> String {
    let needles: &[&str] = &[
        "token=",
        "key=",
        "apikey=",
        "api_key=",
        "password=",
        "secret=",
        "dsh_auth=",
    ];
    let prefixes: &[&str] = &["bearer ", "sk-"];

    let bytes = text.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched = false;
        // `name=value` — the whole name+value span is credential-shaped in
        // a shared report (CR-05). Matching happens **only at i** so spans
        // are collected in text order; a later needle can never jump over
        // an earlier credential (the R2-05 regression class).
        for needle in needles {
            if !ascii_eq_at(bytes, i, needle.as_bytes()) {
                continue;
            }
            // Word boundary: `monkey=` is not a `key=` credential.
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let value_start = i + needle.len();
            let value_stop = value_stop(bytes, value_start);
            if prev_ok && value_stop > value_start {
                spans.push((i, value_stop));
                i = value_stop;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // `Bearer <jwt>` / `sk-<key>` — the value itself travels.
        for prefix in prefixes {
            if !ascii_eq_at(bytes, i, prefix.as_bytes()) {
                continue;
            }
            let value_start = i + prefix.len();
            let value_stop = prefix_value_stop(bytes, value_start);
            if value_stop > value_start + 3 {
                spans.push((i, value_stop));
                i = value_stop;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // Advance one full UTF-8 character — never a bare byte.
        let mut next = i + 1;
        while next < bytes.len() && !text.is_char_boundary(next) {
            next += 1;
        }
        i = next;
    }

    // Assemble once, forward, over stable spans.
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for (start, stop) in spans {
        out.push_str(&text[cursor..start]);
        out.push_str("[已脱敏]");
        cursor = stop;
    }
    out.push_str(&text[cursor..]);
    out
}

/// ASCII-case-insensitive comparison of `needle` against `haystack`
/// starting exactly at `offset`.
fn ascii_eq_at(haystack: &[u8], offset: usize, needle: &[u8]) -> bool {
    haystack.len() - offset >= needle.len()
        && haystack[offset..offset + needle.len()]
            .iter()
            .zip(needle)
            .all(|(h, n)| h.eq_ignore_ascii_case(n))
}

/// End of the `name=value` run: up to the first delimiter. A newline is
/// always a delimiter so one value can never swallow the next line.
fn value_stop(bytes: &[u8], from: usize) -> usize {
    let mut end = from;
    while end < bytes.len() {
        let b = bytes[end];
        if matches!(b, b'&' | b' ' | b'"' | b',' | b']' | b'\n' | b'\r') {
            break;
        }
        end += 1;
    }
    end
}

/// End of a prefixed value run: whitespace and quotes end it.
fn prefix_value_stop(bytes: &[u8], from: usize) -> usize {
    let mut end = from;
    while end < bytes.len() {
        let b = bytes[end];
        if matches!(b, b' ' | b'"' | b',' | b'\n' | b'\r') {
            break;
        }
        end += 1;
    }
    end
}

/// Test hook: synchronous export against a root, without the Tauri state.
#[cfg(test)]
pub(crate) fn export_operation_report_inner(
    root: &Path,
    operation_id: &str,
    destination: &str,
) -> Result<ReportResult, String> {
    // Same body as the command, factored for reuse.
    export_operation_report_body(root, operation_id, destination)
}

#[tauri::command]
pub async fn export_operation_report(
    phl: State<'_, PhlState>,
    operation_id: String,
    destination: String,
) -> Result<ReportResult, String> {
    let root = phl.root().to_path_buf();
    tokio::task::spawn_blocking(move || {
        export_operation_report_body(&root, &operation_id, &destination)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn export_operation_report_body(
    root: &Path,
    operation_id: &str,
    destination: &str,
) -> Result<ReportResult, String> {
    {
        // Latest event for the id: a running→committed pair must export as
        // committed (CR-04).
        let record = load_latest_records(root)
            .into_iter()
            .find(|r| r.operation_id == operation_id)
            .ok_or_else(|| "操作记录不存在".to_string())?;

        // Bounded, scrubbed log tail: the local launch log is not a report.
        let log_tail = read_launch_log_tail(root, &record)
            .map(|tail| scrub_all(&tail))
            .unwrap_or_else(|| "(无日志)".into());

        let report_json = serde_json::json!({
            "schemaVersion": 1,
            "operation": record,
            "environment": {
                "phlVersion": env!("CARGO_PKG_VERSION"),
                "dshVersion": record.detail.get("dshVersion"),
                "nodeVersion": record.detail.get("nodeVersion"),
            },
            "logTail": log_tail,
            "unverified": [
                "插件加载检查",
                "实际模型任务",
            ],
        });
        // The WHOLE tree is scrubbed, not only the log tail: error text,
        // labels and detail entries may carry credential-shaped content the
        // flow copied out of a log line (CR-05). The scrub replaces values
        // inside JSON strings, so the document stays parseable.
        let pretty_text =
            scrub_all(&serde_json::to_string_pretty(&report_json).map_err(|e| e.to_string())?);
        let pretty = pretty_text.into_bytes();
        // The scrub step itself is the claim; if it broke the document we
        // must not ship it.
        if serde_json::from_slice::<serde_json::Value>(&pretty).is_err() {
            return Err("报告脱敏后格式异常，已中止导出".into());
        }

        // Both files land beside the chosen destination path.
        let dest = PathBuf::from(destination);
        let stem = dest
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "report".into());
        let dir = dest
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let json_path = dir.join(format!("{stem}.json"));
        let md_path = dir.join(format!("{stem}.md"));
        std::fs::write(&json_path, &pretty).map_err(|e| format!("无法写入 JSON 报告: {e}"))?;
        // The Markdown carries the same facts — it goes through the same
        // scrub (CR-05).
        std::fs::write(&md_path, scrub_all(&markdown_report(&record, &log_tail)))
            .map_err(|e| format!("无法写入 Markdown 报告: {e}"))?;

        let mut hasher = Sha256::new();
        hasher.update(&pretty);
        Ok(ReportResult {
            json_path: json_path.to_string_lossy().into_owned(),
            markdown_path: md_path.to_string_lossy().into_owned(),
            sha256: hex::encode(hasher.finalize()),
        })
    }
}

fn read_launch_log_tail(root: &Path, record: &OperationRecord) -> Option<String> {
    let target = record.target_id.as_str();
    if target.is_empty() {
        return None;
    }
    let logs_dir = crate::instances::instance_dir(root, target)
        .ok()?
        .join("logs");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&logs_dir).ok()?.flatten() {
        let path = entry.path();
        let modified = entry.metadata().ok()?.modified().ok()?;
        if newest.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            newest = Some((modified, path));
        }
    }
    let (_, path) = newest?;
    let raw = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines.len().saturating_sub(40);
    Some(lines[start..].join("\n"))
}

fn markdown_report(record: &OperationRecord, log_tail: &str) -> String {
    // Verified claims follow the record's actual state — a failed or
    // interrupted operation must not export as "已完成" (CR-05).
    let (verified, verdict) = match record.status.as_str() {
        "committed" => (
            "- 资源复制/安装已按记录完成
- 计划哈希绑定通过（记录含 packSha256 时）"
                .to_string(),
            "已完成",
        ),
        "failed" => ("- 操作失败，未确认任何完成项".to_string(), "失败"),
        "cancelled" => ("- 操作被取消，未确认任何完成项".to_string(), "已取消"),
        "interrupted" => (
            "- 应用在操作进行中退出，结果未知；请核对目标实例实际状态".to_string(),
            "中断",
        ),
        other => (format!("- 状态 {other}：完成项未知"), other),
    };
    let detail_rows: String = record
        .detail
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "dshVersion" | "nodeVersion"))
        .map(|(k, v)| format!("- {k}: {v}"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    format!(
        "# PHL 操作报告 · {}

- 操作类型：{}
- 状态：{verdict}
- 源实例：{}
- 目标实例：{}
- 开始：{}  结束：{}
- PHL 版本：{}
- DSH 版本：{}
- Node 版本：{}
{detail_rows}
## 错误

{}

## 已验证项

{verified}

## 未验证项

- 插件加载检查
- 实际模型任务

## 日志尾部（已脱敏，最近 40 行）

```text
{}
```
",
        record.operation_id,
        record.kind,
        record.source_id,
        record.target_id,
        record.started_at,
        record.finished_at.as_deref().unwrap_or("—"),
        env!("CARGO_PKG_VERSION"),
        record
            .detail
            .get("dshVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("—"),
        record
            .detail
            .get("nodeVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("—"),
        record.error.as_deref().unwrap_or("（无）"),
        log_tail,
    )
}

/// Trial-create span: the flow-specific wrapper so each flow keeps its own
/// journal boilerplate out of the operation module's callers.
pub(crate) fn begin_trial(
    root: &Path,
    source_id: &str,
    target_id: &str,
    label: &str,
    detail: serde_json::Map<String, serde_json::Value>,
) -> Result<OperationSpan, String> {
    begin(
        root,
        OperationRecord {
            operation_id: format!(
                "trial-{}-{}",
                crate::versions::now_millis(),
                std::process::id()
            ),
            kind: "trial-create".into(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            label: label.into(),
            status: "running".into(),
            detail,
            error: None,
            started_at: String::new(),
            finished_at: None,
            retry_of: None,
        },
    )
}

/// Registry of operations the R1 flows call at start/end.
pub(crate) struct OperationSpan {
    pub root: PathBuf,
    pub operation_id: String,
}

pub(crate) fn begin(root: &Path, mut record: OperationRecord) -> Result<OperationSpan, String> {
    record.status = "running".into();
    record.started_at = crate::versions::now_iso();
    append_record(root, &record)?;
    Ok(OperationSpan {
        root: root.to_path_buf(),
        operation_id: record.operation_id,
    })
}

impl OperationSpan {
    pub(crate) fn finish(
        self,
        status: &str,
        error: Option<String>,
        detail_updates: serde_json::Map<String, serde_json::Value>,
    ) {
        let existing = load_records(&self.root)
            .into_iter()
            .find(|r| r.operation_id == self.operation_id)
            .unwrap_or(OperationRecord {
                operation_id: self.operation_id.clone(),
                kind: String::new(),
                source_id: String::new(),
                target_id: String::new(),
                label: String::new(),
                status: status.into(),
                detail: Default::default(),
                error: None,
                started_at: crate::versions::now_iso(),
                finished_at: None,
                retry_of: None,
            });
        // Merge: the finishing event carries its own fields but must not
        // erase the plan facts recorded at begin (dshVersion, scope…).
        let mut detail = existing.detail.clone();
        for (k, v) in detail_updates {
            detail.insert(k, v);
        }
        let record = OperationRecord {
            status: status.into(),
            finished_at: Some(crate::versions::now_iso()),
            error,
            detail,
            ..existing
        };
        let _ = append_record(&self.root, &record);
    }
}

#[cfg(test)]
mod tests;
