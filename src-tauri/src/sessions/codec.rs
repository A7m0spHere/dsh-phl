//! The on-disk session-log codec: framing, the physical header line, and the
//! re-identification a copy performs.
//!
//! Everything here is pure bytes-in/bytes-out so it is unit-testable without a
//! filesystem, and it encodes exactly what the Spike
//! (`docs/research/dsh-session-integration.md` §4-§7) verified against two
//! installed DSH versions and real homes:
//!
//! - A session log is a *concatenation of independent Zstandard frames* (the
//!   header is frame 0, every later durable batch is its own frame), or plain
//!   JSONL when the backend was configured `compression: none`. Frame starts
//!   are located by the Zstandard magic (`28 B5 2F FD`).
//! - A session's **format generation** is named by its artifact file
//!   (`session.jsonl` = v0, `session.vN.jsonl` = vN, plus `.zstd`), and DSH
//!   agrees the filename with the header's own `version` — a mismatch is
//!   refused outright. DSH's installed catalog carries codecs v0..v3 with the
//!   adjacent migration chain, so those are readable here too
//!   ([`MAX_READABLE_GENERATION`]); anything newer is a compatibility refusal,
//!   never a guess.
//! - The header's *field set* differs per generation, and DSH enforces it as an
//!   exact whitelist ([`header_contract`]). v0/v1 carry `seedLength`; v2/v3
//!   require `isSeeded` and have no `seedLength` at all. This is the one part
//!   of the format a copy cannot ignore: the fork marker a copy writes
//!   (`seedLength`) is legal only up to [`HEADER_FORK_MAX_GENERATION`].
//!
//! A copy decodes the whole artifact to plaintext, replaces ONLY the header
//! line (new `id`, `parentSession` → source id, `seedLength` → the log's
//! **logical event count**, see [`DecodedLog::inherited_event_count`] — *not*
//! its row count, which differs whenever a packed Assistant row is present),
//! and re-encodes. Reading is layout-blind on the DSH side for *events* (its
//! `scanLog` folds every frame's plaintext into the same event stream), but
//! **the first frame is not free**: DSH asserts that it holds
//! exactly the header line, and `dsh-workspace` refuses the whole boot when
//! that assertion fails (`corrupt Zstandard session log: first frame is not
//! exactly one header line`). So a copy always re-frames the header into its
//! own first frame, and carries the source's later frames byte-for-byte
//! whenever the source already had that shape — events stay byte-identical,
//! only the header record changes.

use std::io::Read;

use serde_json::{Map, Value};

/// The newest Session format generation this build reads. DSH's installed
/// catalog (`dsh-session-format-catalog`) wires codecs v0..v3 with the adjacent
/// migration chain and `currentVersion: 3`, so every generation up to 3 is
/// decodable; a newer one is a compatibility refusal, never a guess.
pub(crate) const MAX_READABLE_GENERATION: u64 = 3;

/// The generations whose fork marker lives in the *header* line — the only ones
/// a copy can re-identify without writing an event body.
///
/// v0/v1 share the physical contract of
/// `dsh-session-format-v0-to-v1::decodePhysicalHeader`, which reads
/// `seedLength` and derives `isSeeded` from its presence. From v2 on the header
/// has no such field: `isSeeded` is required, the inherited prefix is an
/// in-body `session/end-seed` event (`data.inherited === true`), and the
/// header's key set is an exact whitelist that rejects `seedLength` — so
/// stamping a fork into a v2/v3 header does not merely mislabel it, it makes
/// DSH read the header as corrupt and refuse the whole home.
pub(crate) const HEADER_FORK_MAX_GENERATION: u64 = 1;

/// The zstd suffix DSH appends for a `compression: zstd` backend.
const ZSTD_SUFFIX: &str = ".zstd";

/// The largest generation DSH's parser accepts (`Number.isSafeInteger`): a
/// longer digit string is not a canonical name, it only looks like one.
const MAX_SAFE_GENERATION: u64 = (1 << 53) - 1;

