//! Session migration engine (development spec part 4 & 5): list, inspect, and
//! copy DSH conversations between managed instances.
//!
//! Design line, straight from the Spike and the spec:
//! - DSH's Session log is the single source of truth (spec §1.2). PHL reads and
//!   copies it; it never re-models conversations in its own store.
//! - A copy is *lineage-preserving* (spec §1.3, §5.1): the target gets a brand
//!   new session id, `parentSession` set to the source id, `seedLength` to the
//!   source's event count, and `cwd` preserved verbatim — it is a fork, so a
//!   user can resume it in DSH with the history already in place.
//! - Nothing here parses conversation content beyond the header record; events
//!   travel byte-for-byte (see [`codec`]). PHL therefore never takes on DSH's
//!   packed-row format as its own obligation.
//!
//! Safety rails a copy enforces, in order:
//! - both endpoints are managed instances resolved from *ids* (never a
//!   frontend-supplied path), and the target's own home is where it lands;
//! - the source must not be running (its log can be mid-write), and the target
//!   must not be running either (it would race PHL's file swap);
//! - an external (in-place) target is refused — writing into the user's own
//!   DSH_HOME is exactly what adoption's external mode promised never to do
//!   (`reject_external_write`);
//! - the write is staged (`copy.rs`), validated by re-decoding, then published
//!   with a rename so a crash leaves either nothing or a complete session.

pub(crate) mod codec;

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::errors;
use crate::instances::{home_of, instance_dir, load_manifest, InstanceManifest, ManagementMode};
use crate::launch::Processes;
use crate::paths::PhlState;

pub(crate) use codec::SessionHeaderRecord;

/// The physical session-log filename the backend writes (Spike §4).
pub(crate) const PLAIN_ARTIFACT: &str = "session.jsonl";
pub(crate) const ZSTD_ARTIFACT: &str = "session.jsonl.zstd";

/// One discovered session, from its header only — the list view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Opaque handle = the encoded session directory name (`session-…`).
    pub session_dir: String,
    /// The project group directory this session lives under (a cwd-derived key).
    pub project: String,
    pub id: String,
    /// Unix ms from the header, for the UI to sort/label.
    pub created_at_ms: u64,
    /// The absolute working directory, shown so the user recognises the
    /// conversation by where it happened.
    pub cwd: Option<String>,
    /// Lineage: the id this forked from, if any (may point outside this home).
    pub parent: Option<String>,
    /// Whether this is a subagent child (the UI can fold these away).
    pub origin_subagent: bool,
}

/// A session plus its stored event count — the inspect/preview view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub info: SessionInfo,
    /// Event records after the header. The copy's `seedLength`/lineage basis.
    pub event_count: usize,
}

/// What one copy produced, per target, so a multi-target fan-out can report
/// partial results honestly.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyOutcome {
    pub target_id: String,
    pub target_name: String,
    pub new_session_dir: String,
    pub new_id: String,
}

/// One endpoint of a copy: its instance id, resolved home, and the artifact
/// path + encoding we will read or write.
pub(crate) struct SessionEndpoint {
    pub(crate) instance_id: String,
    pub(crate) dir: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) manifest: InstanceManifest,
}

// ───────────────────────────── discovery walk ───────────────────────────── //

/// The absolute path of a session's log artifact, preferring the zstd physical
/// form a populated home uses and falling back to plaintext, per Spike §5.
fn artifact_path(session_dir: &Path) -> Option<(PathBuf, codec::LogEncoding)> {
    let zstd = session_dir.join(ZSTD_ARTIFACT);
    if zstd.is_file() {
        return Some((zstd, codec::LogEncoding::Zstd));
    }
    let plain = session_dir.join(PLAIN_ARTIFACT);
    if plain.is_file() {
        return Some((plain, codec::LogEncoding::Plain));
    }
    None
}

fn is_session_dir(name: &str) -> bool {
    name.starts_with("session-")
}

