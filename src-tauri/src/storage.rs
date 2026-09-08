//! Storage-level operations on the whole data root: free-space queries and
//! moving the root's data between drives. Like every other command, the root
//! is always an explicit argument — Rust keeps no opinion about where PHL
//! lives, so "moving" is copying trees plus the frontend re-pointing its
//! per-call root afterwards.
//!
//! A migration is resumable, not a one-shot gamble. A journal file lives
//! beside the root pointer (outside both roots, so neither the old nor the
//! new location can swallow it) and records every directory: pending →
//! moving → moved, with byte counts. Cancellation or a crash between those
//! rows is the normal case the design assumes: a restart finds the journal,
//! shows a resume/undo entry, and either re-runs from where the record says
//! or rolls the half-copied directories back. The root pointer is only ever
//! re-pointed after a fully committed migration.

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

/* ------------------------------ journal ------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Not started (or a leftover `moving` treated as not started).
    Pending,
    /// Copy/rename was in flight when the operation stopped. Resume re-does
    /// this directory from scratch; undo deletes whatever it managed to place.
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
        move |_task| async move {
            let journal =
                read_journal(&path)?.ok_or_else(|| "迁移记录不存在，无法撤销".to_string())?;
            undo_migration(&path, &journal).await
        },
    )
    .await
}

/// Roll a non-committed migration back: everything that arrived in `to`
/// (moved and half-copied alike) is walked back to `from`, then the journal
/// is deleted. Data that never left the source stays untouched.
async fn undo_migration(path: &Path, journal: &MigrationJournal) -> Result<MoveSummary, String> {
    if journal.committed {
        return Err("迁移已完成提交，不能整体撤销".into());
    }
    let from = PathBuf::from(&journal.from);
    let to = PathBuf::from(&journal.to);
    let mut returned = Vec::new();
    let mut bytes = 0u64;
    for e in &journal.entries {
        let src = to.join(&e.kind);
        let back = from.join(&e.kind);
        if src.exists() {
            if back.exists() {
                // A moved+source-stuck entry has both sides; the destination
                // copy is the duplicate the migration itself produced.
                tokio::fs::remove_dir_all(&src)
                    .await
                    .map_err(|err| format!("无法清理目标残留 {}: {err}", src.display()))?;
            } else {
                tokio::fs::create_dir_all(&from)
                    .await
                    .map_err(|err| err.to_string())?;
                tokio::fs::rename(&src, &back).await.map_err(|err| {
                    format!("无法把 {} 搬回 {}: {err}", src.display(), back.display())
                })?;
                // The forward pass re-pointed every managed link at `to`, so
                // rolling the move back has to translate them again — the
                // restored tree would otherwise reference a root that is being
                // emptied. `Raw` matching, because `to/versions` may never have
                // moved and therefore cannot be canonicalized.
                if let Err(err) =
                    crate::instances::repoint_managed_links(&back, &to, &from, LinkMatch::Raw)
                {
                    let _ = tokio::fs::rename(&back, &src).await;
                    return Err(format!(
                        "无法还原 {} 中的链接: {err}（该目录已放回目标根）",
                        back.display()
                    ));
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
) -> Result<MoveSummary, String> {
    let flag = transfers.take(&transfer_id);
    let journal_path = phl.sibling_file(JOURNAL_NAME);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "root-migration",
        format!("迁移数据目录 → {to}"),
        vec![crate::resources::Resource::DataRoot],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
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
            // A fully committed migration has served its purpose; only
            // unfinished ones need to survive into the next boot.
            if r.as_ref().is_ok_and(|s| !s.cancelled) {
                if let Some(ref p) = journal_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
            }
            r
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
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
        Some(j) if j.committed => {
            let (moved, bytes) = j.moved_kinds();
            return Ok(MoveSummary {
                moved,
                bytes,
                cancelled: false,
            });
        }
        Some(j) if j.from == from.to_string_lossy() && j.to == to.to_string_lossy() => j,
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
        if journal.entry(kind) == EntryState::Moved {
            continue;
        }
        // A `moving` row is *expected* to own a half tree at the destination —
        // the copy loop below discards it and redoes the directory. Refusing
        // it here would deadlock the recovery this journal exists to enable.
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
        // A leftover `moving` row means an earlier attempt was interrupted:
        // discard its partial landing (destination copy and staging) before
        // redoing the directory from scratch.
        if journal.entry(kind) == EntryState::Moving {
            let _ = tokio::fs::remove_dir_all(to.join(STAGING_DIR).join(kind)).await;
            let _ = tokio::fs::remove_dir_all(to.join(kind)).await;
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
        let bytes = dir_size(&src);

        // Fast path, same drive: rename. The journal row already says
        // `moving`, so an interrupt here is recovered the same way as a
        // half-finished copy.
        if tokio::fs::rename(&src, &dst).await.is_ok() {
            // Rename preserves links verbatim — including absolute targets
            // into the OLD root that the deletion step will erase. Re-point
            // them before declaring the kind moved (and refuse + roll the
            // rename back on anything the copy path would refuse).
            if let Err(e) =
                crate::instances::repoint_managed_links(&dst, &from, &to, LinkMatch::Canonical)
            {
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
            // `versions/` moves with the root, so a link is re-pointed at the
            // same relative location under the destination instead of at the
            // old absolute prefix, which dangles once the source is deleted.
            LinkPolicy::Rewrite {
                new_root: to.clone(),
                match_on: LinkMatch::Canonical,
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
        let staged = dir_size(&stage);
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
        assert_eq!(
            crate::paths::strip_verbatim(&std::fs::read_link(inst.join("a-deps")).unwrap()),
            crate::paths::strip_verbatim(&std::fs::canonicalize(&from).unwrap())
                .join("versions")
                .join("v1")
                .join("node_modules"),
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

        let summary = undo_migration(&journal, &read_journal(&journal).unwrap().unwrap())
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

        let summary = undo_migration(&journal, &read_journal(&journal).unwrap().unwrap())
            .await
            .unwrap();
        assert_eq!(summary.moved, vec!["instances"]);

        let restored = from
            .join("instances")
            .join("a")
            .join("dsh-home")
            .join("profiles")
            .join("node_modules");
        assert_eq!(
            crate::paths::strip_verbatim(&std::fs::read_link(&restored).unwrap()),
            crate::paths::strip_verbatim(&std::fs::canonicalize(&from).unwrap())
                .join("versions")
                .join("v1")
                .join("node_modules"),
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
        // Fabricate a crash mid-copy: journal says `moving`, destination has
        // a half tree, and a staging leftover exists. Rows store canonical
        // paths — that is what a resumed run compares against.
        std::fs::create_dir_all(&to).unwrap();
        let from = std::fs::canonicalize(&from).unwrap();
        let to = std::fs::canonicalize(&to).unwrap();
        let mut j = MigrationJournal::fresh(&from, &to);
        j.set("config", EntryState::Moving, 0);
        std::fs::create_dir_all(to.join("config")).unwrap();
        std::fs::write(to.join("config").join("partial"), b"x").unwrap();
        std::fs::create_dir_all(to.join(STAGING_DIR).join("config")).unwrap();
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
}