/// The v0/v1 row types that carry **several** logical events in one stored row
/// (`dsh-session-format-v0-to-v1::PACKED_TAGS`). A packed row's `seq0` is its
/// first event's seq and its payload array length is how many events it holds —
/// which is the difference between a log's row count and its event count.
const PACKED_ROWS: [&str; 3] = ["text-chunks", "reasoning-chunks", "tool-call-chunks"];

/// `(first event seq, events carried)` for one stored row of a v0/v1 log.
/// Mirrors `dsh-session-format-v0-to-v1::{decodeEvent, decodePackedRun}`.
fn row_shape(row: &Value, index: usize) -> Result<(usize, usize), CodecError> {
    let shape = |detail: String| CodecError::RowShape(format!("第 {index} 行{detail}"));
    let kind = row
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| shape("缺少 type".into()))?;
    let seq_of = |key: &str| -> Result<usize, CodecError> {
        let raw = row
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| shape(format!("缺少 {key}")))?;
        usize::try_from(raw).map_err(|_| shape(format!("{key} 超出整数范围")))
    };
    if !PACKED_ROWS.contains(&kind) {
        return Ok((seq_of("seq")?, 1));
    }
    let seq = seq_of("seq0")?;
    let payload = row
        .get("data")
        .and_then(|data| {
            if kind == "tool-call-chunks" {
                data.get("args")
            } else {
                data.get("texts")
            }
        })
        .and_then(Value::as_array)
        .filter(|payload| !payload.is_empty())
        .ok_or_else(|| shape(format!("{kind} 的 payload 不是非空数组")))?;
    Ok((seq, payload.len()))
}

/// DSH's canonical artifact basename for one generation and physical encoding
/// (`sessionFormatLogFilename` + `compressionSuffix`): generation 0 keeps
/// `session.jsonl`, every later generation carries `.vN` before `.jsonl`.
pub(crate) fn generation_filename(generation: u64, encoding: LogEncoding) -> String {
    let mut name = if generation == 0 {
        "session.jsonl".to_string()
    } else {
        format!("session.v{generation}.jsonl")
    };
    if encoding == LogEncoding::Zstd {
        name.push_str(ZSTD_SUFFIX);
    }
    name
}

/// Parse a canonical artifact basename back to its generation for one physical
/// encoding. Temporary (leading-dot), uppercase, leading-zero and `.v0` names
/// are not canonical, and neither is a name carrying the other encoding's
/// suffix — DSH ignores exactly those names when it scans a session directory
/// (`parseSessionFormatLogFilename` + `parseGenerationLogFilename`), so a
/// resolver that accepted them would read files the owner never sees.
pub(crate) fn generation_of_filename(name: &str, encoding: LogEncoding) -> Option<u64> {
    let base = match encoding {
        LogEncoding::Zstd => name.strip_suffix(ZSTD_SUFFIX)?,
        LogEncoding::Plain => name,
    };
    if base == "session.jsonl" {
        return Some(0);
    }
    let digits = base.strip_prefix("session.v")?.strip_suffix(".jsonl")?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<u64>()
        .ok()
        .filter(|generation| *generation <= MAX_SAFE_GENERATION)
}

/// Whether `path` is absolute in either shape a desktop DSH can be running on:
/// a Windows drive (`C:\` / `C:/`), a Windows root or UNC (`\`, `/`, `\\srv`),
/// or a POSIX leading `/`. A deliberate *superset* of DSH's platform-dependent
/// rule: accepting more than it does can only list a session it skips, never
/// hide one it shows — and hiding is the failure this module exists to end.
fn is_absolute_path(path: &str) -> bool {
    let mut chars = path.chars();
    match chars.next() {
        Some('/') | Some('\\') => true,
        Some(letter) if letter.is_ascii_alphabetic() => {
            chars.next() == Some(':') && matches!(chars.next(), Some('/') | Some('\\'))
        }
        _ => false,
    }
}