/// Read the header of one session directory. A directory that holds no readable
/// artifact, or whose header fails validation, is skipped with a note by the
/// caller — a broken session must not hide the whole list.
fn read_header(session_dir: &Path, project: &str) -> Result<SessionInfo, codec::CodecError> {
    let (path, _enc) = artifact_path(session_dir).ok_or(codec::CodecError::MissingHeader)?;
    let buf = std::fs::read(&path).map_err(|e| codec::CodecError::FrameDecode(e.to_string()))?;
    let log = codec::decode(&buf)?;
    Ok(info_from(&log.header, session_dir, project))
}

fn info_from(header: &SessionHeaderRecord, session_dir: &Path, project: &str) -> SessionInfo {
    let created_at_ms = header
        .value
        .get("createdAt")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    SessionInfo {
        session_dir: session_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        project: project.to_string(),
        id: header.id.clone(),
        created_at_ms,
        cwd: header.cwd().map(str::to_string),
        parent: header.parent().map(str::to_string),
        origin_subagent: header
            .value
            .get("origin")
            .and_then(serde_json::Value::as_str)
            == Some("subagent"),
    }
}

/// Walk `<home>/sessions/<project>/<session-dir>` collecting every readable
/// session. Blocking filesystem work; callers run it off the async thread.
/// `pub(crate)` also for the adoption wizard, which lists a *source* home's
/// conversations (it is not yet an instance) for the "选择对话" step.
pub(crate) fn list_in_home(home: &Path) -> (Vec<SessionInfo>, usize) {
    let root = home.join("sessions");
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let Ok(projects) = std::fs::read_dir(&root) else {
        return (out, 0);
    };
    for project in projects.flatten() {
        if !project.path().is_dir() {
            continue;
        }
        let project_name = project.file_name().to_string_lossy().into_owned();
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for sess in sessions.flatten() {
            let name = sess.file_name().to_string_lossy().into_owned();
            if !is_session_dir(&name) || !sess.path().is_dir() {
                continue;
            }
            match read_header(&sess.path(), &project_name) {
                Ok(info) => out.push(info),
                Err(_) => skipped += 1,
            }
        }
    }
    out.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| a.session_dir.cmp(&b.session_dir))
    });
    (out, skipped)
}

/// Resolve an instance id to a copy endpoint (managed copy or, for a source,
/// also an external home — the *reader* of a source is always allowed).
async fn endpoint(root: &Path, id: &str) -> Result<SessionEndpoint, String> {
    let dir = instance_dir(root, id)?;
    let manifest = load_manifest(&dir, id).await?;
    let home = home_of(&dir, &manifest);
    Ok(SessionEndpoint {
        instance_id: id.to_string(),
        dir,
        home,
        manifest,
    })
}

// ─────────────────────────────── commands ──────────────────────────────── //

/// List the sessions one instance's DSH_HOME holds. Read-only.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, PhlState>,
    id: String,
) -> Result<Vec<SessionInfo>, String> {
    let root = state.root();
    let ep = endpoint(&root, &id).await?;
    tokio::task::spawn_blocking(move || list_in_home(&ep.home).0)
        .await
        .map_err(|e| e.to_string())
}

/// Inspect one session: its header plus stored event count. `session_dir` is
/// the encoded directory name from `list_sessions`.
#[tauri::command]
pub async fn inspect_session(
    state: State<'_, PhlState>,
    id: String,
    session_dir: String,
) -> Result<SessionDetail, String> {
    let root = state.root();
    let ep = endpoint(&root, &id).await?;
    let session_dir = sanitize_session_dir(&session_dir)?;
    let home = ep.home;
    let found = tokio::task::spawn_blocking(move || find_and_read(home, session_dir))
        .await
        .map_err(|e| e.to_string())??;
    Ok(found)
}

