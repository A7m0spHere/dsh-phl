//! Copy-first restore transactions. The original home stays complete until
//! promotion, and its backup is retired only when promotion is unambiguous.

use super::*;

const STAGING: &str = ".phl-restore";
const BACKUP: &str = ".phl-old-dsh-home";
const JOURNAL: &str = "transaction.json";
const LOCK: &str = ".phl-restore.lock";
const KEEP: [&str; 2] = ["sessions", "attachments"];

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Copying,
    Ready,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    instance_id: String,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    SnapshotCopied,
    SessionsCopied,
    CopyAttachments,
    AttachmentsCopied,
    Backup,
    BackedUp,
    Promote,
    Promoted,
    Rollback,
    Cleanup,
}

fn uncertain(dir: &Path, reason: impl std::fmt::Display) -> String {
    format!(
        "快照还原状态无法安全确认：{reason}；已保留 {}，请勿删除暂存或备份目录",
        dir.display()
    )
}

/// `exists()` would mistake access errors for absence. Never follow a link
/// at a transaction boundary, including Windows junctions/reparse points.
async fn plain_dir(path: &Path) -> Result<bool, String> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => {
            if is_link(&meta) || !meta.is_dir() {
                return Err(uncertain(path, "事务路径不是普通目录"));
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(uncertain(path, e)),
    }
}

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Keep the inode/file name after unlocking: unlinking a lock file would
/// allow two processes to lock different files under the same name.
pub(super) async fn lock(dir: &Path) -> Result<std::fs::File, String> {
    if !plain_dir(dir).await? {
        return Err(uncertain(dir, "实例目录不存在"));
    }
    let path = dir.join(LOCK);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(meta) if !meta.is_file() || is_link(&meta) => {
            return Err(uncertain(dir, "事务锁不是普通文件"));
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(uncertain(dir, e)),
        _ => {}
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| uncertain(dir, e))?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|e| format!("无法取得快照事务独占锁，请等待其他 PHL 操作结束：{e}"))?;
    Ok(file)
}

pub(super) async fn pending(dir: &Path) -> Result<bool, String> {
    if !plain_dir(dir).await? {
        return Ok(false);
    }
    // Inspect both, even when staging exists: neither may hide a bad link.
    let staging = plain_dir(&dir.join(STAGING)).await?;
    let backup = plain_dir(&dir.join(BACKUP)).await?;
    Ok(staging || backup)
}

async fn read_journal(staging: &Path, id: &str) -> Result<Option<Journal>, String> {
    let path = staging.join(JOURNAL);
    let meta = match tokio::fs::symlink_metadata(&path).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(uncertain(staging, e)),
    };
    if !meta.is_file() || is_link(&meta) || meta.len() > 8192 {
        return Err(uncertain(staging, "事务记录不是普通文件"));
    }
    let raw = tokio::fs::read(&path)
        .await
        .map_err(|e| uncertain(staging, e))?;
    let journal: Journal = serde_json::from_slice(&raw).map_err(|e| uncertain(staging, e))?;
    if journal.version != 1 || journal.instance_id != id {
        return Err(uncertain(staging, "事务记录版本或实例身份不匹配"));
    }
    Ok(Some(journal))
}

fn write_journal(dir: &Path, id: &str, phase: Phase) -> Result<(), String> {
    let journal = Journal {
        version: 1,
        instance_id: id.into(),
        phase,
    };
    let bytes = serde_json::to_vec(&journal).map_err(|e| e.to_string())?;
    crate::paths::write_durable(&dir.join(STAGING).join(JOURNAL), &bytes)
}

async fn remove_owned(path: &Path) -> Result<(), String> {
    if plain_dir(path).await? {
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|e| uncertain(path, format!("清理失败：{e}")))?;
    }
    Ok(())
}

pub(super) async fn recover(dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    if !pending(dir).await? {
        return Ok(());
    }
    let _lock = lock(dir).await?;
    recover_locked(dir, manifest).await
}

