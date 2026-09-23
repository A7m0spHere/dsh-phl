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

use super::{codec, CopyOutcome, SessionEndpoint};
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
    /// Absolute path to the session's authoritative artifact
    /// (`session[.vN].jsonl[.zstd]`).
    pub artifact: PathBuf,
    pub encoding: codec::LogEncoding,
    /// The artifact's format generation — the `.vN` in its filename, which DSH
    /// requires the header's `version` to equal.
    pub generation: u64,
}

/// The physical encoding a home's session backend already uses, inferred from
/// the artifacts its session directories hold. `None` = the home has no
/// sessions yet, so its backend is not observable from disk.
pub(crate) fn home_session_encoding(home: &Path) -> Option<codec::LogEncoding> {
    let root = home.join("sessions");
    for project in std::fs::read_dir(root).ok()?.flatten() {
        for session in std::fs::read_dir(project.path()).ok()?.flatten() {
            for entry in std::fs::read_dir(session.path()).ok()?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if codec::generation_of_filename(&name, codec::LogEncoding::Zstd).is_some() {
                    return Some(codec::LogEncoding::Zstd);
                }
                if codec::generation_of_filename(&name, codec::LogEncoding::Plain).is_some() {
                    return Some(codec::LogEncoding::Plain);
                }
            }
        }
    }
    None
}

/// Whether a copy would land an artifact the target's session backend refuses.
///
/// DSH validates that a home holds one physical encoding: any session
/// directory carrying the *opposite* generation's artifact fails
/// `checkRootEncoding` with `encodingMismatch`, and that is thrown out of the
/// first session operation — i.e. it stops the instance from booting, exactly
/// like the header-frame rule this module already gates on. A copy therefore
/// has to match the target's backend; an empty target home cannot be observed,
/// so DSH's own default (`DEFAULT_COMPRESSION = "zstd"`) is assumed and a
/// plaintext source is refused rather than guessed into a zstd home.
pub(crate) fn encoding_conflict(
    source: codec::LogEncoding,
    target_has: Option<codec::LogEncoding>,
) -> Option<&'static str> {
    let target = target_has.unwrap_or(codec::LogEncoding::Zstd);
    if source == target {
        return None;
    }
    Some(match source {
        codec::LogEncoding::Plain => {
            "源会话是明文（compression: none），而目标的会话后端是 zstd：跨编码复制会让目标实例无法启动，已拒绝"
        }
        codec::LogEncoding::Zstd => {
            "源会话是 zstd，而目标的会话后端是明文的：跨编码复制会让目标实例无法启动，已拒绝"
        }
    })
}

/// Find a session directory under `<home>/sessions/*/<dir>`. `dir` is the
/// encoded name; the enclosing project group is discovered by the search (the
/// caller cannot know the lossy `projectKey` without the header's cwd).
///
/// The artifact is resolved the way DSH resolves it — the highest canonical
/// generation of this home's physical encoding — so a copy migrates the log the
/// owner actually reads, not a stale pre-migration one.
pub(crate) fn locate_source(home: &Path, dir: &str) -> Result<SourceSession, String> {
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Err(errors::coded(errors::ErrCode::NotFound, "实例暂无会话目录"));
    };
    let encoding = home_session_encoding(home).unwrap_or(codec::LogEncoding::Zstd);
    for project in projects.flatten() {
        let cand = project.path().join(dir);
        if !cand.is_dir() {
            continue;
        }
        let (artifact, generation) = super::artifact_path(&cand, encoding)
            .map_err(|e| e.to_command_error())?
            .ok_or_else(|| errors::coded(errors::ErrCode::NotFound, "未找到源会话"))?;
        return Ok(SourceSession {
            project_dir: project.path(),
            dir: dir.to_string(),
            artifact,
            encoding,
            generation,
        });
    }
    Err(errors::coded(errors::ErrCode::NotFound, "未找到源会话"))
}