/// The physical header fields DSH enforces per generation, mirrored so a copy
/// can never publish a header the owner would read as corrupt (its
/// `isHeaderLine` guard rejects any key outside the whitelist, and a rejected
/// header fails the whole home's scan, not just that session).
///
/// v0/v1: `dsh-session-format-v0-to-v1::PHYSICAL_HEADER_REQUIRED/OPTIONAL`.
/// v2/v3: `dsh-session-persistence-jsonl::HEADER_REQUIRED_KEYS/HEADER_OPTIONAL_KEYS`.
fn header_contract(generation: u64) -> (&'static [&'static str], &'static [&'static str]) {
    const V0_REQUIRED: &[&str] = &["type", "version", "id", "createdAt", "delegationDepth"];
    const V0_OPTIONAL: &[&str] = &[
        "cwd",
        "parentSession",
        "seedLength",
        "origin",
        "agentPreset",
    ];
    const V2_REQUIRED: &[&str] = &[
        "type",
        "version",
        "id",
        "createdAt",
        "isSeeded",
        "delegationDepth",
    ];
    const V2_OPTIONAL: &[&str] = &["cwd", "parentSession", "origin", "agentPreset"];
    if generation <= HEADER_FORK_MAX_GENERATION {
        (V0_REQUIRED, V0_OPTIONAL)
    } else {
        (V2_REQUIRED, V2_OPTIONAL)
    }
}

/// Zstandard frame magic, little-endian `0xFD2FB528`.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// What a log artifact looks like: plaintext JSONL or concatenated zstd frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogEncoding {
    Zstd,
    Plain,
}

/// Why a session log could not be read or copied. Rendered as a stable,
/// explainable error by the caller (mirrors the manifest's
/// `UnsupportedSchema` "too new → refuse, never guess" discipline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodecError {
    /// Empty or truncated below a complete header line.
    MissingHeader,
    /// First record is not a `type:"session"` line.
    NotASessionLog,
    /// The header declares a format version this build cannot read.
    UnsupportedVersion { found: u64 },
    /// The artifact's filename generation and the header's `version` disagree.
    /// DSH refuses the pair outright, so this is a data fault, not a preference.
    GenerationMismatch { filename: u64, header: u64 },
    /// A session directory holds an artifact of the home's *opposite* physical
    /// encoding. DSH raises `encodingMismatch` for it and that error leaves the
    /// first session operation — i.e. it stops the instance from booting.
    EncodingMismatch(String),
    /// A frame's bytes are not decodable Zstandard (corrupt / wrong codec).
    FrameDecode(String),
    /// A stored event row is unreadable or violates DSH's row invariant — the
    /// shape a fork's inherited-prefix length is counted from.
    RowShape(String),
    /// The header line is not valid JSON / not an object.
    MalformedHeader(String),
    /// The artifact is not decodable text at all.
    UnsupportedEncoding(String),
}

impl CodecError {
    /// A `coded` error string for the command layer (compatibility failures use
    /// `ErrCode::State`; decode problems are data, not a busy resource).
    pub(crate) fn to_command_error(&self) -> String {
        let detail = match self {
            CodecError::MissingHeader => "会话日志为空或被截断，缺少 header 行".to_string(),
            CodecError::NotASessionLog => {
                "首条记录不是 session header，可能不是 DSH 会话文件".to_string()
            }
            CodecError::UnsupportedVersion { found } => format!(
                "会话格式版本 {found} 超出当前支持的 v{MAX_READABLE_GENERATION}，为避免损坏已拒绝复制"
            ),
            CodecError::GenerationMismatch { filename, header } => format!(
                "会话文件名标识 v{filename}，但 header 自称 v{header}：DSH 会直接拒绝该会话"
            ),
            CodecError::EncodingMismatch(name) => format!(
                "会话目录里存在与 home 物理编码不符的产物 {name}：DSH 会因此拒绝启动整个实例"
            ),
            CodecError::FrameDecode(e) => format!("zstd 帧解码失败: {e}"),
            CodecError::RowShape(e) => format!("会话事件行无法用于计算继承长度: {e}"),
            CodecError::MalformedHeader(e) => format!("header 解析失败: {e}"),
            CodecError::UnsupportedEncoding(e) => format!("会话日志编码不支持: {e}"),
        };
        crate::errors::coded(crate::errors::ErrCode::State, detail)
    }
}