pub(super) async fn recover_locked(dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    if !pending(dir).await? {
        return Ok(());
    }
    reject_external_write(manifest, &manifest.id, "恢复中断的快照事务")?;
    let current = home_of(dir, manifest);
    let staging = dir.join(STAGING);
    let backup = dir.join(BACKUP);
    let has_current = plain_dir(&current).await?;
    let has_backup = plain_dir(&backup).await?;
    let staged_home = staging.join("dsh-home");
    let has_staged_home = plain_dir(&staged_home).await?;
    if let Some(journal) = read_journal(&staging, &manifest.id).await? {
        match (journal.phase, has_current, has_backup, has_staged_home) {
            // No swap started: every staged byte is a copy, not a sole copy.
            (Phase::Copying | Phase::Ready, true, false, _) => remove_owned(&staging).await,
            // Crash after live -> backup. Roll back the complete original.
            (Phase::Ready, false, true, true) => {
                tokio::fs::rename(&backup, &current)
                    .await
                    .map_err(|e| uncertain(dir, format!("回滚失败：{e}")))?;
                remove_owned(&staging).await
            }
            // Promotion consumed staged_home. Never roll this current home
            // back: DSH may have written newer messages since promotion.
            (Phase::Ready, true, true, false) => {
                remove_owned(&backup).await?;
                remove_owned(&staging).await
            }
            _ => Err(uncertain(dir, "事务记录与目录布局矛盾")),
        }
    } else {
        recover_legacy(dir, &current, &staging, &backup, has_current, has_backup).await
    }
}