fn find_and_read(home: PathBuf, session_dir: String) -> Result<SessionDetail, String> {
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Err(errors::coded(errors::ErrCode::NotFound, "实例暂无会话目录"));
    };
    for project in projects.flatten() {
        let cand = project.path().join(&session_dir);
        if !cand.is_dir() {
            continue;
        }
        let (path, _enc) = artifact_path(&cand)
            .ok_or_else(|| errors::coded(errors::ErrCode::NotFound, "会话日志文件缺失"))?;
        let buf = std::fs::read(&path).map_err(|e| e.to_string())?;
        let log = codec::decode(&buf).map_err(|e| e.to_command_error())?;
        let project_name = project.file_name().to_string_lossy().into_owned();
        return Ok(SessionDetail {
            info: info_from(&log.header, &cand, &project_name),
            event_count: log.event_count,
        });
    }
    Err(errors::coded(errors::ErrCode::NotFound, "未找到该会话"))
}

/// Copy one session into one target instance, preserving lineage and cwd.
/// Registered like `copy_sessions` (below) — the copy can be multi-GB and
/// writes into the target's home.
#[tauri::command]
pub async fn copy_session(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    processes: State<'_, Processes>,
    source_id: String,
    session_dir: String,
    target_id: String,
) -> Result<CopyOutcome, String> {
    let outcomes = guarded_session_copy(
        &locks,
        &tasks,
        &state,
        &processes,
        source_id,
        vec![session_dir],
        vec![target_id.clone()],
    )
    .await?;
    outcomes
        .into_iter()
        .next()
        .ok_or_else(|| "复制未产生结果".to_string())
}

/// Copy one or more sessions into one or more target instances. Every (session
/// × target) pair is independent; a failure is returned per the first error but
/// completed pairs are reported to the caller through the progress channel's
/// final aggregate. For P1 the command surfaces the full set on total success
/// and the first failure otherwise (the UI then narrows its selection).
///
/// Both copy commands run under `guarded` holding the source AND every target
/// instance (audit C): a multi-GB matrix used to run untracked — no
/// task-centre row, and nothing stopped a snapshot/clone/plugin-write from
/// racing the same homes mid-copy.
#[tauri::command]
pub async fn copy_sessions(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    processes: State<'_, Processes>,
    source_id: String,
    session_dirs: Vec<String>,
    target_ids: Vec<String>,
    on_progress: Channel<copy::SessionProgress>,
) -> Result<Vec<CopyOutcome>, String> {
    let mut resources: Vec<crate::resources::Resource> = std::iter::once(source_id.as_str())
        .chain(target_ids.iter().map(String::as_str))
        .map(|id| crate::resources::Resource::Instance(id.to_string()))
        .collect();
    resources.sort_by_key(|r| r.key());
    resources.dedup_by_key(|r| r.key());
    crate::resources::guarded(
        crate::resources::next_task_id("session-copy"),
        "session-copy",
        format!(
            "迁移 {} 个会话 → {} 个实例",
            session_dirs.len(),
            target_ids.len()
        ),
        resources,
        None,
        &locks,
        &tasks,
        move |task| async move {
            task.set_phase("copying");
            copy_sessions_inner_with(
                &state.root(),
                &processes,
                &source_id,
                &session_dirs,
                &target_ids,
                Some(&on_progress),
                Some(&task),
            )
            .await
        },
    )
    .await
}

