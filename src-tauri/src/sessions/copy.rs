//! The per-session copy: locate a source log, re-identify its header, and
//! publish a fresh fork into a target instance's DSH_HOME.
//!
//! This is where lineage stops being a comment and becomes bytes. The event
//! stream is carried verbatim (the codec guarantees it); only the header
//! changes, to a brand-new `session-<uuid>` id that records `parentSession` =
//! the source id and `seedLength` = the source's event count — the shape DSH
//! itself uses for a fork (Spike §7). The new session is filed under the same
//! project directory the source used, because `projectKey` is intentionally
//! lossy and the source dir is the only faithful spelling of its cwd.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{codec, CopyOutcome, SessionEndpoint, ZSTD_ARTIFACT};
use crate::errors;

/// Streaming progress for a fan-out copy: which target/session is landing now.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProgress {
    /// 0-based index of the (session × target) unit now completing.
    pub done: usize,
    /// Total units in this operation.
    pub total: usize,
    pub target_id: String,
    pub new_session_dir: String,
}

/// A source session resolved to its on-disk artifact and project group.
#[derive(Debug)]
pub(crate) struct SourceSession {
    /// The `<home>/sessions/<project>` directory the session lives under.
    pub project_dir: PathBuf,
    /// The encoded session directory name (equal to its id by DSH convention).
    pub dir: String,
    /// Absolute path to the `session.jsonl[.zstd]` artifact.
    pub artifact: PathBuf,
    pub encoding: codec::LogEncoding,
}

/// Find a session directory under `<home>/sessions/*/<dir>`. `dir` is the
/// encoded name; the enclosing project group is discovered by the search (the
/// caller cannot know the lossy `projectKey` without the header's cwd).
pub(crate) fn locate_source(home: &Path, dir: &str) -> Result<SourceSession, String> {
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Err(errors::coded(errors::ErrCode::NotFound, "实例暂无会话目录"));
    };
    for project in projects.flatten() {
        let cand = project.path().join(dir);
        if !cand.is_dir() {
            continue;
        }
        // Prefer the zstd artifact a populated home actually uses.
        let zstd = cand.join(ZSTD_ARTIFACT);
        if zstd.is_file() {
            return Ok(SourceSession {
                project_dir: project.path(),
                dir: dir.to_string(),
                artifact: zstd,
                encoding: codec::LogEncoding::Zstd,
            });
        }
        let plain = cand.join("session.jsonl");
        if plain.is_file() {
            return Ok(SourceSession {
                project_dir: project.path(),
                dir: dir.to_string(),
                artifact: plain,
                encoding: codec::LogEncoding::Plain,
            });
        }
    }
    Err(errors::coded(errors::ErrCode::NotFound, "未找到源会话"))
}

