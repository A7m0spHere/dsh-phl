//! Storage-level operations on the whole data root: free-space queries and
//! moving the root's data between drives. Like every other command, the root
//! is always an explicit argument — Rust keeps no opinion about where PHL
//! lives, so "moving" is copying trees plus the frontend re-pointing its
//! per-call root afterwards.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::instances::{copy_tree, dir_size_skipping};
use crate::versions::Transfers;

/// Every subdirectory of the root that carries PHL data, in migration order.
/// `cache` is disposable but still moved, so the freed space actually arrives
/// on the new drive instead of re-accumulating on the old one.
const DATA_DIRS: [&str; 4] = ["instances", "versions", "runtimes", "cache"];

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
        cache: dir("cache"),
        has_data: false,
    };
    RootDataSummary {
        has_data: summary.instances.entries > 0
            || summary.versions.entries > 0
            || summary.runtimes.entries > 0
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

#[tauri::command]
pub async fn move_root_data(
    transfers: State<'_, Transfers>,
    transfer_id: String,
    from: String,
    to: String,
    on_progress: Channel<MoveProgress>,
) -> Result<MoveSummary, String> {
    let flag = transfers.take(&transfer_id);
    let result = move_root_inner(&flag, Path::new(&from), Path::new(&to), &|p| {
        let _ = on_progress.send(p);
    })
    .await;
    transfers.release(&transfer_id);
    result
}

async fn move_root_inner<F: Fn(MoveProgress) + Send + Sync>(
    flag: &Arc<AtomicBool>,
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

    let mut moved: Vec<String> = Vec::new();
    let mut bytes_moved = 0u64;

    for kind in DATA_DIRS {
        if flag.load(Ordering::SeqCst) {
            // A partial stop is a valid result: finished kinds live in the
            // destination, untouched kinds stay in the source, and a re-run
            // skips whatever has already moved.
            return Ok(MoveSummary {
                moved,
                bytes: bytes_moved,
                cancelled: true,
            });
        }
        let src = from.join(kind);
        if !src.exists() {
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
            tokio::fs::remove_dir(&dst).await.map_err(|e| e.to_string())?;
        }

        let bytes = dir_size_skipping(&src);
        // A same-drive move renames instantly; anything else (across drives)
        // falls back to copy-then-delete below.
        if tokio::fs::rename(&src, &dst).await.is_ok() {
            moved.push(kind.into());
            bytes_moved += bytes;
            let _ = on_progress(MoveProgress {
                kind: kind.into(),
                progress: 1.0,
                bytes_done: bytes,
                bytes_total: bytes,
            });
            continue;
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
        let worker_flag = Arc::clone(flag);
        let src2 = src.clone();
        let dst2 = dst.clone();
        let worker = std::thread::spawn(move || {
            let mut done = 0u64;
            let result = copy_tree(&src2, &dst2, &worker_flag, &mut done, bytes, &tx);
            let _ = tx.blocking_send(result.map(|()| (bytes, bytes)));
        });

        let mut copy_result = Ok(());
        while let Some(message) = rx.recv().await {
            match message {
                Ok((done, total)) => {
                    let _ = on_progress(MoveProgress {
                        kind: kind.into(),
                        progress: if total > 0 {
                            (done as f64 / total as f64).min(1.0)
                        } else {
                            1.0
                        },
                        bytes_done: done,
                        bytes_total: total,
                    });
                }
                Err(e) => {
                    copy_result = Err(e);
                    break;
                }
            }
        }
        let _ = worker.join();
        // The cancel check must precede the error check: copy_tree reports
        // cancellation as an Err("cancelled"), but a cancelled migration is a
        // normal outcome, not a failure.
        if flag.load(Ordering::SeqCst) {
            // A cancelled copy must not leave a partial tree behind — a later
            // run's "is the target non-empty" guard would trip over it.
            let _ = tokio::fs::remove_dir_all(&dst).await;
            return Ok(MoveSummary {
                moved,
                bytes: bytes_moved,
                cancelled: true,
            });
        }
        copy_result?;
        tokio::fs::remove_dir_all(&src)
            .await
            .map_err(|e| format!("内容已复制到新目录，但删除源目录失败（可稍后手动删除）: {e}"))?;
        moved.push(kind.into());
        bytes_moved += bytes;
    }

    Ok(MoveSummary {
        moved,
        bytes: bytes_moved,
        cancelled: false,
    })
}