/// `copy_session`'s wrapper: run the same guarded matrix for one pair, so the
/// single-copy command cannot bypass the locks the matrix command enforces.
async fn guarded_session_copy(
    locks: &crate::resources::ResourceLocks,
    tasks: &crate::resources::Tasks,
    state: &PhlState,
    processes: &Processes,
    source_id: String,
    session_dirs: Vec<String>,
    target_ids: Vec<String>,
) -> Result<Vec<CopyOutcome>, String> {
    let targets = target_ids.clone();
    crate::resources::guarded(
        crate::resources::next_task_id("session-copy"),
        "session-copy",
        format!(
            "迁移 {} 个会话 → {} 个实例",
            session_dirs.len(),
            target_ids.len()
        ),
        std::iter::once(source_id.clone())
            .chain(targets.into_iter())
            .map(crate::resources::Resource::Instance)
            .collect::<Vec<_>>(),
        None,
        locks,
        tasks,
        move |task| async move {
            task.set_phase("copying");
            copy_sessions_inner_with(
                &state.root(),
                processes,
                &source_id,
                &session_dirs,
                &target_ids,
                None,
                Some(&task),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn copy_sessions_inner(
    root: &Path,
    processes: &Processes,
    source_id: &str,
    session_dirs: &[String],
    target_ids: &[String],
) -> Result<Vec<CopyOutcome>, String> {
    copy_sessions_inner_with(
        root,
        processes,
        source_id,
        session_dirs,
        target_ids,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn copy_sessions_inner_with(
    root: &Path,
    processes: &Processes,
    source_id: &str,
    session_dirs: &[String],
    target_ids: &[String],
    on_progress: Option<&Channel<copy::SessionProgress>>,
    task: Option<&crate::resources::Task>,
) -> Result<Vec<CopyOutcome>, String> {
    if session_dirs.is_empty() {
        return Err("未选择任何会话".into());
    }
    if target_ids.is_empty() {
        return Err("未选择任何目标实例".into());
    }
    // Source: any managed instance is a legal reader (external homes included).
    let source = endpoint(root, source_id).await?;
    crate::instances::snapshot::ensure_not_running(processes, source_id)?;
    // Pre-validate every target's gate before touching a single file, so a
    // mid-list refusal (running / external / self) cannot leave partial copies.
    let mut targets = Vec::with_capacity(target_ids.len());
    for t in target_ids {
        if t == source_id {
            return Err("目标实例不能与源实例相同".into());
        }
        let ep = endpoint(root, t).await?;
        if ep.manifest.management_mode == ManagementMode::External {
            return Err(format!(
                "目标「{t}」是原地接入实例，其 DSH_HOME 由你自行管理，PHL 不向其写入会话"
            ));
        }
        crate::instances::snapshot::ensure_not_running(processes, t)?;
        targets.push(ep);
    }
    // Resolve the source's session artifact once per selected dir.
    let mut sources = Vec::with_capacity(session_dirs.len());
    for dir in session_dirs {
        let dir = sanitize_session_dir(dir)?;
        let located = copy::locate_source(&source.home, &dir)?;
        sources.push(located);
    }
    let total = targets.len() * sources.len();
    let mut outcomes = Vec::with_capacity(total);
    for target in &targets {
        for src in &sources {
            let outcome = copy::copy_one(src, target).await?;
            // The matrix owns the counters: every landed pair advances the
            // UI from `i/N`, which a select-all copy needs to read honestly.
            if let Some(ch) = on_progress {
                let _ = ch.send(copy::SessionProgress {
                    done: outcomes.len() + 1,
                    total,
                    target_id: outcome.target_id.clone(),
                    new_session_dir: outcome.new_session_dir.clone(),
                });
            }
            // The task-centre row reads the same ratio the panel shows.
            if let Some(t) = task {
                t.set_progress(Some((outcomes.len() + 1) as f64 / total as f64));
            }
            outcomes.push(outcome);
        }
    }
    Ok(outcomes)
}

/// A session dir name is DSH's `session-<uuid>`; keep it to the filesystem
/// whitelist the same way every other id-addressed path does, before it is
/// ever joined onto a home path.
pub(crate) fn sanitize_session_dir(name: &str) -> Result<String, String> {
    crate::paths::sanitize_segment(name, "会话目录名")?;
    if !name.starts_with("session-") {
        return Err(errors::coded(errors::ErrCode::State, "非法的会话目录名"));
    }
    Ok(name.to_string())
}

pub(crate) mod copy;

#[cfg(test)]
mod tests;