/// The decoded header record of a session log (frame 0 / first line).
#[derive(Debug, Clone)]
pub(crate) struct SessionHeaderRecord {
    pub value: Value,
    pub id: String,
    /// The artifact's format generation — equal to the header's `version` by
    /// construction, because a mismatch is refused (see [`parse_header`]).
    pub generation: u64,
}

impl SessionHeaderRecord {
    /// The absolute working directory the session was created in, if any.
    pub fn cwd(&self) -> Option<&str> {
        self.value.get("cwd").and_then(Value::as_str)
    }
    /// The session this one was forked from, if any.
    pub fn parent(&self) -> Option<&str> {
        self.value.get("parentSession").and_then(Value::as_str)
    }
    /// Whether this generation carries a copy's fork marker in the header
    /// (`seedLength`) rather than in the event body.
    pub(crate) fn supports_header_fork(&self) -> bool {
        self.generation <= HEADER_FORK_MAX_GENERATION
    }
}

/// A fully decoded session log: the header, the count of event records, and
/// the raw plaintext of those events (everything after the header line,
/// newline-terminated as read), ready for a copy to re-emit verbatim.
#[derive(Debug)]
pub(crate) struct DecodedLog {
    pub encoding: LogEncoding,
    pub header: SessionHeaderRecord,
    /// Event records following the header — the `seedLength` a copy stamps.
    pub event_count: usize,
    /// Plaintext of the event records, each line newline-terminated.
    events: Vec<u8>,
    /// The source's frames *after* the first, byte-for-byte, when the source
    /// already had the shape DSH requires (first frame = the header line alone)
    /// and nothing was dropped while splitting committed records. `None` means
    /// a copy must re-frame the events itself; `Some(empty)` means the source
    /// was a header-only log.
    tail_frames: Option<Vec<u8>>,
}

impl DecodedLog {
    /// The inherited-prefix length a fork of this log must record
    /// (`seedLength`) — the log's **logical event count**, which is not its row
    /// count once packed Assistant rows are involved.
    ///
    /// DSH's v0/v1 row scanner (`dsh-session-format-v0-to-v1::scanRows`) keeps a
    /// running event count and requires every row's `seq` to equal it
    /// (`released Session row N has seq gap`); a *packed* Assistant row starts
    /// at `seq0` and advances the count by its payload length, a normal row by
    /// one. So the count is a property of the log, not of its line breaks, and
    /// a copy that stamps the row count writes a cut that lands inside an
    /// Assistant attempt — which DSH refuses at migration time
    /// (`inherited Session cut N splits one Assistant attempt`).
    ///
    /// Counting only: this reads `type`, `seq`, `seq0` and the length of the
    /// payload array, never a payload's content. Refusing (on a row DSH itself
    /// would reject, or a shape this walk cannot account for) is the only
    /// alternative — guessing the cut is what produced unrunnable copies.
    pub(crate) fn inherited_event_count(&self) -> Result<usize, CodecError> {
        let text = std::str::from_utf8(&self.events)
            .map_err(|e| CodecError::RowShape(format!("事件明文非 UTF-8: {e}")))?;
        let mut count = 0usize;
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let row: Value = serde_json::from_str(line)
                .map_err(|e| CodecError::RowShape(format!("第 {index} 行不是 JSON: {e}")))?;
            let (seq, carries) = row_shape(&row, index)?;
            if seq != count {
                return Err(CodecError::RowShape(format!(
                    "第 {index} 行的 seq 是 {seq}，但此前已有 {count} 个事件（DSH 会判为 seq gap，拒绝该日志）"
                )));
            }
            count = count
                .checked_add(carries)
                .ok_or_else(|| CodecError::RowShape("事件计数溢出".into()))?;
        }
        Ok(count)
    }

    /// The event records as parsed JSON lines (best-effort; malformed trailing
    /// records are skipped, matching a committed-prefix read). Test-only
    /// assertion helper: production reads event_count and copies the bytes.
    #[cfg(test)]
    pub fn event_values(&self) -> Vec<Value> {
        String::from_utf8_lossy(&self.events)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// The committed event plaintext, byte-for-byte. Test-only: it is what a
    /// copy must reproduce exactly (a parse-level comparison would not notice a
    /// resurrected torn record).
    #[cfg(test)]
    pub fn event_bytes(&self) -> &[u8] {
        &self.events
    }
}