/// Whether this build can copy a session of that format generation.
///
/// A copy re-identifies a session by rewriting its **header line**: the fork
/// marker it writes is `seedLength` + `parentSession`. Only v0/v1 have a
/// `seedLength` field (see [`codec::HEADER_FORK_MAX_GENERATION`]); from v2 on
/// the inherited prefix is an in-body `session/end-seed` event, and the
/// header's key set is an exact whitelist, so stamping `seedLength` there would
/// not fork the session — it would hand the target a header DSH reads as
/// corrupt, which fails that home's whole scan and stops the instance from
/// booting. Refusing loudly is the only honest answer until PHL can write that
/// body event.
pub(crate) fn unsupported_copy_reason(generation: u64) -> Option<String> {
    if generation <= codec::HEADER_FORK_MAX_GENERATION {
        return None;
    }
    Some(errors::coded(
        errors::ErrCode::State,
        format!(
            "v{generation} 会话无法迁移：它的继承前缀写在日志体内（session/end-seed 事件），而复制只能改写 header 行 \
             —— 强行复制会让目标实例起不来。v0/v1 会话可以照常迁移；该会话仍可在源实例里正常打开。"
        ),
    ))
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
fn copy_one_sync(
    src: &SourceSession,
    target: &SessionEndpoint,
    target_encoding: codec::LogEncoding,
) -> Result<CopyOutcome, String> {
    if let Some(reason) = unsupported_copy_reason(src.generation) {
        return Err(reason);
    }
    // The publisher's own encoding check, so no caller can land the opposite
    // generation's artifact in a home: DSH raises `encodingMismatch` for that
    // and it stops the target instance from booting. The orchestrator checks
    // this too (with the target's name in the message); this is the backstop.
    if let Some(reason) = encoding_conflict(src.encoding, Some(target_encoding)) {
        return Err(errors::coded(errors::ErrCode::State, reason));
    }
    let buf = std::fs::read(&src.artifact)
        .map_err(|e| errors::coded(errors::ErrCode::NotFound, format!("读取源会话失败: {e}")))?;
    let log = codec::decode(&buf, src.generation).map_err(|e| e.to_command_error())?;
    // The artifact's content must match the encoding its name declares: DSH
    // reads a home's encoding off these names, and a mismatch is exactly the
    // `encodingMismatch` it refuses to boot on.
    if log.encoding != src.encoding {
        return Err(errors::coded(
            errors::ErrCode::State,
            "源会话的内容与其文件名的物理编码不一致，已拒绝复制",
        ));
    }

    // The fork's inherited-prefix length is the source's *logical event
    // count*, not its row count: a packed Assistant row carries many events,
    // and a cut that lands inside an Assistant attempt makes DSH refuse the
    // copy at migration time ("inherited Session cut N splits one Assistant
    // attempt"). Counting it is a walk over `seq`/`seq0` and payload lengths;
    // if that walk cannot account for the log (or finds the seq gap DSH
    // refuses), the copy is refused rather than stamped with a guessed cut.
    let inherited = log
        .inherited_event_count()
        .map_err(|e| e.to_command_error())?;
    let new_id = new_session_id();
    let rewritten = codec::rewrite_header_line(
        &log.header,
        &codec::Reidentification {
            new_id: &new_id,
            parent: &log.header.id,
            inherited,
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
    // The copy keeps the source's generation: DSH pairs the artifact filename
    // with the header's own `version` and refuses a pair that disagrees, so a
    // v3 source must land as `session.v3.jsonl.zstd`, not as v0.
    let filename = codec::generation_filename(src.generation, src.encoding);
    let final_path = session_dir.join(&filename);
    let staged = session_dir.join(format!(".{filename}.part"));
    std::fs::write(&staged, &bytes).map_err(|e| {
        errors::coded(
            errors::ErrCode::Permission,
            format!("写入会话副本失败: {e}"),
        )
    })?;

    // Verify before publishing: the staged copy must re-decode to the new id,
    // the source lineage, the same events, and the inherited-prefix length this
    // copy computed. If it does not, delete the stage and fail — never rename a
    // suspect artifact into the target.
    let staged_bytes = std::fs::read(&staged).map_err(|e| {
        let _ = std::fs::remove_dir_all(&session_dir);
        e.to_string()
    })?;
    let check = codec::decode(&staged_bytes, src.generation).map_err(|e| {
        let _ = std::fs::remove_dir_all(&session_dir);
        e.to_command_error()
    });
    match check {
        Ok(re) => {
            let staged_cut = re.inherited_event_count().ok();
            if re.header.id != new_id
                || re.header.parent().map(str::to_string).as_deref() != Some(log.header.id.as_str())
                || re.event_count != log.event_count
                || staged_cut != Some(inherited)
            {
                let _ = std::fs::remove_dir_all(&session_dir);
                return Err(errors::coded(
                    errors::ErrCode::State,
                    "复制校验失败：新会话身份、事件或继承长度与预期不符，已回滚",
                ));
            }
            // A copy that stops the target instance from booting is worse than
            // no copy: DSH refuses to start when the first frame is not the
            // header line alone. Decoding alone does not notice a bad frame
            // layout, so the publish gate checks DSH's rule explicitly.
            if let Err(e) = codec::header_frame_is_alone(&staged_bytes) {
                let _ = std::fs::remove_dir_all(&session_dir);
                let detail = match e {
                    codec::CodecError::FrameDecode(detail) => detail,
                    other => format!("{other:?}"),
                };
                return Err(errors::coded(
                    errors::ErrCode::State,
                    format!("复制校验失败：{detail}，已回滚"),
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
    target_encoding: codec::LogEncoding,
) -> Result<CopyOutcome, String> {
    let src_owned = SourceSession {
        project_dir: src.project_dir.clone(),
        dir: src.dir.clone(),
        artifact: src.artifact.clone(),
        encoding: src.encoding,
        generation: src.generation,
    };
    let target_owned = SessionEndpoint {
        instance_id: target.instance_id.clone(),
        dir: target.dir.clone(),
        home: target.home.clone(),
        manifest: target.manifest.clone(),
    };
    tokio::task::spawn_blocking(move || copy_one_sync(&src_owned, &target_owned, target_encoding))
        .await
        .map_err(|e| format!("复制任务异常退出: {e}"))?
}

#[cfg(test)]
mod tests;
