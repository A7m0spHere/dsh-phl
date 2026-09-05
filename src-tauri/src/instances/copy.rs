//! The shared tree-copy machinery: cloning, snapshots and data-root
//! relocation all copy directories with progress and cancellation, and the
//! skip rules differ per caller (documented on `SkipRule`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::CloneProgress;

pub(crate) fn copy_tree_sync(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_dir() {
            copy_tree_sync(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("复制 {} 失败: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

pub(crate) fn skipped(name: &str) -> bool {
    name == "logs" || name == "snapshots" || name == ".phl-cache" || name.starts_with(".phl-")
}

/// What a copy is allowed to leave behind.
///
/// This is not a detail: the same `copy_tree` serves cloning an instance and
/// relocating the entire data root, and those want opposite things. Migrating
/// with the clone's filter silently dropped every instance's `snapshots/` and
/// `logs/` and then deleted the source — destroying the user's only rollback
/// points while reporting success.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipRule {
    /// Copy everything. Required whenever the copy replaces the original.
    Nothing,
    /// Drop run state, but only at the tree's own root. A nested `logs/` deep
    /// inside `node_modules` belongs to whatever package created it and is
    /// part of that package, not PHL's per-instance history.
    RunStateAtRoot,
}

impl SkipRule {
    pub(crate) fn skips(self, name: &str) -> bool {
        self == SkipRule::RunStateAtRoot && skipped(name)
    }
    /// Recursion always descends with `Nothing`: the rule only ever applies to
    /// the entries directly under the root it was given.
    pub(crate) fn inside(self) -> Self {
        SkipRule::Nothing
    }
}

pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

pub(crate) fn dir_size_skipping(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// Shared by cloning, snapshots and cross-drive relocation. Completion comes
/// from the worker result, never from a closed progress channel. Awaiting the
/// worker also ensures cancellation cannot race staging-directory cleanup.
pub(crate) async fn copy_tree_with_progress<F: Fn(CloneProgress) + Send + Sync>(
    from: PathBuf,
    to: PathBuf,
    flag: Arc<AtomicBool>,
    skip: SkipRule,
    on_progress: &F,
) -> Result<u64, String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
    let worker = tokio::task::spawn_blocking(move || {
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let total = match skip {
            SkipRule::Nothing => dir_size(&from),
            SkipRule::RunStateAtRoot => dir_size_skipping(&from),
        };
        let mut done = 0;
        copy_tree(&from, &to, &flag, &mut done, total, &tx, skip)?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        Ok(done)
    });
    while let Some(message) = rx.recv().await {
        let (bytes_done, bytes_total) = message?;
        on_progress(CloneProgress {
            progress: if bytes_total == 0 {
                1.0
            } else {
                (bytes_done as f64 / bytes_total as f64).min(1.0)
            },
            bytes_done,
            bytes_total,
        });
    }
    worker.await.map_err(|e| format!("复制线程异常退出: {e}"))?
}

pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    flag: &AtomicBool,
    done: &mut u64,
    total: u64,
    tx: &tokio::sync::mpsc::Sender<Result<(u64, u64), String>>,
    skip: SkipRule,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取复制源目录失败: {e}"))?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = entry.file_name();
        if skip.skips(&name.to_string_lossy()) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        let meta = entry
            .metadata()
            .map_err(|e| format!("读取复制源属性失败 {}: {e}", src.display()))?;
        // A relocation deletes the source afterwards. Silently skipping a
        // link (or following it outside the tree) would lose or duplicate data.
        if meta.file_type().is_symlink() {
            return Err(format!(
                "复制源包含符号链接，请先处理后重试: {}",
                src.display()
            ));
        }
        if meta.is_dir() {
            copy_tree(&src, &dst, flag, done, total, tx, skip.inside())?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("复制失败 {}: {e}", src.display()))?;
            *done += meta.len();
            // A closed channel means the caller gave up; stop rather than keep
            // writing into a directory it is already deleting.
            if tx.blocking_send(Ok((*done, total))).is_err() {
                return Err("cancelled".into());
            }
        }
    }
    Ok(())
}