/// Detect a log file's physical encoding by its leading bytes.
pub(crate) fn detect_encoding(buf: &[u8]) -> LogEncoding {
    if buf.len() >= 4 && buf[0..4] == ZSTD_MAGIC {
        LogEncoding::Zstd
    } else {
        LogEncoding::Plain
    }
}

fn decode_frame(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(bytes)
        .map_err(|e| CodecError::FrameDecode(e.to_string()))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| CodecError::FrameDecode(e.to_string()))?;
    Ok(out)
}

/// Split a zstd artifact into frame byte-ranges by scanning for the magic. A
/// `Magic_Restart` byte sequence inside a compressed payload is theoretically
/// possible; a wrong boundary yields a non-decodable frame, which surfaces as a
/// `FrameDecode` error rather than silent corruption — so it can never pass a
/// copy's header validation unnoticed.
fn split_frames(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut i = 0usize;
    while i + ZSTD_MAGIC.len() <= buf.len() {
        match buf[i..]
            .windows(ZSTD_MAGIC.len())
            .position(|w| w == ZSTD_MAGIC)
        {
            Some(off) => {
                let start = i + off;
                let next = buf[start + ZSTD_MAGIC.len()..]
                    .windows(ZSTD_MAGIC.len())
                    .position(|w| w == ZSTD_MAGIC)
                    .map(|p| start + ZSTD_MAGIC.len() + p)
                    .unwrap_or(buf.len());
                ranges.push((start, next));
                i = next;
            }
            None => break,
        }
    }
    ranges
}

/// DSH's first-frame rule, mirrored as a publish gate: the independently
/// decodable first frame must hold exactly one newline-terminated line.
///
/// `dsh-session-persistence-jsonl` asserts this while listing a home's session
/// artifacts, and `dsh-workspace` fails its own load on that error — so a file
/// violating it does not merely fail to open: **it stops the whole instance
/// from booting** (`corrupt Zstandard session log: first frame is not exactly
/// one header line`). A copy therefore validates it before publishing.
///
/// The rule is a *framing* rule: a plaintext log has no frames to violate (its
/// header is line 0 by construction), so only the zstd form is checked.
pub(crate) fn header_frame_is_alone(buf: &[u8]) -> Result<(), CodecError> {
    if detect_encoding(buf) == LogEncoding::Plain {
        return Ok(());
    }
    let ranges = split_frames(buf);
    let (start, end) = ranges
        .first()
        .ok_or_else(|| CodecError::FrameDecode("副本没有 zstd 帧".into()))?;
    let first = decode_frame(&buf[*start..*end])?;
    let lone_line = first.last() == Some(&b'\n')
        && first[..first.len().saturating_sub(1)]
            .iter()
            .all(|b| *b != b'\n');
    if !lone_line {
        return Err(CodecError::FrameDecode(
            "副本首个 zstd 帧不是单独的 header 行".into(),
        ));
    }
    Ok(())
}

/// Parse a session log artifact of a known generation. `generation` is the one
/// its canonical filename names (see [`generation_of_filename`]).
pub(crate) fn decode(buf: &[u8], generation: u64) -> Result<DecodedLog, CodecError> {
    if buf.is_empty() {
        return Err(CodecError::MissingHeader);
    }
    match detect_encoding(buf) {
        LogEncoding::Plain => decode_plaintext(buf, generation),
        LogEncoding::Zstd => decode_zstd(buf, generation),
    }
}

