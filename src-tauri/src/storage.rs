//! Storage-level operations on the whole data root: free-space queries and
//! moving the root's data between drives. The backend commits the root
//! pointer after data arrives; the frontend only mirrors that committed root.
//!
//! A migration is resumable, not a one-shot gamble. A journal file lives
//! beside the root pointer (outside both roots, so neither the old nor the
//! new location can swallow it) and records every directory: pending →
//! moving → moved, with byte counts. Cancellation or a crash between those
//! rows is the normal case the design assumes: a restart finds the journal,
//! shows a resume/undo entry, and either re-runs from where the record says
//! or rolls the half-copied directories back. The root pointer is only ever
//! re-pointed after a fully committed migration — and the commit happens
//! inside `move_root_data` itself: the pointer moves first, the journal goes
//! with it (one `relocate_root`). A crash can therefore never find the data
//! already relocated *and* the journal gone; the committed-journal-with-stale-
//! pointer window is finished by 完成切换 (`storage_migration_finish`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::instances::{copy_tree_with_progress, dir_size, LinkMatch, LinkPolicy, SkipRule};
use crate::paths::PhlState;
use crate::versions::Transfers;

/// Every subdirectory of the root that carries PHL data, in migration order.
/// `cache` is disposable but still moved, so the freed space actually arrives
/// on the new drive instead of re-accumulating on the old one.
const DATA_DIRS: [&str; 5] = ["instances", "versions", "runtimes", "config", "cache"];

