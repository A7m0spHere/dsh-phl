//! Archive handling: tar.gz extraction with traversal-safe path resolution
//! and compressed-byte progress.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use flate2::read::GzDecoder;

struct CountingReader<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Resolves a tar entry's path under `dest`, refusing anything that escapes.
///
/// `Path::starts_with` compares components without resolving `..`, so
/// `dest.join("../../x")` still "starts with" `dest` and passes a prefix
/// check — while the OS happily resolves it at `File::create` time. The
/// components have to be rejected themselves.
pub(crate) fn safe_join(dest: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut out = dest.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("压缩包包含越界路径，已中止".into())
            }
        }
    }
    Ok(out)
}

pub(crate) async fn extract<F: Fn(f64) + Send + Sync>(
    archive_path: &Path,
    dest: &Path,
    on_tick: &F,
    flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| e.to_string())?;
    let compressed = tokio::fs::metadata(archive_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // The tar jobs are blocking; run them on a worker thread and report
    // progress through a channel the async loop can forward.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<f64, String>>(16);
    let path = archive_path.to_path_buf();
    let dest_clone = dest.to_path_buf();
    let flag_worker = Arc::clone(flag);
    std::thread::spawn(move || {
        let consumed = Arc::new(AtomicU64::new(0));
        let result = (|| -> Result<(), String> {
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let counted = CountingReader {
                inner: file,
                read: Arc::clone(&consumed),
            };
            let mut archive = tar::Archive::new(GzDecoder::new(counted));
            let entries = archive.entries().map_err(|e| e.to_string())?;
            for entry in entries {
                // The cancel flag has to be read *here*, not only by the
                // async receiver: without it the thread kept unpacking after
                // the caller had already started deleting the directory.
                if flag_worker.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                let mut entry = entry.map_err(|e| e.to_string())?;
                let relative = strip_first(&entry.path().map_err(|e| e.to_string())?)
                    .ok_or("压缩包内没有预期的 package/ 前缀")?;
                if relative.as_os_str().is_empty() {
                    continue; // the prefix directory entry itself
                }
                let target = safe_join(&dest_clone, &relative)?;
                if entry.header().entry_type().is_dir() {
                    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                } else {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
                }
                let progress = if compressed > 0 {
                    (consumed.load(Ordering::Relaxed) as f64 / compressed as f64).min(1.0)
                } else {
                    0.0
                };
                // A closed channel means the caller gave up; stop rather
                // than keep writing into a directory it is cleaning up.
                if tx.blocking_send(Ok(progress)).is_err() {
                    return Err("cancelled".into());
                }
            }
            Ok(())
        })();
        let _ = tx.blocking_send(result.map(|()| 1.0));
    });

    while let Some(msg) = rx.recv().await {
        match msg {
            Ok(progress) => on_tick(progress),
            Err(e) => return Err(e),
        }
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
    }
    Ok(())
}

/// npm tarballs nest everything under `package/`; drop that single prefix.
/// GitHub codeload tarballs have a `repo-<sha>/` prefix that strips the same
/// way, so the plugin installer reuses this too.
pub(crate) fn strip_first(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    components.next()?;
    Some(components.as_path().to_path_buf())
}