/// Parse a session log header against the generation its artifact filename
/// names. Fails loudly on a wrong/missing header, a generation the filename and
/// the header disagree about, an unsupported version, or a field outside that
/// generation's contract — never invents a reading of bytes DSH would refuse.
///
/// `generation` comes from the artifact basename ([`generation_of_filename`]),
/// which is what DSH itself pairs the header against: reading a header without
/// knowing its filename would accept a pair the owner rejects.
///
/// One thing is *not* mirrored: DSH's `cwd`-must-be-absolute rule is, in DSH
/// itself, a `malformed` verdict that makes it **skip** the session rather than
/// throw — so the check below uses the union of Windows and POSIX absoluteness
/// ([`is_absolute_path`]) and can only ever list a session DSH skips, never
/// hide one it lists.
fn parse_header(first_line: &str, generation: u64) -> Result<SessionHeaderRecord, CodecError> {
    if first_line.trim().is_empty() {
        return Err(CodecError::MissingHeader);
    }
    let value: Value =
        serde_json::from_str(first_line).map_err(|e| CodecError::MalformedHeader(e.to_string()))?;
    if value.get("type").and_then(Value::as_str) != Some("session") {
        return Err(CodecError::NotASessionLog);
    }
    let Some(obj) = value.as_object() else {
        return Err(CodecError::MalformedHeader("header 非对象".into()));
    };
    let version = obj
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| CodecError::MalformedHeader("header 缺少 version".into()))?;
    if version > MAX_READABLE_GENERATION {
        return Err(CodecError::UnsupportedVersion { found: version });
    }
    // DSH: "session generation filename identifies vN, but its header
    // identifies vM" — a pair that disagrees is refused, so a copy must never
    // produce one.
    if version != generation {
        return Err(CodecError::GenerationMismatch {
            filename: generation,
            header: version,
        });
    }
    let (required, optional) = header_contract(version);
    for key in required {
        if !obj.contains_key(*key) {
            return Err(CodecError::MalformedHeader(format!("header 缺少 {key}")));
        }
    }
    // DSH's shape guard is an exact whitelist: unknown keys do not decorate a
    // header, they disqualify it.
    for key in obj.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(CodecError::MalformedHeader(format!(
                "header 含 v{version} 契约之外的字段 {key}"
            )));
        }
    }
    for key in ["createdAt", "delegationDepth"] {
        let ok = obj
            .get(key)
            .and_then(Value::as_u64)
            .is_some_and(|v| i64::try_from(v).is_ok());
        if !ok {
            return Err(CodecError::MalformedHeader(format!(
                "header 的 {key} 必须是非负整数"
            )));
        }
    }
    if version > HEADER_FORK_MAX_GENERATION
        && obj.get("isSeeded").and_then(Value::as_bool).is_none()
    {
        return Err(CodecError::MalformedHeader(
            "header 的 isSeeded 必须是布尔值".into(),
        ));
    }
    for key in ["cwd", "parentSession", "agentPreset", "origin"] {
        if let Some(v) = obj.get(key) {
            if !v.is_string() {
                return Err(CodecError::MalformedHeader(format!(
                    "header 的 {key} 必须是字符串"
                )));
            }
        }
    }
    if let Some(cwd) = obj.get("cwd").and_then(Value::as_str) {
        if !is_absolute_path(cwd) {
            return Err(CodecError::MalformedHeader(
                "header 的 cwd 必须是绝对路径".into(),
            ));
        }
    }
    if let Some(origin) = obj.get("origin").and_then(Value::as_str) {
        if origin != "subagent" {
            return Err(CodecError::MalformedHeader(
                "header 的 origin 只能是 \"subagent\"".into(),
            ));
        }
    }
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CodecError::MalformedHeader("header 缺少 id".into()))?
        .to_string();
    Ok(SessionHeaderRecord {
        value,
        id,
        generation: version,
    })
}

