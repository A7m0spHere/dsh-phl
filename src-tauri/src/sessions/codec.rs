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
//! - The physical header is the version-0 record `{type:"session", version, id,
//!   createdAt, cwd?, parentSession?, seedLength?, origin?, delegationDepth,
//!   agentPreset?}`. `SESSION_FORMAT_VERSION` is 0 and the backend refuses any
//!   other version with no migration — so a copy must fail loudly rather than
//!   carry an unreadable file into a target home.
//!
//! A copy decodes the whole artifact to plaintext, replaces ONLY the header
//! line (new `id`, `parentSession` → source id, `seedLength` → the inherited
//! event count), and re-encodes. Reading is layout-blind on the DSH side (its
//! `scanLog` folds every frame's plaintext into the same event stream), so the
//! copy may land as a single frame — the events themselves are re-emitted
//! byte-for-byte, only the header record changes.

use std::io::Read;

use serde_json::{Map, Value};

/// The Session header's on-disk version this build understands (DSH's
/// `SESSION_FORMAT_VERSION`; any other value is a compatibility refusal).
pub(crate) const SESSION_FORMAT_VERSION: u64 = 0;

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
    /// A frame's bytes are not decodable Zstandard (corrupt / wrong codec).
    FrameDecode(String),
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
                "会话格式版本 {found} 超出当前支持的 {SESSION_FORMAT_VERSION}，为避免损坏已拒绝复制"
            ),
            CodecError::FrameDecode(e) => format!("zstd 帧解码失败: {e}"),
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
}

impl DecodedLog {
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

/// Parse a session log artifact. Fails loudly on a wrong/missing header or an
/// unsupported format version — never invents a reading of newer bytes.
pub(crate) fn decode(buf: &[u8]) -> Result<DecodedLog, CodecError> {
    if buf.is_empty() {
        return Err(CodecError::MissingHeader);
    }
    match detect_encoding(buf) {
        LogEncoding::Plain => decode_plaintext(buf),
        LogEncoding::Zstd => decode_zstd(buf),
    }
}

fn parse_header(first_line: &str) -> Result<SessionHeaderRecord, CodecError> {
    if first_line.trim().is_empty() {
        return Err(CodecError::MissingHeader);
    }
    let value: Value =
        serde_json::from_str(first_line).map_err(|e| CodecError::MalformedHeader(e.to_string()))?;
    if value.get("type").and_then(Value::as_str) != Some("session") {
        return Err(CodecError::NotASessionLog);
    }
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| CodecError::MalformedHeader("header 缺少 version".into()))?;
    if version != SESSION_FORMAT_VERSION {
        return Err(CodecError::UnsupportedVersion { found: version });
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CodecError::MalformedHeader("header 缺少 id".into()))?
        .to_string();
    Ok(SessionHeaderRecord { value, id })
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

fn decode_plaintext(buf: &[u8]) -> Result<DecodedLog, CodecError> {
    let text = std::str::from_utf8(buf)
        .map_err(|e| CodecError::UnsupportedEncoding(format!("明文日志非 UTF-8: {e}")))?;
    let (first, events) = split_committed(text);
    let header = parse_header(first.ok_or(CodecError::MissingHeader)?)?;
    let event_count = events.len();
    Ok(DecodedLog {
        encoding: LogEncoding::Plain,
        header,
        event_count,
        events: encode_events(&events),
    })
}

fn decode_zstd(buf: &[u8]) -> Result<DecodedLog, CodecError> {
    let ranges = split_frames(buf);
    if ranges.is_empty() {
        return Err(CodecError::FrameDecode("未发现 zstd 帧".into()));
    }
    // Concatenate every frame's plaintext, then treat the first line as the
    // header. This keeps a copy's events byte-identical to the source's
    // decoded stream regardless of how DSH grouped them into frames.
    let mut all = Vec::new();
    for (s, e) in &ranges {
        all.extend_from_slice(&decode_frame(&buf[*s..*e])?);
    }
    let text = std::str::from_utf8(&all)
        .map_err(|e| CodecError::UnsupportedEncoding(format!("zstd 明文非 UTF-8: {e}")))?;
    let (first, events) = split_committed(text);
    let header = parse_header(first.ok_or(CodecError::MissingHeader)?)?;
    let event_count = events.len();
    Ok(DecodedLog {
        encoding: LogEncoding::Zstd,
        header,
        event_count,
        events: encode_events(&events),
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
pub(crate) fn rewrite_header_line(
    header: &SessionHeaderRecord,
    reid: &Reidentification<'_>,
) -> Result<String, CodecError> {
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
    /// verbatim, re-framed to match the source's physical encoding. A zstd copy
    /// lands as a single checksummed frame (reading is layout-blind); a plain
    /// copy stays plain.
    pub fn build_copy(&self, new_header_line: &str) -> Result<Vec<u8>, CodecError> {
        let mut plaintext = String::new();
        plaintext.push_str(new_header_line);
        plaintext.push('\n');
        let mut bytes = plaintext.into_bytes();
        bytes.extend_from_slice(&self.events);
        match self.encoding {
            LogEncoding::Plain => Ok(bytes),
            LogEncoding::Zstd => zstd::encode_all(&bytes[..], 3)
                .map_err(|e| CodecError::FrameDecode(format!("编码会话帧失败: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests;