/// A DSH session directory name is `session-<uuid>`; the directory name and the
/// header `id` are kept equal (that is the layout DSH derives paths from).
///
/// UUID-v4-shaped without a `uuid` dependency: that crate's MSRV (1.85) is
/// above this build's pinned 1.77.2. `RandomState`' hasher is seeded from the
/// OS RNG per instance, and combined with the nanosecond clock it yields 16
/// bytes we format with the version-4 / variant-1 nibbles set — collision-safe
/// for local instance ids, which is the only guarantee the copy needs.
fn new_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    let clock = nanos();
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(clock ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let v = hasher.finish();
        chunk.copy_from_slice(&v.to_le_bytes());
    }
    // UUID v4: version nibble 4, variant nibble 10xx.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let h = hex::encode(bytes);
    format!(
        "session-{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Copy one located source session into one target endpoint. Blocking IO on the
/// filesystem; runs under the async command's `spawn_blocking`.
fn copy_one_sync(src: &SourceSession, target: &SessionEndpoint) -> Result<CopyOutcome, String> {
    let buf = std::fs::read(&src.artifact)
        .map_err(|e| errors::coded(errors::ErrCode::NotFound, format!("读取源会话失败: {e}")))?;
    let log = codec::decode(&buf).map_err(|e| e.to_command_error())?;

    let new_id = new_session_id();
    let rewritten = codec::rewrite_header_line(
        &log.header,
        &codec::Reidentification {
            new_id: &new_id,
            parent: &log.header.id,
            inherited: log.event_count,
        },
    )
    .map_err(|e| e.to_command_error())?;
    let bytes = log
        .build_copy(&rewritten)
        .map_err(|e| e.to_command_error())?;

    // Land beside the source's project group inside the target home, in a new
    // directory named for the new id. Staged to a `.part` and renamed so a torn
    // write never leaves a half session; a pre-existing target of the same name
    // (a re-copy) is replaced atomically.
    let dest_dir = target.home.join("sessions").join(
        src.project_dir
            .file_name()
            .ok_or_else(|| "源项目目录名异常".to_string())?,
    );
    let session_dir = dest_dir.join(&new_id);
    std::fs::create_dir_all(&session_dir).map_err(|e| {
        errors::coded(
            errors::ErrCode::Permission,
            format!("创建目标会话目录失败: {e}"),
        )
    })?;
    let filename = match src.encoding {
        codec::LogEncoding::Zstd => ZSTD_ARTIFACT,
        codec::LogEncoding::Plain => "session.jsonl",
    };
    let final_path = session_dir.join(filename);
    let staged = session_dir.join(format!(".{filename}.part"));
    std::fs::write(&staged, &bytes).map_err(|e| {
        errors::coded(
            errors::ErrCode::Permission,
            format!("写入会话副本失败: {e}"),
        )
    })?;

    // Verify before publishing: the staged copy must re-decode to the new id,
    // the source lineage, and the same event count. If it does not, delete the
    // stage and fail — never rename a suspect artifact into the target.
    let staged_bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let check = codec::decode(&staged_bytes).map_err(|e| {
        let _ = std::fs::remove_dir_all(&session_dir);
        e.to_command_error()
    });
    match check {
        Ok(re) => {
            if re.header.id != new_id
                || re.header.parent().map(str::to_string).as_deref() != Some(log.header.id.as_str())
                || re.event_count != log.event_count
            {
                let _ = std::fs::remove_dir_all(&session_dir);
                return Err(errors::coded(
                    errors::ErrCode::State,
                    "复制校验失败：新会话身份或事件数与预期不符，已回滚",
                ));
            }
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&session_dir);
            return Err(e);
        }
    }
    std::fs::rename(&staged, &final_path).map_err(|e| {
        let _ = std::fs::remove_dir_all(&session_dir);
        errors::coded(
            errors::ErrCode::Permission,
            format!("发布会话副本失败: {e}"),
        )
    })?;

    Ok(CopyOutcome {
        target_id: target.instance_id.clone(),
        target_name: target.manifest.name.clone(),
        new_session_dir: new_id.clone(),
        new_id,
    })
}

/// Public async entry: run one copy off the async thread.
///
/// Progress is deliberately NOT reported here. The only driver of a real
/// (session × target) matrix is `copy_sessions_inner_with`, which owns the
/// done/total counters — the old per-copy event carried a constant
/// `done: 0, total: 0`, so a select-all copy looked stuck at zero no matter
/// how many sessions had already landed.
pub(crate) async fn copy_one(
    src: &SourceSession,
    target: &SessionEndpoint,
) -> Result<CopyOutcome, String> {
    let src_owned = SourceSession {
        project_dir: src.project_dir.clone(),
        dir: src.dir.clone(),
        artifact: src.artifact.clone(),
        encoding: src.encoding,
    };
    let target_owned = SessionEndpoint {
        instance_id: target.instance_id.clone(),
        dir: target.dir.clone(),
        home: target.home.clone(),
        manifest: target.manifest.clone(),
    };
    tokio::task::spawn_blocking(move || copy_one_sync(&src_owned, &target_owned))
        .await
        .map_err(|e| format!("复制任务异常退出: {e}"))?
}

#[cfg(test)]
mod tests;