/// Split decoded plaintext into `(header_line, committed_event_lines)`. The
/// last line is dropped when the buffer had no trailing newline — DSH treats a
/// torn final record (a crash mid-append) as uncommitted, and a copy must not
/// resurrect it. Empty lines are never events.
fn split_committed(text: &str) -> (Option<&str>, Vec<&str>) {
    let ends_with_newline = text.ends_with('\n');
    let mut it = text.split('\n').peekable();
    let header = it.next().filter(|l| !l.trim().is_empty());
    let mut events: Vec<&str> = Vec::new();
    while let Some(line) = it.peek() {
        let line = *line;
        it.next();
        // The final element of a `split('\n')` with no trailing newline is a
        // torn, uncommitted record — keep it only if we are not the last.
        let is_last = it.peek().is_none();
        if line.trim().is_empty() {
            continue;
        }
        if is_last && !ends_with_newline {
            break;
        }
        events.push(line);
    }
    (header, events)
}

/// Emit committed event lines as newline-terminated plaintext + their count.
fn encode_events(events: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in events {
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    out
}

fn decode_plaintext(buf: &[u8], generation: u64) -> Result<DecodedLog, CodecError> {
    let text = std::str::from_utf8(buf)
        .map_err(|e| CodecError::UnsupportedEncoding(format!("明文日志非 UTF-8: {e}")))?;
    let (first, events) = split_committed(text);
    let header = parse_header(first.ok_or(CodecError::MissingHeader)?, generation)?;
    let event_count = events.len();
    Ok(DecodedLog {
        encoding: LogEncoding::Plain,
        header,
        event_count,
        events: encode_events(&events),
        // Plaintext logs have no frames — the header is line 0 by construction.
        tail_frames: None,
    })
}

/// The source's frames after the first, reusable byte-for-byte — `Some` only
/// when the source is already in the shape DSH asserts (frame 0 = the header
/// line alone) *and* every later frame's plaintext is exactly the committed
/// event bytes. Any mismatch (a single-frame log from an older PHL copy, a
/// frame that also carries events, or a torn tail this decoder deliberately
/// drops) means the copy has to re-frame the events: carrying those bytes
/// would either violate the header-frame rule or resurrect an uncommitted
/// record.
///
/// `frame0_len` is frame 0's *decoded* length: with the plaintexts concatenated
/// in frame order, `frame0_len != header length` means frame 0 carries events
/// too — the shape that must never be carried forward (and the reason a
/// single-frame source cannot reuse anything, even though its bytes after the
/// header happen to equal the events).
fn reusable_tail_frames(
    buf: &[u8],
    ranges: &[(usize, usize)],
    frame0_len: usize,
    all: &[u8],
    header_line: &str,
    events: &[u8],
) -> Option<Vec<u8>> {
    let mut expected_header = header_line.as_bytes().to_vec();
    expected_header.push(b'\n');
    if ranges.len() < 2 || frame0_len != expected_header.len() {
        return None;
    }
    if !all.starts_with(&expected_header) {
        return None;
    }
    if &all[expected_header.len()..] != events {
        return None;
    }
    let mut out = Vec::new();
    for (s, e) in &ranges[1..] {
        out.extend_from_slice(&buf[*s..*e]);
    }
    Some(out)
}

fn decode_zstd(buf: &[u8], generation: u64) -> Result<DecodedLog, CodecError> {
    let ranges = split_frames(buf);
    if ranges.is_empty() {
        return Err(CodecError::FrameDecode("未发现 zstd 帧".into()));
    }
    // Concatenate every frame's plaintext, then treat the first line as the
    // header. This keeps a copy's events byte-identical to the source's
    // decoded stream regardless of how DSH grouped them into frames.
    let mut all = Vec::new();
    let mut frame0_len = 0usize;
    for (index, (s, e)) in ranges.iter().enumerate() {
        let plain = decode_frame(&buf[*s..*e])?;
        if index == 0 {
            frame0_len = plain.len();
        }
        all.extend_from_slice(&plain);
    }
    let text = std::str::from_utf8(&all)
        .map_err(|e| CodecError::UnsupportedEncoding(format!("zstd 明文非 UTF-8: {e}")))?;
    let (first, events) = split_committed(text);
    let header_line = first.ok_or(CodecError::MissingHeader)?;
    let header = parse_header(header_line, generation)?;
    let event_count = events.len();
    let events_bytes = encode_events(&events);
    let tail_frames =
        reusable_tail_frames(buf, &ranges, frame0_len, &all, header_line, &events_bytes);
    Ok(DecodedLog {
        encoding: LogEncoding::Zstd,
        header,
        event_count,
        events: events_bytes,
        tail_frames,
    })
}

/// The rewrite a copy applies to a session header.
pub(crate) struct Reidentification<'a> {
    pub new_id: &'a str,
    /// Lineage: the source session id this copy is forked from.
    pub parent: &'a str,
    /// Exact inherited prefix length (`seedLength`).
    pub inherited: usize,
}