/// Older releases MOVED live conversations into staging. Return those before
/// removing anything. Conflicting copies are preserved for manual recovery.
async fn recover_legacy(
    dir: &Path,
    current: &Path,
    staging: &Path,
    backup: &Path,
    mut has_current: bool,
    has_backup: bool,
) -> Result<(), String> {
    if has_current && has_backup {
        return Err(uncertain(
            dir,
            "旧版事务同时存在当前目录和备份，不能判定提交结果",
        ));
    }
    if !has_current && has_backup {
        tokio::fs::rename(backup, current)
            .await
            .map_err(|e| uncertain(dir, format!("旧版回滚失败：{e}")))?;
        has_current = true;
    }
    if !has_current {
        return Err(uncertain(dir, "当前目录和备份均缺失"));
    }
    if !plain_dir(staging).await? {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(staging)
        .await
        .map_err(|e| uncertain(dir, e))?;
    while let Some(entry) = entries.next_entry().await.map_err(|e| uncertain(dir, e))? {
        if entry.file_name() != "dsh-home" {
            return Err(uncertain(dir, "旧版暂存目录含未识别内容"));
        }
    }
    for keep in KEEP {
        let staged = staging.join("dsh-home").join(keep);
        if plain_dir(&staged).await? && plain_dir(&current.join(keep)).await? {
            return Err(uncertain(
                dir,
                format!("{keep} 同时存在于当前目录和暂存目录，拒绝覆盖"),
            ));
        }
    }
    for keep in KEEP {
        let staged = staging.join("dsh-home").join(keep);
        if plain_dir(&staged).await? {
            tokio::fs::rename(&staged, current.join(keep))
                .await
                .map_err(|e| uncertain(dir, format!("归还 {keep} 失败：{e}")))?;
        }
    }
    remove_owned(staging).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run<F, H>(
    root: &Path,
    id: &str,
    snapshot_id: &str,
    processes: &Processes,
    flag: &Arc<AtomicBool>,
    on_progress: &F,
    hook: &H,
) -> Result<InstanceRecord, String>
where
    F: Fn(CloneProgress) + Send + Sync,
    H: Fn(Point) -> Result<(), String> + Send + Sync,
{
    let id = sanitize_segment(id, "实例 id")?;
    let dir = instance_dir(root, &id)?;
    let manifest = load_manifest(&dir, &id).await?;
    ensure_not_running(processes, &id)?;
    reject_external_write(&manifest, &id, "还原快照并覆盖")?;
    let _lock = lock(&dir).await?;
    recover_locked(&dir, &manifest).await?;
    let snap_dir = snapshot_dir(&dir, snapshot_id)?;
    let raw = tokio::fs::read(snap_dir.join("snapshot.json"))
        .await
        .map_err(|e| format!("快照不可读取 {snapshot_id}：{e}"))?;
    serde_json::from_slice::<SnapshotFile>(&raw).map_err(|e| format!("快照元数据解析失败：{e}"))?;
    let snap_home = snap_dir.join("dsh-home");
    let current = home_of(&dir, &manifest);
    if !plain_dir(&snap_home).await? || !plain_dir(&current).await? {
        return Err("快照或实例缺少 dsh-home，无法还原".into());
    }
    let staging = dir.join(STAGING);
    // Exclusive creation; never delete an old transaction to make room.
    tokio::fs::create_dir(&staging)
        .await
        .map_err(|e| uncertain(&dir, e))?;
    write_journal(&dir, &id, Phase::Copying)?;
    let dest_home = staging.join("dsh-home");
    let copying = async {
        let mut copied = copy_tree_with_progress(
            snap_home.clone(),
            dest_home.clone(),
            Arc::clone(flag),
            SkipRule::Restore,
            LinkPolicy::Preserve {
                source_root: snap_home,
                dest_root: current.clone(),
            },
            root.to_path_buf(),
            &|mut p: CloneProgress| {
                p.progress *= 0.8;
                on_progress(p);
            },
        )
        .await?;
        hook(Point::SnapshotCopied)?;
        for (index, keep) in KEEP.into_iter().enumerate() {
            // Windows case aliases can bypass a literal skip rule. Discard
            // only staged snapshot copies, while all originals are intact.
            remove_owned(&dest_home.join(keep)).await?;
            if index == 1 {
                hook(Point::CopyAttachments)?;
            }
            let live = current.join(keep);
            if plain_dir(&live).await? {
                copied += copy_tree_with_progress(
                    live.clone(),
                    dest_home.join(keep),
                    Arc::clone(flag),
                    SkipRule::Nothing,
                    LinkPolicy::Preserve {
                        // A link into mutable configuration could lose its
                        // target when the old environment is retired. Keep
                        // only links within this retained conversation tree
                        // (or the copy engine's immutable shared versions).
                        source_root: live,
                        dest_root: current.join(keep),
                    },
                    root.to_path_buf(),
                    &|mut p: CloneProgress| {
                        p.progress = 0.8 + 0.1 * (index as f64 + p.progress);
                        on_progress(p);
                    },
                )
                .await?;
            }
            hook(if index == 0 {
                Point::SessionsCopied
            } else {
                Point::AttachmentsCopied
            })?;
        }
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        Ok::<_, String>(copied)
    }
    .await;
    let copied = match copying {
        Ok(copied) => copied,
        Err(error) => {
            if let Err(cleanup) = recover_locked(&dir, &manifest).await {
                return Err(format!("还原复制失败：{error}；{cleanup}"));
            }
            return Err(error);
        }
    };
    // Only this durable record authorizes the swap and backup retirement.
    write_journal(&dir, &id, Phase::Ready)?;
    hook(Point::Backup)?;
    let backup = dir.join(BACKUP);
    tokio::fs::rename(&current, &backup)
        .await
        .map_err(|e| uncertain(&dir, format!("备份原目录失败：{e}")))?;
    let promotion = async {
        hook(Point::BackedUp)?;
        hook(Point::Promote)?;
        tokio::fs::rename(&dest_home, &current)
            .await
            .map_err(|e| format!("提交还原失败：{e}"))
    }
    .await;
    if let Err(error) = promotion {
        let rollback = async {
            hook(Point::Rollback)?;
            tokio::fs::rename(&backup, &current)
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        if let Err(rollback) = rollback {
            return Err(uncertain(
                &dir,
                format!(
                    "{error}；回滚失败：{rollback}；原目录保存在 {}",
                    backup.display()
                ),
            ));
        }
        recover_locked(&dir, &manifest)
            .await
            .map_err(|cleanup| format!("{error}；{cleanup}"))?;
        return Err(error);
    }
    hook(Point::Promoted)?;
    hook(Point::Cleanup)?;
    recover_locked(&dir, &manifest)
        .await
        .map_err(|e| format!("快照内容已提交，但还原清理未完成：{e}"))?;
    on_progress(CloneProgress {
        progress: 1.0,
        bytes_done: copied,
        bytes_total: copied,
    });
    let manifest = load_manifest(&dir, &id).await?;
    Ok(build_record(&dir, manifest).await)
}

#[cfg(test)]
mod tests;