/// The journal's file name, resolved beside the root pointer.
const JOURNAL_NAME: &str = "migration.json";
/// Cross-drive copies land here first, then get renamed into place.
const STAGING_DIR: &str = ".phl-staging";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirSummary {
    pub exists: bool,
    pub entries: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootDataSummary {
    pub instances: DirSummary,
    pub versions: DirSummary,
    pub runtimes: DirSummary,
    pub config: DirSummary,
    pub cache: DirSummary,
    pub has_data: bool,
}

/// Cheap, one-level-deep look at what a root still holds. Deep byte counts
/// are deliberately avoided: the caller only needs "is there anything worth
/// migrating", and walking node_modules trees for that would be slow.
#[tauri::command]
pub fn root_data_summary(root: String) -> RootDataSummary {
    let dir = |name: &str| {
        let path = Path::new(&root).join(name);
        let exists = path.exists();
        let entries = std::fs::read_dir(&path).map(|d| d.count()).unwrap_or(0);
        DirSummary { exists, entries }
    };
    let summary = RootDataSummary {
        instances: dir("instances"),
        versions: dir("versions"),
        runtimes: dir("runtimes"),
        config: dir("config"),
        cache: dir("cache"),
        has_data: false,
    };
    RootDataSummary {
        has_data: summary.instances.entries > 0
            || summary.versions.entries > 0
            || summary.runtimes.entries > 0
            || summary.config.entries > 0
            || summary.cache.entries > 0,
        ..summary
    }
}

/// Free bytes on the drive containing `path`. The path may not exist yet (a
/// root the user is only considering), so the probe walks up to the nearest
/// existing ancestor.
#[tauri::command]
pub fn free_space(path: String) -> Result<u64, String> {
    let mut probe = PathBuf::from(&path);
    loop {
        if probe.as_os_str().is_empty() {
            return Err(format!("无法定位磁盘: {path}"));
        }
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => return Err(format!("无法定位磁盘: {path}")),
        }
    }
    fs2::available_space(&probe).map_err(|e| format!("无法读取磁盘剩余空间: {e}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveProgress {
    /// Which data directory is currently being moved.
    pub kind: String,
    pub progress: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveSummary {
    pub moved: Vec<String>,
    pub bytes: u64,
    pub cancelled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveOutcome {
    #[serde(flatten)]
    summary: MoveSummary,
    /// Only present after the durable pointer and in-memory root agree.
    root: Option<String>,
}

/* ------------------------------ journal ------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Not started.
    Pending,
    /// Copy/rename was in flight. Staging-only work can be retried; a final
    /// destination without a committed row requires manual reconciliation.
    Moving,
    /// Fully relocated to the destination (or confirmed absent from source).
    Moved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub kind: String,
    pub state: EntryState,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationJournal {
    pub from: String,
    pub to: String,
    pub entries: Vec<JournalEntry>,
    pub committed: bool,
    pub started_at: String,
}

impl MigrationJournal {
    fn fresh(from: &Path, to: &Path) -> Self {
        MigrationJournal {
            from: from.to_string_lossy().into_owned(),
            to: to.to_string_lossy().into_owned(),
            entries: DATA_DIRS
                .iter()
                .map(|k| JournalEntry {
                    kind: (*k).to_string(),
                    state: EntryState::Pending,
                    bytes: 0,
                })
                .collect(),
            committed: false,
            started_at: crate::versions::now_iso(),
        }
    }

    fn entry(&self, kind: &str) -> EntryState {
        self.entries
            .iter()
            .find(|e| e.kind == kind)
            .map(|e| e.state)
            .unwrap_or(EntryState::Pending)
    }

    fn set(&mut self, kind: &str, state: EntryState, bytes: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.kind == kind) {
            e.state = state;
            e.bytes = bytes;
        }
    }

    fn moved_kinds(&self) -> (Vec<String>, u64) {
        let mut kinds = Vec::new();
        let mut bytes = 0u64;
        for e in &self.entries {
            if e.state == EntryState::Moved {
                kinds.push(e.kind.clone());
                bytes += e.bytes;
            }
        }
        (kinds, bytes)
    }

    /// tmp+rename: a torn journal file is worse than no journal file, it is
    /// the only record of where half-moved data is.
    fn save(&self, path: &Path) -> Result<(), String> {
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn read_journal(path: &Path) -> Result<Option<MigrationJournal>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("迁移记录已损坏，无法解析: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("无法读取迁移记录: {e}")),
    }
}

/// The state the frontend keys its restart-recovery banner off: an
/// uncommitted journal is, by definition, an interrupted or cancelled
/// migration awaiting 继续 or 撤销.
#[tauri::command]
pub fn storage_migration_status(
    phl: State<'_, PhlState>,
) -> Result<Option<MigrationJournal>, String> {
    match phl.sibling_file(JOURNAL_NAME) {
        Some(path) => read_journal(&path),
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn storage_migration_undo(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
) -> Result<MoveSummary, String> {
    let Some(path) = phl.sibling_file(JOURNAL_NAME) else {
        return Err("没有迁移记录可撤销".into());
    };
    crate::resources::guarded(
        crate::resources::next_task_id("root-migration-undo"),
        "root-migration-undo",
        "撤销未完成的数据目录迁移",
        vec![crate::resources::Resource::DataRoot],
        None,
        &locks,
        &tasks,
        move |task| async move {
            let journal =
                read_journal(&path)?.ok_or_else(|| "迁移记录不存在，无法撤销".to_string())?;
            undo_migration(&path, &journal, &task).await
        },
    )
    .await
}

/// Roll a non-committed migration back: everything that arrived in `to`
/// (moved and half-copied alike) is walked back to `from`, then the journal
/// is deleted. Data that never left the source stays untouched.
async fn undo_migration(
    path: &Path,
    journal: &MigrationJournal,
    task: &crate::resources::Task,
) -> Result<MoveSummary, String> {
    if journal.committed {
        return Err("迁移已完成提交，不能整体撤销".into());
    }
    let from = PathBuf::from(&journal.from);
    let to = PathBuf::from(&journal.to);
    // A crash can leave a completed rename, or a partly deleted source,
    // before the journal catches up. Neither tree is disposable evidence.
    // Check all rows before undo mutates even the first directory.
    for e in &journal.entries {
        ensure_recovery_is_unambiguous(e.state, &from.join(&e.kind), &to.join(&e.kind))?;
        if e.state == EntryState::Moved
            && from.join(&e.kind).try_exists().map_err(|e| e.to_string())?
            && to.join(&e.kind).try_exists().map_err(|e| e.to_string())?
        {
            return Err(format!(
                "源和目标都保留了 {}，无法确认源目录是否完整；已保留两侧数据和迁移记录，请核对后恢复",
                e.kind
            ));
        }
    }
    let mut returned = Vec::new();
    let mut bytes = 0u64;
    for e in &journal.entries {
        if e.state == EntryState::Pending {
            continue;
        }
        // A cross-volume undo copies gigabytes with no progress channel; the
        // task row at least names the directory being restored.
        task.set_phase(&format!("撤销 {}", e.kind));
        let src = to.join(&e.kind);
        let back = from.join(&e.kind);
        if src.exists() {
            if back.exists() {
                // Recheck after preflight in case a source appeared meanwhile.
                return Err(format!("源目录 {} 已存在，已保留目标副本", back.display()));
            } else {
                tokio::fs::create_dir_all(&from)
                    .await
                    .map_err(|err| err.to_string())?;
                if let Err(rename_err) = tokio::fs::rename(&src, &back).await {
                    // A migration that crossed volumes cannot be undone by a
                    // rename; put it back the same way it was copied forward.
                    copy_back(&e.kind, &src, &back, &from, &to)
                        .await
                        .map_err(|err| {
                            format!(
                                "无法把 {} 搬回 {}：重命名失败（{rename_err}），复制也失败（{err}）",
                                src.display(),
                                back.display()
                            )
                        })?;
                } else {
                    // The forward pass re-pointed every managed link at `to`, so
                    // rolling the move back has to translate them again — the
                    // restored tree would otherwise reference a root that is being
                    // emptied. `Raw` matching, because `to/versions` may never have
                    // moved and therefore cannot be canonicalized. The walk and its
                    // `mklink` calls are blocking, so they run off the runtime.
                    let rewritten = tokio::task::spawn_blocking({
                        let (dir, from_root, to_root) = (back.clone(), to.clone(), from.clone());
                        move || {
                            crate::instances::repoint_managed_links(
                                &dir,
                                &from_root,
                                &to_root,
                                LinkMatch::Raw,
                            )
                        }
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("链接重写线程异常退出: {e}")));
                    if let Err(err) = rewritten {
                        let _ = tokio::fs::rename(&back, &src).await;
                        return Err(format!(
                            "无法还原 {} 中的链接: {err}（该目录已放回目标根）",
                            back.display()
                        ));
                    }
                }
            }
            returned.push(e.kind.clone());
            bytes += e.bytes;
        }
    }
    let _ = tokio::fs::remove_dir_all(to.join(STAGING_DIR)).await;
    let _ = std::fs::remove_file(path);
    Ok(MoveSummary {
        moved: returned,
        bytes,
        cancelled: true,
    })
}

/// Undo one directory of a migration that crossed volumes, where no rename
/// exists to roll back. The forward pass handles that case by copy + staging;
/// this mirrors it — copy into `.phl-staging` under the target root, verify the
/// byte count, rename the verified staging directory into place, and only then
/// delete the copy at `to_root`. Links are translated during the copy
/// (`Rewrite`), so the separate repoint pass the rename path needs would be
/// both unnecessary and wrong here.
async fn copy_back(
    kind: &str,
    src: &Path,
    back: &Path,
    from_root: &Path,
    to_root: &Path,
) -> Result<(), String> {
    let stage_root = from_root.join(STAGING_DIR);
    let stage = stage_root.join(kind);
    let _ = tokio::fs::remove_dir_all(&stage).await;
    tokio::fs::create_dir_all(&stage_root)
        .await
        .map_err(|e| e.to_string())?;
    let bytes = tokio::task::spawn_blocking({
        let src = src.to_path_buf();
        move || dir_size(&src)
    })
    .await
    .unwrap_or(0);
    let copied = copy_tree_with_progress(
        src.to_path_buf(),
        stage.clone(),
        Arc::new(AtomicBool::new(false)),
        SkipRule::Nothing,
        LinkPolicy::Rewrite {
            new_root: from_root.to_path_buf(),
            match_on: LinkMatch::Canonical,
            source_root: src.to_path_buf(),
        },
        to_root.to_path_buf(),
        &|_p| {},
    )
    .await;
    if let Err(error) = copied {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(error);
    }
    let staged = tokio::task::spawn_blocking({
        let stage = stage.clone();
        move || dir_size(&stage)
    })
    .await
    .unwrap_or(0);
    if staged != bytes {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(format!(
            "{} 的暂存副本校验不一致（源 {bytes} 字节，暂存 {staged} 字节），已中止",
            src.display()
        ));
    }
    tokio::fs::rename(&stage, back).await.map_err(|e| {
        format!(
            "{} 已暂存于 {} 并通过校验，放回原根失败（可重试撤销）: {e}",
            kind,
            stage.display()
        )
    })?;
    // Only now is the destination copy disposable — the restored directory is
    // complete and verified.
    if let Err(e) = tokio::fs::remove_dir_all(src).await {
        eprintln!("[phl] 已把 {kind} 复制回原根，但删除目标根中的副本失败（可稍后手动删除）: {e}");
    }
    let _ = tokio::fs::remove_dir_all(&stage_root).await;
    Ok(())
}

fn ensure_recovery_is_unambiguous(state: EntryState, src: &Path, dst: &Path) -> Result<(), String> {
    if state == EntryState::Moving && dst.try_exists().map_err(|e| e.to_string())? {
        return Err(format!(
            "迁移在目录落位时中断，无法确认 {} 与 {} 哪一侧完整；已保留两侧数据和迁移记录，请核对后恢复",
            src.display(), dst.display()
        ));
    }
    Ok(())
}

/* ------------------------------ migration ------------------------------ */

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn move_root_data(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    from: String,
    to: String,
    on_progress: Channel<MoveProgress>,
) -> Result<MoveOutcome, String> {
    let flag = transfers.take(&transfer_id);
    let journal_path = phl.sibling_file(JOURNAL_NAME);
    let phl = phl.shared();
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "root-migration",
        format!("迁移数据目录 → {to}"),
        vec![crate::resources::Resource::DataRoot],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
            if let Some(path) = journal_path.as_deref() {
                retire_completed_migration(&phl, path, Path::new(&from), Path::new(&to))?;
            }
            let r = move_root_inner(
                &flag,
                &task,
                journal_path.as_deref(),
                Path::new(&from),
                Path::new(&to),
                &|p| {
                    let _ = on_progress.send(p);
                },
            )
            .await;
            if crate::versions::cancelled(&flag) {
                return Err("cancelled".into());
            }
            // A committed migration is finished *here*: the root pointer is
            // re-pointed at the new root inside this same DataRoot-guarded
            // operation, and only then does the journal go (one
            // `relocate_root`). Deleting the record before the pointer moves
            // is the window where a crash strands the data at `to` with no
            // recovery entry; committing both here closes it. A pointer
            // commit that fails leaves the journal in place — already
            // committed, so the next boot offers 完成切换 (see
            // `storage_migration_finish`).
            let summary = r?;
            complete_move(&phl, journal_path.as_deref(), Path::new(&to), summary)
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

fn same_location(a: &Path, b: &Path) -> bool {
    let normalized = |p: &Path| {
        crate::paths::strip_verbatim(&std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
    };
    let (a, b) = (normalized(a), normalized(b));
    #[cfg(windows)]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// A journal deletion can fail after its root was committed. It is safe to
/// retire that metadata for a new move only when both the authoritative root
/// and the requested source are the previous destination. Never apply an old
/// migration's successful summary to a different pair of directories.
fn retire_completed_migration(
    phl: &PhlState,
    path: &Path,
    from: &Path,
    to: &Path,
) -> Result<(), String> {
    let Some(j) = read_journal(path)? else {
        return Ok(());
    };
    if j.committed
        && same_location(&phl.root(), Path::new(&j.to))
        && same_location(from, &phl.root())
        && !(same_location(from, Path::new(&j.from)) && same_location(to, Path::new(&j.to)))
    {
        std::fs::remove_file(path).map_err(|e| {
            format!("上次迁移已完成，但迁移记录无法清理；请检查配置目录权限后重试: {e}")
        })?;
    }
    Ok(())
}

fn complete_move(
    phl: &PhlState,
    journal_path: Option<&Path>,
    to: &Path,
    summary: MoveSummary,
) -> Result<MoveOutcome, String> {
    let root = if summary.cancelled {
        None
    } else {
        let target = match journal_path {
            Some(path) => {
                let j = read_journal(path)?.ok_or("迁移记录缺失，无法确认目标目录")?;
                if !j.committed || !same_location(Path::new(&j.to), to) {
                    return Err("迁移记录与目标不一致，未切换数据目录".into());
                }
                PathBuf::from(j.to)
            }
            None => std::fs::canonicalize(to).map_err(|e| e.to_string())?,
        };
        Some(
            phl.relocate_root(target, journal_path)?
                .to_string_lossy()
                .into_owned(),
        )
    };
    Ok(MoveOutcome { summary, root })
}

/// The finish step itself: the data of a committed migration is complete at
/// `to`, only the pointer commit is missing. Takes the state by reference so
/// it is testable without a Tauri `State`, and returns the adopted root.
fn finish_committed_migration(phl: &PhlState, path: &Path) -> Result<String, String> {
    let journal = read_journal(path)?.ok_or_else(|| "迁移记录不存在，无法确认".to_string())?;
    if !journal.committed {
        return Err("迁移尚未完成数据搬运，请继续迁移或撤销".into());
    }
    let to = phl.relocate_root(PathBuf::from(&journal.to), Some(path))?;
    Ok(to.to_string_lossy().into_owned())
}

/// Finish a migration whose data fully arrived (committed journal) but whose
/// root-pointer commit never happened — the narrow crash/failure window
/// between the last directory landing and `move_root_data`'s own
/// `relocate_root`. Re-pointing the pointer is the backend's job, so this
/// runs the identical commit rather than trusting the UI to re-send a path.
#[tauri::command]
pub async fn storage_migration_finish(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
) -> Result<String, String> {
    let Some(path) = phl.sibling_file(JOURNAL_NAME) else {
        return Err("没有迁移记录可确认".into());
    };
    crate::resources::guarded(
        crate::resources::next_task_id("root-migration-finish"),
        "root-migration-finish",
        "完成未确认的数据目录迁移",
        vec![crate::resources::Resource::DataRoot],
        None,
        &locks,
        &tasks,
        move |_task| async move { finish_committed_migration(&phl, &path) },
    )
    .await
}

async fn move_root_inner<F: Fn(MoveProgress) + Send + Sync>(
    flag: &Arc<AtomicBool>,
    task: &crate::resources::Task,
    journal_path: Option<&Path>,
    from: &Path,
    to: &Path,
    on_progress: &F,
) -> Result<MoveSummary, String> {
    // Nesting either way would misplace data: moving a root into itself
    // recurses forever, and parking the destination inside the source strands
    // the old data under the new root.
    if from.starts_with(to) || to.starts_with(from) {
        return Err("新旧目录不能互为父子".into());
    }
    if !from.exists() {
        return Err(format!("源目录不存在: {}", from.display()));
    }
    tokio::fs::create_dir_all(to)
        .await
        .map_err(|e| format!("无法创建目标目录: {e}"))?;

    // Resolve aliases, `..`, drive-letter casing and directory junctions before
    // checking ancestry. Lexical prefixes alone do not identify real paths.
    let from = std::fs::canonicalize(from).map_err(|e| e.to_string())?;
    let to = std::fs::canonicalize(to).map_err(|e| e.to_string())?;
    if from.starts_with(&to) || to.starts_with(&from) {
        return Err("新旧目录不能互为父子".into());
    }

    // Resume or start: the journal decides which directories are already
    // committed to the destination. A committed leftover (crash between the
    // final row and the file deletion) is just finished business. An open
    // journal for a *different* pair must be resolved first — silently
    // switching direction would strand the half-moved data.
    let existing = match journal_path {
        Some(p) => read_journal(p)?,
        None => None,
    };
    let mut journal = match existing {
        Some(j)
            if j.committed
                && same_location(Path::new(&j.from), &from)
                && same_location(Path::new(&j.to), &to) =>
        {
            let (moved, bytes) = j.moved_kinds();
            return Ok(MoveSummary {
                moved,
                bytes,
                cancelled: false,
            });
        }
        Some(j)
            if !j.committed
                && same_location(Path::new(&j.from), &from)
                && same_location(Path::new(&j.to), &to) =>
        {
            j
        }
        Some(j) => {
            return Err(crate::errors::coded(
                crate::errors::ErrCode::State,
                format!(
                    "存在未完成的迁移 {} → {}，请先在设置中继续或撤销它",
                    j.from, j.to
                ),
            ))
        }
        None => {
            let j = MigrationJournal::fresh(&from, &to);
            if let Some(p) = journal_path {
                j.save(p)?;
            }
            j
        }
    };

    // Check every destination before moving the first directory. A conflict
    // in config/ must not strand instances/ in the other root. Directories
    // already `moved` in a resumed run are the expected occupants of their
    // destination and skip the guard.
    for kind in DATA_DIRS {
        ensure_recovery_is_unambiguous(journal.entry(kind), &from.join(kind), &to.join(kind))?;
        if journal.entry(kind) == EntryState::Moved {
            continue;
        }
        // An interrupted staging copy can be retried. A final destination
        // was rejected above because its ownership/completeness is ambiguous.
        if journal.entry(kind) == EntryState::Moving {
            continue;
        }
        if !from.join(kind).exists() {
            continue;
        }
        let dst = to.join(kind);
        if dst.exists() {
            let mut entries = std::fs::read_dir(&dst)
                .map_err(|e| format!("无法检查目标目录 {}: {e}", dst.display()))?;
            if entries
                .next()
                .transpose()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err(format!(
                    "目标已存在非空目录 {} —— 请改用一个空目录",
                    dst.display()
                ));
            }
        }
    }

    let mut moved: Vec<String> = Vec::new();
    let mut bytes_moved = 0u64;
    // A resumed run carries the earlier runs' arrivals into the summary.
    if journal_started(&journal) {
        let (done_kinds, done_bytes) = journal.moved_kinds();
        moved.extend(done_kinds);
        bytes_moved += done_bytes;
    }

    for kind in DATA_DIRS {
        if flag.load(Ordering::SeqCst) {
            // A partial stop is a valid result: the journal records exactly
            // what has arrived, and a re-run skips whatever is already moved.
            return Ok(MoveSummary {
                moved,
                bytes: bytes_moved,
                cancelled: true,
            });
        }
        if journal.entry(kind) == EntryState::Moved {
            continue;
        }
        task.set_phase(kind);
        // Only staging is known to be disposable. Preflight rejects an
        // ambiguous final destination before any directory is changed.
        if journal.entry(kind) == EntryState::Moving {
            let _ = tokio::fs::remove_dir_all(to.join(STAGING_DIR).join(kind)).await;
        }
        journal.set(kind, EntryState::Moving, 0);
        if let Some(p) = journal_path {
            journal.save(p)?;
        }

        let src = from.join(kind);
        if !src.exists() {
            journal.set(kind, EntryState::Moved, 0);
            if let Some(p) = journal_path {
                journal.save(p)?;
            }
            continue;
        }
        let dst = to.join(kind);
        if dst.exists() {
            let empty = std::fs::read_dir(&dst)
                .map(|mut d| d.next().is_none())
                .unwrap_or(true);
            if !empty {
                return Err(format!(
                    "目标已存在非空目录 {} —— 请改用一个空目录",
                    dst.display()
                ));
            }
            tokio::fs::remove_dir(&dst)
                .await
                .map_err(|e| e.to_string())?;
        }

        // The full size, and below a copy that skips nothing. Relocating the
        // data root *replaces* the original — a filter that made sense for
        // cloning an instance would drop every `snapshots/` and `logs/` here
        // and the source is deleted right after, destroying them for good.
        // Blocking tree walk: off the runtime, like the copy itself.
        let bytes = tokio::task::spawn_blocking({
            let src = src.clone();
            move || dir_size(&src)
        })
        .await
        .unwrap_or(0);

        // Fast path, same drive: rename. The journal row already says
        // `moving`, so an interrupt here is recovered the same way as a
        // half-finished copy.
        if tokio::fs::rename(&src, &dst).await.is_ok() {
            // Rename preserves links verbatim — including absolute targets
            // into the OLD root that the deletion step will erase. Re-point
            // them before declaring the kind moved (and refuse + roll the
            // rename back on anything the copy path would refuse).
            // `Raw`: the tree has already been renamed, so a link into its old
            // location (pnpm's `.pnpm`, for instance) is dangling and could
            // never be classified by resolving it. The rename moves paths, and
            // path text is exactly what has to be translated. The walk and its
            // `mklink` calls are blocking, so they run off the runtime.
            let rewritten = tokio::task::spawn_blocking({
                let (dir, from_root, to_root) = (dst.clone(), from.clone(), to.clone());
                move || {
                    crate::instances::repoint_managed_links(
                        &dir,
                        &from_root,
                        &to_root,
                        LinkMatch::Raw,
                    )
                }
            })
            .await
            .unwrap_or_else(|e| Err(format!("链接重写线程异常退出: {e}")));
            if let Err(e) = rewritten {
                let _ = tokio::fs::rename(&dst, &src).await;
                journal.set(kind, EntryState::Pending, 0);
                if let Some(p) = journal_path {
                    let _ = journal.save(p);
                }
                return Err(e);
            }
            moved.push(kind.into());
            bytes_moved += bytes;
            journal.set(kind, EntryState::Moved, bytes);
            if let Some(p) = journal_path {
                journal.save(p)?;
            }
            on_progress(MoveProgress {
                kind: kind.into(),
                progress: 1.0,
                bytes_done: bytes,
                bytes_total: bytes,
            });
            continue;
        }

        // Cross drive: copy into staging, verify the byte count, then rename
        // staging onto the destination — a directory that never appears
        // half-populated at `to/<kind>` is what makes a committed journal
        // row trustworthy.
        let stage_root = to.join(STAGING_DIR);
        let stage = stage_root.join(kind);
        let _ = tokio::fs::remove_dir_all(&stage).await;
        tokio::fs::create_dir_all(&stage_root)
            .await
            .map_err(|e| e.to_string())?;
        let copy_result = copy_tree_with_progress(
            src.clone(),
            stage.clone(),
            Arc::clone(flag),
            SkipRule::Nothing,
            // The whole root moves, so a link into the shared `versions/` tree
            // or into this subtree is re-pointed at the same relative location
            // under the destination instead of at the old absolute prefix,
            // which dangles once the source is deleted.
            LinkPolicy::Rewrite {
                new_root: to.clone(),
                match_on: LinkMatch::Canonical,
                source_root: src.clone(),
            },
            from.clone(),
            &|p| {
                on_progress(MoveProgress {
                    kind: kind.into(),
                    progress: p.progress,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                })
            },
        )
        .await;
        if flag.load(Ordering::SeqCst) {
            // A cancelled copy must not leave a partial tree behind in
            // staging; the `moving` row tells the next run to redo this kind.
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Ok(MoveSummary {
                moved,
                bytes: bytes_moved,
                cancelled: true,
            });
        }
        if let Err(error) = copy_result {
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Err(error);
        }
        let staged = tokio::task::spawn_blocking({
            let stage = stage.clone();
            move || dir_size(&stage)
        })
        .await
        .unwrap_or(0);
        if staged != bytes {
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Err(format!(
                "{} 的暂存副本校验不一致（源 {bytes} 字节，暂存 {staged} 字节），已中止",
                src.display()
            ));
        }
        tokio::fs::rename(&stage, &dst).await.map_err(|e| {
            format!(
                "{} 已暂存于 {} 并通过校验，放入目标失败（可重试迁移）: {e}",
                kind,
                stage.display()
            )
        })?;

        // Persist the verified destination BEFORE deleting any source byte.
        // A crash during source deletion must never trigger a recopy from a
        // now-incomplete source and erase the complete destination.
        journal.set(kind, EntryState::Moved, bytes);
        if let Some(p) = journal_path {
            journal.save(p)?;
        }

        // Source removal is the one step that is safe to *not* be fatal: the
        // destination holds a verified copy, the data is not lost, and the
        // journal says `moved` so no future attempt re-copies it.
        match tokio::fs::remove_dir_all(&src).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("[phl] 已复制 {kind} 到新目录，但删除源失败（可稍后手动删除）: {e}")
            }
        }
        moved.push(kind.into());
        bytes_moved += bytes;
        journal.set(kind, EntryState::Moved, bytes);
        if let Some(p) = journal_path {
            journal.save(p)?;
        }
    }

    let _ = tokio::fs::remove_dir_all(to.join(STAGING_DIR)).await;
    journal.committed = true;
    if let Some(p) = journal_path {
        journal.save(p)?;
    }
    task.set_phase("committed");

    Ok(MoveSummary {
        moved,
        bytes: bytes_moved,
        cancelled: false,
    })
}

/// True once any row has progressed past the initial state — used to decide
/// whether a resumed run should pre-load its byte totals.
fn journal_started(j: &MigrationJournal) -> bool {
    j.entries.iter().any(|e| e.state != EntryState::Pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "phl-storage-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A writable directory on a volume *other* than the one holding
    /// `%TEMP%`, or `None` on a single-volume machine. A cross-volume undo can
    /// only be proven against a real second volume: a mocked rename failure
    /// would test the mock, not Windows.
    struct OtherVolume(PathBuf);
    impl OtherVolume {
        fn new() -> Option<Self> {
            let temp = std::env::temp_dir();
            let temp_prefix = temp.components().next()?;
            for letter in b'C'..=b'Z' {
                let root = PathBuf::from(format!("{}:\\", letter as char));
                if !root.is_dir() || root.components().next() == Some(temp_prefix) {
                    continue;
                }
                let probe = root.join(format!("phl-cross-volume-{}", std::process::id()));
                if std::fs::create_dir_all(&probe).is_ok() {
                    return Some(Self(probe));
                }
            }
            None
        }
    }
    impl Drop for OtherVolume {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Compare two paths by *identity*, not spelling.
    ///
    /// Windows gives the same directory two names: the 8.3 short form
    /// (`C:\Users\RUNNER~1\…`) and the long form (`C:\Users\runneradmin\…`).
    /// CI's `%TEMP%` is the short one, so a link created from it and a path
    /// canonicalized from it are the same directory but different strings —
    /// comparing the text made these tests fail on the runner only.
    fn same_dir(a: &Path, b: &Path) -> bool {
        let norm = |p: &Path| {
            std::fs::canonicalize(p)
                .map(|c| crate::paths::strip_verbatim(&c))
                .unwrap_or_else(|_| crate::paths::strip_verbatim(p))
        };
        norm(a) == norm(b)
    }

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn null_task() -> crate::resources::Task {
        let tasks = crate::resources::Tasks::default();
        // The registry row is dropped right after; only set_phase is used.
        tasks
            .begin(
                crate::resources::TaskInfo::new(
                    "test-migration".into(),
                    "test",
                    String::new(),
                    &[],
                ),
                None,
            )
            .unwrap()
    }

    async fn move_with_journal(
        flag: &Arc<AtomicBool>,
        journal: &Path,
        from: &Path,
        to: &Path,
    ) -> Result<MoveSummary, String> {
        move_root_inner(flag, &null_task(), Some(journal), from, to, &|_| {}).await
    }

    #[tokio::test]
    async fn config_only_root_is_detected_and_migrated() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::write(from.join("config/api.json"), b"test-config").unwrap();
        let summary = root_data_summary(from.to_string_lossy().into_owned());
        assert!(summary.has_data);
        assert_eq!(summary.config.entries, 1);
        let result = move_with_journal(&no_cancel(), &root.0.join(JOURNAL_NAME), &from, &to)
            .await
            .unwrap();
        assert_eq!(result.moved, vec!["config"]);
        assert_eq!(
            std::fs::read(to.join("config/api.json")).unwrap(),
            b"test-config"
        );
        assert!(!from.join("config").exists());
        // Committed migrations leave no journal behind for the next boot.
        let j = read_journal(&root.0.join(JOURNAL_NAME))
            .unwrap()
            .expect("inner does not delete the file; the command wrapper does");
        assert!(j.committed);
    }

    #[tokio::test]
    async fn late_destination_conflict_leaves_all_sources_untouched() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        for kind in ["instances", "config"] {
            std::fs::create_dir_all(from.join(kind)).unwrap();
            std::fs::write(from.join(kind).join("data"), b"original").unwrap();
        }
        std::fs::create_dir_all(to.join("config")).unwrap();
        std::fs::write(to.join("config/api.json"), b"existing").unwrap();
        assert!(
            move_with_journal(&no_cancel(), &root.0.join(JOURNAL_NAME), &from, &to)
                .await
                .is_err()
        );
        assert!(from.join("instances/data").exists());
        assert!(from.join("config/data").exists());
        assert!(!to.join("instances").exists());
        assert_eq!(
            std::fs::read(to.join("config/api.json")).unwrap(),
            b"existing"
        );
    }

    #[tokio::test]
    async fn a_migration_repoints_managed_links_at_the_new_root() {
        // Defect #4's relocation half: instances move before versions, so the
        // link must be *translated* to the new root — kept as a live link
        // (not materialized gigabytes, not refused, and never left pointing
        // at the old absolute prefix the deletion step is about to erase).
        use crate::instances::copy::recreate_link;
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        let dep = from
            .join("versions")
            .join("v1")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(dep.join("index.js"), b"shared-module").unwrap();
        let nm = from
            .join("instances")
            .join("a")
            .join("dsh-home")
            .join("profiles");
        std::fs::create_dir_all(&nm).unwrap();
        recreate_link(
            &nm.join("node_modules"),
            &from.join("versions").join("v1").join("node_modules"),
            true,
        )
        .unwrap();
        let summary = move_with_journal(&no_cancel(), &root.0.join(JOURNAL_NAME), &from, &to)
            .await
            .unwrap();
        assert!(summary.moved.contains(&"instances".to_string()));

        // The migrated link resolves through to the *migrated* content.
        let migrated_link = to
            .join("instances")
            .join("a")
            .join("dsh-home")
            .join("profiles")
            .join("node_modules");
        assert!(
            std::fs::symlink_metadata(&migrated_link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "the link survived as a link — nothing materialized"
        );
        assert_eq!(
            std::fs::read(migrated_link.join("dep").join("index.js")).unwrap(),
            b"shared-module",
            "reads resolve through the translated link into the new versions/"
        );
        // The junction's raw target must name the NEW root's versions/ dir —
        // exactly the translation repoint performs (not the old absolute path
        // the rename carried along, which the deletion step would orphan).
        let raw_target = crate::paths::strip_verbatim(&std::fs::read_link(&migrated_link).unwrap());
        let expected = crate::paths::strip_verbatim(&std::fs::canonicalize(&to).unwrap())
            .join("versions")
            .join("v1")
            .join("node_modules");
        assert_eq!(
            raw_target, expected,
            "the link was translated to the new root"
        );
        assert!(
            !from.join("instances").exists(),
            "the source went as always"
        );
    }

    #[tokio::test]
    async fn a_same_drive_move_refuses_escaping_links_and_leaves_the_source_whole() {
        // The fast path must not be the soft spot: a link that the copy path
        // would refuse (here, one into a non-versions directory) aborts the
        // move with the source exactly where it started.
        use crate::instances::copy::recreate_link;
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        let inst = from.join("instances").join("a");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::write(inst.join("file"), b"keep").unwrap();
        // A managed link that classifies FIRST (name order), so the refusal
        // below happens after it was already classified: a one-phase rewrite
        // would have moved it into `to/versions`, which does not exist yet.
        let shared = from.join("versions").join("v1").join("node_modules");
        std::fs::create_dir_all(shared.join("dep")).unwrap();
        std::fs::write(shared.join("dep").join("index.js"), b"shared-gold").unwrap();
        recreate_link(&inst.join("a-deps"), &shared, true).unwrap();
        let elsewhere = from.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        recreate_link(&inst.join("z-borrowed"), &elsewhere, true).unwrap();

        let err = move_with_journal(&no_cancel(), &root.0.join(JOURNAL_NAME), &from, &to)
            .await
            .unwrap_err();
        assert!(err.contains("受管版本之外"), "got: {err}");
        // Rolled back: the source tree stands with its links, the destination
        // holds nothing that would mislead the next run.
        assert!(from.join("instances").join("a").join("file").exists());
        assert!(
            std::fs::symlink_metadata(from.join("instances").join("a").join("z-borrowed"))
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
        );
        assert!(
            same_dir(
                &std::fs::read_link(inst.join("a-deps")).unwrap(),
                &from.join("versions").join("v1").join("node_modules"),
            ),
            "the already-classified managed link was NOT re-pointed by the aborted move"
        );
        assert_eq!(
            std::fs::read(inst.join("a-deps").join("dep").join("index.js")).unwrap(),
            b"shared-gold",
            "and it still resolves"
        );
        assert!(!to.join("instances").exists(), "no stranded half-move");
    }

    #[tokio::test]
    async fn dotdot_alias_cannot_move_a_root_into_itself() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        std::fs::create_dir_all(from.join("instances")).unwrap();
        std::fs::create_dir_all(root.0.join("alias")).unwrap();
        let to = root.0.join("alias/../old/nested");
        assert!(
            move_with_journal(&no_cancel(), &root.0.join(JOURNAL_NAME), &from, &to)
                .await
                .is_err()
        );
        assert!(from.join("instances").exists());
    }

    #[tokio::test]
    async fn cancellation_before_moving_preserves_config() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        std::fs::create_dir_all(from.join("config")).unwrap();
        let result = move_with_journal(
            &Arc::new(AtomicBool::new(true)),
            &root.0.join(JOURNAL_NAME),
            &from,
            &root.0.join("new"),
        )
        .await
        .unwrap();
        assert!(result.cancelled);
        assert!(result.moved.is_empty());
        assert!(from.join("config").exists());
    }

    /// The restart-recovery promise: cancel mid-way, boot again, and the
    /// journal — read exactly like `storage_migration_status` does — says
    /// which directories already arrived. A resume re-runs the same call and
    /// skips them; only the untouched ones move.
    #[tokio::test]
    async fn cancelled_migration_resumes_from_the_journal() {
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("instances")).unwrap();
        std::fs::write(from.join("instances/a.txt"), b"ia").unwrap();
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::write(from.join("config/api.json"), b"cfg").unwrap();

        // Run one: instances moves first; cancel arrives before config.
        let flag = no_cancel();
        let to2 = to.clone();
        let from2 = from.clone();
        let runner = {
            let flag_in = flag.clone();
            let journal = journal.clone();
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let h = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(move_root_inner(
                    &flag_in,
                    &null_task(),
                    Some(&journal),
                    &from2,
                    &to2,
                    &|p| {
                        if p.kind == "instances" && p.progress >= 1.0 {
                            tx.send(()).unwrap();
                        }
                    },
                ))
            });
            rx.recv().unwrap();
            flag.store(true, Ordering::SeqCst);
            h.join().unwrap().unwrap()
        };
        assert!(runner.cancelled);
        assert_eq!(runner.moved, vec!["instances"]);
        assert!(to.join("instances/a.txt").exists());
        assert!(from.join("config/api.json").exists());

        // Boot: the recovery entry sees an uncommitted journal...
        let open = read_journal(&journal).unwrap().unwrap();
        assert!(!open.committed);
        assert_eq!(open.entry("instances"), EntryState::Moved);
        assert_eq!(open.entry("config"), EntryState::Pending);

        // ...and a resume (new flag, same call) finishes without re-moving.
        let result = move_with_journal(&no_cancel(), &journal, &from, &to)
            .await
            .unwrap();
        assert!(!result.cancelled);
        assert!(result.moved.contains(&"config".to_string()));
        assert!(to.join("config/api.json").exists());
        assert!(!from.join("config").exists());
        let done = read_journal(&journal).unwrap().unwrap();
        assert!(done.committed);
    }

    #[tokio::test]
    async fn open_journal_for_a_different_pair_blocks_a_new_migration() {
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let a_from = root.0.join("a-from");
        let a_to = root.0.join("a-to");
        std::fs::create_dir_all(a_from.join("config")).unwrap();
        let _ = move_with_journal(&Arc::new(AtomicBool::new(true)), &journal, &a_from, &a_to).await; // cancelled run 1: journal is open
        let b_from = root.0.join("b-from");
        let b_to = root.0.join("b-to");
        std::fs::create_dir_all(b_from.join("config")).unwrap();
        let err = move_with_journal(&no_cancel(), &journal, &b_from, &b_to)
            .await
            .unwrap_err();
        assert!(err.contains("未完成的迁移"), "{err}");
        assert!(b_from.join("config").exists());
    }

    #[tokio::test]
    async fn undo_returns_moved_data_and_clears_the_journal() {
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("instances")).unwrap();
        std::fs::write(from.join("instances/a.txt"), b"ia").unwrap();
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::write(from.join("config/api.json"), b"cfg").unwrap();
        // Cancel after the first directory arrived.
        let flag = no_cancel();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let h = {
            let flag2 = flag.clone();
            let (from, to, journal) = (from.clone(), to.clone(), journal.clone());
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                rt.block_on(move_root_inner(
                    &flag2,
                    &null_task(),
                    Some(&journal),
                    &from,
                    &to,
                    &|p| {
                        if p.kind == "instances" && p.progress >= 1.0 {
                            tx.send(()).unwrap();
                        }
                    },
                ))
            })
        };
        rx.recv().unwrap();
        flag.store(true, Ordering::SeqCst);
        h.join().unwrap().unwrap();
        assert!(to.join("instances/a.txt").exists());
        assert!(from.join("config/api.json").exists());

        let summary = undo_migration(
            &journal,
            &read_journal(&journal).unwrap().unwrap(),
            &null_task(),
        )
        .await
        .unwrap();
        assert_eq!(summary.moved, vec!["instances"]);
        assert!(from.join("instances/a.txt").exists(), "data walks back");
        assert!(!to.join("instances").exists());
        assert!(read_journal(&journal).unwrap().is_none(), "journal cleared");
    }

    #[tokio::test]
    async fn undo_translates_managed_links_back_to_the_source_root() {
        // The forward pass re-points every managed link at `to`, so undoing the
        // move has to translate them again. The state is fabricated rather than
        // raced for: `instances` arrived and was re-pointed, `versions` never
        // moved — so the link's target does not exist, and canonical matching
        // cannot even read it (the `Raw` mode is what makes undo possible).
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let from = root.0.join("old");
        let to = root.0.join("new");
        let dep = from
            .join("versions")
            .join("v1")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(dep.join("index.js"), b"shared-gold").unwrap();

        // What the rename + repoint left behind in the destination root.
        let migrated = to
            .join("instances")
            .join("a")
            .join("dsh-home")
            .join("profiles");
        std::fs::create_dir_all(&migrated).unwrap();
        crate::instances::copy::recreate_link(
            &migrated.join("node_modules"),
            &to.join("versions").join("v1").join("node_modules"),
            true,
        )
        .unwrap();
        assert!(
            std::fs::metadata(to.join("versions")).is_err(),
            "the new versions tree never arrived"
        );

        let mut open = MigrationJournal::fresh(&from, &to);
        open.set("instances", EntryState::Moved, 0);
        open.save(&journal).unwrap();

        let summary = undo_migration(
            &journal,
            &read_journal(&journal).unwrap().unwrap(),
            &null_task(),
        )
        .await
        .unwrap();
        assert_eq!(summary.moved, vec!["instances"]);

        let restored = from
            .join("instances")
            .join("a")
            .join("dsh-home")
            .join("profiles")
            .join("node_modules");
        assert!(
            same_dir(
                &std::fs::read_link(&restored).unwrap(),
                &from.join("versions").join("v1").join("node_modules"),
            ),
            "the restored link points at the source root, not the emptied one"
        );
        assert_eq!(
            std::fs::read(restored.join("dep").join("index.js")).unwrap(),
            b"shared-gold",
            "and it resolves to the real content again"
        );
        assert!(read_journal(&journal).unwrap().is_none(), "journal cleared");
    }

    #[tokio::test]
    async fn interrupted_moving_row_is_redone_cleanly() {
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::write(from.join("config/api.json"), b"cfg").unwrap();
        // Fabricate a crash mid-copy: journal says `moving`, staging has
        // a half tree, but no final destination. Rows store canonical
        // paths — that is what a resumed run compares against.
        std::fs::create_dir_all(&to).unwrap();
        let from = std::fs::canonicalize(&from).unwrap();
        let to = std::fs::canonicalize(&to).unwrap();
        let mut j = MigrationJournal::fresh(&from, &to);
        j.set("config", EntryState::Moving, 0);
        std::fs::create_dir_all(to.join(STAGING_DIR).join("config")).unwrap();
        std::fs::write(to.join(STAGING_DIR).join("config/partial"), b"x").unwrap();
        j.save(&journal).unwrap();

        let result = move_with_journal(&no_cancel(), &journal, &from, &to)
            .await
            .unwrap();
        assert!(!result.cancelled);
        assert!(result.moved.contains(&"config".to_string()));
        assert!(
            to.join("config/api.json").exists(),
            "the redone copy landed"
        );
        assert!(
            !to.join("config/partial").exists(),
            "the half tree was discarded, not merged into"
        );
        assert!(!to.join(STAGING_DIR).exists());
    }

    #[tokio::test]
    async fn interrupted_landing_never_deletes_the_only_complete_copy() {
        // After rename the source is absent; during cross-drive source
        // removal it may still exist but contain only part of the data.
        for source_remains in [false, true] {
            let root = TestRoot::new();
            let journal_path = root.0.join(JOURNAL_NAME);
            let from = root.0.join("old");
            let to = root.0.join("new");
            std::fs::create_dir_all(from.join("instances")).unwrap();
            std::fs::write(from.join("instances/untouched"), b"earlier-row").unwrap();
            std::fs::create_dir_all(to.join("config")).unwrap();
            std::fs::write(to.join("config/complete"), b"only-complete-copy").unwrap();
            if source_remains {
                std::fs::create_dir_all(from.join("config")).unwrap();
                std::fs::write(from.join("config/remnant"), b"partial-source").unwrap();
            }
            let from = std::fs::canonicalize(from).unwrap();
            let to = std::fs::canonicalize(to).unwrap();
            let mut journal = MigrationJournal::fresh(&from, &to);
            journal.set("config", EntryState::Moving, 0);
            journal.save(&journal_path).unwrap();
            let saved = std::fs::read(&journal_path).unwrap();

            assert!(move_with_journal(&no_cancel(), &journal_path, &from, &to)
                .await
                .unwrap_err()
                .contains("已保留"));
            assert!(undo_migration(&journal_path, &journal, &null_task())
                .await
                .is_err());
            assert_eq!(
                std::fs::read(to.join("config/complete")).unwrap(),
                b"only-complete-copy"
            );
            assert_eq!(
                std::fs::read(from.join("instances/untouched")).unwrap(),
                b"earlier-row"
            );
            assert!(!to.join("instances").exists(), "preflight before any move");
            assert_eq!(std::fs::read(&journal_path).unwrap(), saved);
            if source_remains {
                assert_eq!(
                    std::fs::read(from.join("config/remnant")).unwrap(),
                    b"partial-source"
                );
            }
        }
    }

    #[tokio::test]
    async fn copy_back_restores_a_directory_and_translates_its_managed_links() {
        // The cross-drive undo path: no rename exists, so the directory is
        // copied back through staging exactly like the forward pass copied it
        // out. The link must follow the data, not the root being emptied.
        use crate::instances::copy::recreate_link;
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        let shared = to.join("versions").join("v1").join("node_modules");
        std::fs::create_dir_all(shared.join("dep")).unwrap();
        std::fs::write(shared.join("dep/index.js"), b"shared-module").unwrap();
        let inst = to.join("instances").join("a").join("dsh-home");
        std::fs::create_dir_all(&inst).unwrap();
        std::fs::write(inst.join("home.json"), b"home").unwrap();
        recreate_link(&inst.join("node_modules"), &shared, true).unwrap();
        // The root the data returns to owns its own shared install, so the
        // translated link has a real target there.
        std::fs::create_dir_all(
            from.join("versions")
                .join("v1")
                .join("node_modules")
                .join("dep"),
        )
        .unwrap();
        std::fs::write(
            from.join("versions/v1/node_modules/dep/index.js"),
            b"shared-module",
        )
        .unwrap();
        let from = std::fs::canonicalize(&from).unwrap();
        let to = std::fs::canonicalize(&to).unwrap();

        copy_back(
            "instances",
            &to.join("instances"),
            &from.join("instances"),
            &from,
            &to,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(from.join("instances/a/dsh-home/home.json")).unwrap(),
            b"home"
        );
        assert!(
            !to.join("instances").exists(),
            "the destination copy is deleted only after the restore is verified"
        );
        let link = from.join("instances/a/dsh-home/node_modules");
        assert!(
            std::fs::symlink_metadata(&link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "the link survived as a link — nothing materialized"
        );
        assert_eq!(
            std::fs::read(link.join("dep/index.js")).unwrap(),
            b"shared-module",
            "reads resolve through the translated link into the restored root"
        );
        let raw_target = crate::paths::strip_verbatim(&std::fs::read_link(&link).unwrap());
        let expected = crate::paths::strip_verbatim(&from)
            .join("versions")
            .join("v1")
            .join("node_modules");
        assert_eq!(
            raw_target, expected,
            "the link was translated to the root it was restored into"
        );
        assert!(
            !from.join(STAGING_DIR).exists() && !to.join(STAGING_DIR).exists(),
            "no staging residue on either side"
        );
    }

    #[tokio::test]
    async fn undo_returns_the_data_when_the_two_roots_sit_on_different_volumes() {
        // Blocker #3 of the alpha acceptance: a migration that crossed volumes
        // has no rename to undo. Only a real second volume proves it, so this
        // test skips (loudly) where there is one drive; CI runners have one.
        use crate::instances::copy::recreate_link;
        let Some(other) = OtherVolume::new() else {
            eprintln!("[phl] 只有一个卷，跳过跨盘撤销测试；跨盘分支由 copy_back 单元测试覆盖");
            return;
        };
        let temp = TestRoot::new();
        let from = other.0.join("old");
        let to = temp.0.join("new");
        std::fs::create_dir_all(&to).unwrap();
        // Precondition, stated rather than assumed: a rename across these two
        // roots must genuinely fail, otherwise this test proves nothing.
        let probe = to.join("probe");
        std::fs::write(&probe, b"x").unwrap();
        if std::fs::rename(&probe, other.0.join("probe")).is_ok() {
            eprintln!("[phl] 两个根之间可以重命名，跳过跨盘撤销测试");
            let _ = std::fs::rename(other.0.join("probe"), &probe);
            return;
        }
        std::fs::remove_file(&probe).unwrap();

        // The forward pass left the data at `to` and the journal says so.
        let shared = to.join("versions").join("v1").join("node_modules");
        std::fs::create_dir_all(shared.join("dep")).unwrap();
        std::fs::write(shared.join("dep/index.js"), b"shared-module").unwrap();
        let home = to.join("instances").join("a").join("dsh-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("home.json"), b"home").unwrap();
        recreate_link(&home.join("node_modules"), &shared, true).unwrap();
        // The original root owns its own shared install, so the translated
        // link lands on a real target.
        std::fs::create_dir_all(
            from.join("versions")
                .join("v1")
                .join("node_modules")
                .join("dep"),
        )
        .unwrap();
        std::fs::write(
            from.join("versions/v1/node_modules/dep/index.js"),
            b"shared-module",
        )
        .unwrap();
        let from = std::fs::canonicalize(&from).unwrap();
        let to = std::fs::canonicalize(&to).unwrap();
        let mut journal = MigrationJournal::fresh(&from, &to);
        journal.set("instances", EntryState::Moved, 4);
        let path = to.join(JOURNAL_NAME);
        journal.save(&path).unwrap();

        let summary = undo_migration(&path, &journal, &null_task()).await.unwrap();

        assert_eq!(summary.moved, vec!["instances".to_string()]);
        assert!(summary.cancelled, "undo reports a cancellation, not a move");
        assert_eq!(
            std::fs::read(from.join("instances/a/dsh-home/home.json")).unwrap(),
            b"home",
            "the data is back on its original volume"
        );
        assert!(
            !to.join("instances").exists(),
            "the destination copy was deleted after the restore was verified"
        );
        let link = from.join("instances/a/dsh-home/node_modules");
        assert_eq!(
            std::fs::read(link.join("dep/index.js")).unwrap(),
            b"shared-module",
            "reads resolve through the translated link on the restored root"
        );
        let raw_target = crate::paths::strip_verbatim(&std::fs::read_link(&link).unwrap());
        let expected = crate::paths::strip_verbatim(&from)
            .join("versions")
            .join("v1")
            .join("node_modules");
        assert_eq!(raw_target, expected);
        assert!(!path.exists(), "the journal is cleared by a completed undo");
        assert!(
            !from.join(STAGING_DIR).exists() && !to.join(STAGING_DIR).exists(),
            "no staging residue on either side"
        );
    }

    #[tokio::test]
    async fn undo_preserves_verified_destination_when_source_deletion_was_partial() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::create_dir_all(to.join("config")).unwrap();
        std::fs::write(from.join("config/remnant"), b"partial").unwrap();
        std::fs::write(to.join("config/complete"), b"complete").unwrap();
        let mut journal = MigrationJournal::fresh(&from, &to);
        journal.set("config", EntryState::Moved, 8);
        let path = root.0.join(JOURNAL_NAME);
        journal.save(&path).unwrap();
        assert!(undo_migration(&path, &journal, &null_task()).await.is_err());
        assert_eq!(
            std::fs::read(to.join("config/complete")).unwrap(),
            b"complete"
        );
        assert_eq!(
            std::fs::read(from.join("config/remnant")).unwrap(),
            b"partial"
        );
        assert!(path.exists());
    }

    #[tokio::test]
    async fn undo_does_not_delete_a_pending_destination_conflict() {
        let root = TestRoot::new();
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::create_dir_all(to.join("config")).unwrap();
        std::fs::write(to.join("config/other-user-data"), b"not-owned").unwrap();
        let journal = MigrationJournal::fresh(&from, &to);
        let path = root.0.join(JOURNAL_NAME);
        journal.save(&path).unwrap();
        undo_migration(&path, &journal, &null_task()).await.unwrap();
        assert_eq!(
            std::fs::read(to.join("config/other-user-data")).unwrap(),
            b"not-owned"
        );
    }

    #[tokio::test]
    async fn committed_journal_left_behind_reads_as_finished() {
        let root = TestRoot::new();
        let journal = root.0.join(JOURNAL_NAME);
        let from = root.0.join("old");
        let to = root.0.join("new");
        std::fs::create_dir_all(from.join("config")).unwrap();
        std::fs::write(from.join("config/api.json"), b"cfg").unwrap();
        let mut j = MigrationJournal::fresh(&from, &to);
        j.set("config", EntryState::Moved, 3);
        j.committed = true;
        j.save(&journal).unwrap();
        let result = move_with_journal(&no_cancel(), &journal, &from, &to)
            .await
            .unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.moved, vec!["config"]);
        assert_eq!(result.bytes, 3);
    }

    #[tokio::test]
    async fn stale_committed_journal_cannot_report_a_different_move_as_success() {
        let root = TestRoot::new();
        let a = root.0.join("a");
        let b = root.0.join("b");
        let c = root.0.join("c");
        let journal = root.0.join(JOURNAL_NAME);
        std::fs::create_dir_all(a.join("config")).unwrap();
        std::fs::write(a.join("config/sentinel"), b"preserved").unwrap();
        move_with_journal(&no_cancel(), &journal, &a, &b)
            .await
            .unwrap();
        let result = move_with_journal(&no_cancel(), &journal, &b, &c).await;
        assert!(
            result.is_err(),
            "a stale A -> B journal must not report B -> C as complete"
        );
        assert!(b.join("config/sentinel").exists());
        assert!(!c.join("config/sentinel").exists());
        assert!(journal.exists());
    }

    #[tokio::test]
    async fn a_finished_root_can_retire_its_journal_and_really_move_again() {
        let root = TestRoot::new();
        let a = root.0.join("a");
        let b = root.0.join("b");
        let c = root.0.join("c");
        let journal = root.0.join(JOURNAL_NAME);
        let state = PhlState::with_pointer(Some(root.0.join("root.json")));
        std::fs::create_dir_all(a.join("config")).unwrap();
        std::fs::write(a.join("config/sentinel"), b"preserved").unwrap();
        move_with_journal(&no_cancel(), &journal, &a, &b)
            .await
            .unwrap();
        // Commit the pointer while retaining the journal, as after a cleanup failure.
        state.relocate_root(b.clone(), None).unwrap();
        retire_completed_migration(&state, &journal, &b, &c).unwrap();
        assert!(!journal.exists());
        let summary = move_with_journal(&no_cancel(), &journal, &b, &c)
            .await
            .unwrap();
        let outcome = complete_move(&state, Some(&journal), &c, summary).unwrap();
        assert!(same_location(Path::new(outcome.root.as_ref().unwrap()), &c));
        assert!(same_location(&state.root(), &c));
        assert_eq!(
            std::fs::read(c.join("config/sentinel")).unwrap(),
            b"preserved"
        );
        assert!(!b.join("config").exists());
        assert!(!journal.exists());
    }

    #[tokio::test]
    async fn an_unadopted_completed_journal_cannot_be_retired_for_another_move() {
        let root = TestRoot::new();
        let a = root.0.join("a");
        let b = root.0.join("b");
        let c = root.0.join("c");
        let journal = root.0.join(JOURNAL_NAME);
        let state = PhlState::with_pointer(Some(root.0.join("root.json")));
        std::fs::create_dir_all(a.join("config")).unwrap();
        state.relocate_root(a.clone(), None).unwrap();
        move_with_journal(&no_cancel(), &journal, &a, &b)
            .await
            .unwrap();
        retire_completed_migration(&state, &journal, &b, &c).unwrap();
        assert!(
            journal.exists(),
            "pointer must be adopted before retiring its recovery entry"
        );
        assert!(move_with_journal(&no_cancel(), &journal, &b, &c)
            .await
            .is_err());
        let summary = MoveSummary {
            moved: vec![],
            bytes: 0,
            cancelled: false,
        };
        assert!(complete_move(&state, Some(&journal), &c, summary).is_err());
        assert!(same_location(&state.root(), &a));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn locked_completed_journal_blocks_the_next_move_without_losing_data() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = TestRoot::new();
        let a = root.0.join("a");
        let b = root.0.join("b");
        let c = root.0.join("c");
        let journal = root.0.join(JOURNAL_NAME);
        let state = PhlState::with_pointer(Some(root.0.join("root.json")));
        std::fs::create_dir_all(a.join("config")).unwrap();
        std::fs::write(a.join("config/sentinel"), b"preserved").unwrap();
        move_with_journal(&no_cancel(), &journal, &a, &b)
            .await
            .unwrap();
        state.relocate_root(b.clone(), None).unwrap();
        // Permit reads, deny delete-sharing: reproduce journal cleanup failure.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&journal)
            .unwrap();
        let err = retire_completed_migration(&state, &journal, &b, &c).unwrap_err();
        assert!(err.contains("迁移记录无法清理"));
        assert!(b.join("config/sentinel").exists());
        assert!(!c.exists());
        assert!(journal.exists());
        drop(held);
        retire_completed_migration(&state, &journal, &b, &c).unwrap();
        assert!(!journal.exists());
    }

    #[test]
    fn a_cancelled_move_does_not_return_or_commit_a_new_root() {
        let root = TestRoot::new();
        let pointer = root.0.join("root.json");
        let state = PhlState::with_pointer(Some(pointer.clone()));
        let summary = MoveSummary {
            moved: vec![],
            bytes: 0,
            cancelled: true,
        };
        let outcome = complete_move(&state, None, &root.0.join("missing"), summary).unwrap();
        assert!(outcome.root.is_none());
        assert!(!pointer.exists());
    }

    /* ------------------------- R1: pointer + journal ------------------------- */

    /// The relocation commit is one operation: the pointer names the new
    /// root *and* the journal goes, so the state "data arrived, journal
    /// deleted, pointer still old" — which used to be the wrapper's deletion
    /// order and had no recovery entry — cannot be produced.
    #[test]
    fn relocate_root_moves_the_pointer_and_clears_the_journal_together() {
        let dir = TestRoot::new();
        let pointer = dir.0.join("state").join("root.json");
        let journal_path = dir.0.join(JOURNAL_NAME);
        let mut j = MigrationJournal::fresh(Path::new("C:\\old-root"), Path::new("C:\\new-root"));
        j.set("config", EntryState::Moved, 1);
        j.committed = true;
        j.save(&journal_path).unwrap();

        let state = PhlState::with_pointer(Some(pointer.clone()));
        let adopted = state
            .relocate_root(PathBuf::from(&j.to), Some(&journal_path))
            .unwrap();
        assert_eq!(adopted, PathBuf::from("C:\\new-root"));
        assert_eq!(
            std::fs::read_to_string(&pointer).unwrap(),
            "C:\\new-root",
            "the persisted pointer names the new root"
        );
        assert!(
            !journal_path.exists(),
            "the journal is cleared in the same commit"
        );
        let rebooted = PhlState::with_pointer(Some(pointer));
        assert_eq!(
            rebooted.root(),
            PathBuf::from("C:\\new-root"),
            "the next boot reads the new root"
        );
    }

    /// When the commit fails, the order inside `relocate_root` matters: the
    /// journal must SURVIVE an un-writable pointer — it is the recovery
    /// entry (`storage_migration_finish`) for exactly this state.
    #[test]
    fn a_failed_pointer_commit_keeps_the_journal_and_the_old_root() {
        let dir = TestRoot::new();
        // A pointer whose parent is a *file* makes every write fail.
        let blocker = dir.0.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let journal_path = dir.0.join(JOURNAL_NAME);
        let mut j = MigrationJournal::fresh(Path::new("C:\\old-root"), Path::new("C:\\new-root"));
        j.committed = true;
        j.save(&journal_path).unwrap();

        let state = PhlState::with_pointer(Some(blocker.join("root.json")));
        assert!(state
            .relocate_root(PathBuf::from(&j.to), Some(&journal_path))
            .is_err());
        assert!(
            journal_path.exists(),
            "the journal survives the failed commit — the restart keeps its entry"
        );
        assert_eq!(
            state.root(),
            crate::paths::default_root(),
            "the in-memory root did not move on a failed commit"
        );
    }

    #[test]
    fn finish_only_adopts_a_committed_journal() {
        let dir = TestRoot::new();
        let pointer = dir.0.join("root.json");
        let journal_path = dir.0.join(JOURNAL_NAME);

        // An unfinished journal is refused, pointer untouched, journal kept.
        let state = PhlState::with_pointer(Some(pointer.clone()));
        let mut open =
            MigrationJournal::fresh(Path::new("C:\\old-root"), Path::new("C:\\new-root"));
        open.set("config", EntryState::Moved, 1);
        open.save(&journal_path).unwrap();
        assert!(finish_committed_migration(&state, &journal_path).is_err());
        assert_eq!(state.root(), crate::paths::default_root());
        assert!(journal_path.exists());

        // A committed one adopts the journal's `to`, whatever the UI thinks.
        let mut done = open.clone();
        done.committed = true;
        done.save(&journal_path).unwrap();
        let adopted = finish_committed_migration(&state, &journal_path).unwrap();
        assert_eq!(adopted, "C:\\new-root");
        assert_eq!(state.root(), PathBuf::from("C:\\new-root"));
        assert!(!journal_path.exists(), "finishing clears the journal");

        // No journal left behind means there is nothing to confirm.
        assert!(finish_committed_migration(&state, &journal_path).is_err());
    }
}