/// Produce a header line re-identified for a copy. `cwd`, `agentPreset`,
/// timestamps and every other field survive untouched (spec §5.1: cwd
/// preservation + lineage). Key order is preserved by mutating the parsed
/// object in place, so only id/parentSession/seedLength differ from the source.
///
/// Refuses a generation whose header has no `seedLength` field
/// ([`HEADER_FORK_MAX_GENERATION`]): writing one there would put a key outside
/// the generation's whitelist, which DSH reads as a corrupt header and which
/// fails the target home's whole scan — the copy would break the instance it
/// was meant to enrich.
pub(crate) fn rewrite_header_line(
    header: &SessionHeaderRecord,
    reid: &Reidentification<'_>,
) -> Result<String, CodecError> {
    if !header.supports_header_fork() {
        return Err(CodecError::MalformedHeader(format!(
            "v{} 会话的继承前缀写在日志体内（session/end-seed 事件），header 里没有 seedLength 字段",
            header.generation
        )));
    }
    let mut obj: Map<String, Value> = match header.value.clone() {
        Value::Object(m) => m,
        _ => return Err(CodecError::MalformedHeader("header 非对象".into())),
    };
    obj.insert("id".into(), Value::String(reid.new_id.to_string()));
    obj.insert(
        "parentSession".into(),
        Value::String(reid.parent.to_string()),
    );
    obj.insert("seedLength".into(), Value::Number(reid.inherited.into()));
    serde_json::to_string(&Value::Object(obj))
        .map_err(|e| CodecError::MalformedHeader(e.to_string()))
}

impl DecodedLog {
    /// Assemble the copied artifact: a new header line + the source's events
    /// verbatim, re-framed to match the source's physical encoding. A plain
    /// copy stays plain; a zstd copy always puts the header in its own first
    /// frame (DSH asserts that) and then carries the source's later frames
    /// untouched, falling back to one frame of committed events when the
    /// source's framing could not be reused.
    pub fn build_copy(&self, new_header_line: &str) -> Result<Vec<u8>, CodecError> {
        match self.encoding {
            LogEncoding::Plain => {
                let mut out = Vec::with_capacity(new_header_line.len() + 1 + self.events.len());
                out.extend_from_slice(new_header_line.as_bytes());
                out.push(b'\n');
                out.extend_from_slice(&self.events);
                Ok(out)
            }
            LogEncoding::Zstd => {
                // Frame 0: the header line alone — DSH's `assertZstdHeaderFrame`
                // rejects any other shape and refuses the whole boot on it.
                let header_frame = format!("{new_header_line}\n");
                let mut out = zstd::encode_all(header_frame.as_bytes(), 3)
                    .map_err(|e| CodecError::FrameDecode(format!("编码会话帧失败: {e}")))?;
                match &self.tail_frames {
                    Some(tail) => out.extend_from_slice(tail),
                    None if self.events.is_empty() => {}
                    None => {
                        let events = zstd::encode_all(&self.events[..], 3)
                            .map_err(|e| CodecError::FrameDecode(format!("编码事件帧失败: {e}")))?;
                        out.extend_from_slice(&events);
                    }
                }
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests;
