//! Archive extraction — the consuming half of the format (development spec
//! §21–22). [`read_pack`] proves the archive is safe *by its names*; this
//! module turns those proven names into bytes on disk, and every destination
//! is confined to a caller-chosen root.
//!
//! Why confinement lives here and not just in the host: the spec's security
//! list ("`../` path traversal, absolute path, symlink escape …") applies to
//! *writing*, and both consumers — the PHL installer (embedded → profile,
//! sessions → home) and the `phl-pack` CLI (`unpack` to a dump dir) — would
//! otherwise re-implement it and could drift. [`unpack_entries_to`] keeps the
//! map decision with each caller (where files *should* go is product logic)
//! but owns the "refuse anything outside the root" rule once.
//!
//! The archive is re-opened here and read entry by entry. A caller that has
//! already run [`crate::read_pack`] has proven the names traversal-free; the
//! [`confine`] check is defence-in-depth for a re-read that races an edit,
//! never the sole barrier.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::{ensure_not_cancelled, PackError, MAX_SINGLE_ENTRY, MAX_UNCOMPRESSED_TOTAL};

/// Fixed streaming buffer for archive I/O (§R6): no payload is ever read or
/// written as a whole-entry `Vec`, so peak memory is independent of file size.
const STREAM_BUF: usize = 65_536;

/// One extracted payload file's destination (relative form is kept for the
/// caller to tally sessions vs plugins by the map it supplied).
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    /// Normalised archive-relative name (forward slashes).
    pub archive_name: String,
    /// Absolute on-disk path written.
    pub dest: PathBuf,
    /// Byte count written.
    pub bytes: u64,
}

/// Run `map` over every file entry of `pack_path`; when it returns a
/// destination, write the bytes there (confined to a root the caller passes
/// back, so a map bug cannot escape) and collect the result. A `map` returning
/// `None` skips the entry (the CLI dumps everything; the installer ignores
/// metadata-only paths like `phlpack.json` / `assets/`).
///
/// The caller supplies, per mapped dest, the `root` it must stay under. This
/// is intentionally not a single root: the installer writes embedded plugins
/// and sessions into *different* directories of one instance tree, so one
/// root would be a lie. `map` returns `(dest, root)` — the root is the part
/// of the instance the dest belongs to.
pub fn unpack_entries_to<F>(pack_path: &Path, map: F) -> Result<Vec<ExtractedFile>, PackError>
where
    F: FnMut(&Path) -> Option<(PathBuf, PathBuf)>,
{
    unpack_entries_to_with_cancel(pack_path, map, &|| false)
}

/// Cancellable form of [`unpack_entries_to`]. Cancellation is polled between
/// entries and for every decompressed 64 KiB chunk; the current partial file is
/// removed before the cancellation error is returned.
pub fn unpack_entries_to_with_cancel<M, C>(
    pack_path: &Path,
    mut map: M,
    cancel: &C,
) -> Result<Vec<ExtractedFile>, PackError>
where
    M: FnMut(&Path) -> Option<(PathBuf, PathBuf)>,
    C: Fn() -> bool + ?Sized,
{
    ensure_not_cancelled(cancel)?;
    let file = std::fs::File::open(pack_path)
        .map_err(|e| PackError::Unreadable(format!("打开整合包失败: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| PackError::Unreadable(e.to_string()))?;
    let mut out = Vec::new();
    let mut total_written: u64 = 0;
    for index in 0..archive.len() {
        ensure_not_cancelled(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| PackError::Unreadable(e.to_string()))?;
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        let rel = crate::normalize_entry(entry.name())?;
        let Some((dest, root)) = map(&rel) else {
            continue;
        };
        let confined = confine(&root, &dest)?;
        if let Some(parent) = confined.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PackError::Unreadable(format!("创建解包目录失败: {e}")))?;
        }
        // Stream the entry to disk in fixed buffers (§R6), enforcing the
        // single-file and total limits on the *actual* bytes decompressed (a
        // hostile archive can under-declare `size`), and removing the partial
        // file if any read/write step fails so the caller never sees a
        // half-written payload.
        let archive_name = entry.name().replace('\\', "/");
        let written = stream_entry_to_file(&mut entry, &confined, &mut total_written, cancel)
            .inspect_err(|_| {
                // A partial write must not survive: drop the half-written file.
                let _ = std::fs::remove_file(&confined);
            })?;
        out.push(ExtractedFile {
            archive_name,
            dest: confined,
            bytes: written,
        });
    }
    Ok(out)
}

/// Copy one entry into `dest` with a fixed buffer, growing the caller's running
/// `total_written`. Enforces `MAX_SINGLE_ENTRY` per file and
/// `MAX_UNCOMPRESSED_TOTAL` overall against the *bytes actually produced* — not
/// the archive's self-declared size — then flushes and returns the byte count.
fn stream_entry_to_file<R: Read>(
    entry: &mut R,
    dest: &Path,
    total_written: &mut u64,
    cancel: &(impl Fn() -> bool + ?Sized),
) -> Result<u64, PackError> {
    let mut file = std::fs::File::create(dest)
        .map_err(|e| PackError::Unreadable(format!("创建解包文件失败 {dest:?}: {e}")))?;
    let mut buf = [0u8; STREAM_BUF];
    let mut written: u64 = 0;
    loop {
        ensure_not_cancelled(cancel)?;
        let n = entry
            .read(&mut buf)
            .map_err(|e| PackError::Unreadable(format!("解压读取失败 {dest:?}: {e}")))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        *total_written += n as u64;
        if written > MAX_SINGLE_ENTRY {
            return Err(PackError::TooLarge(format!(
                "解包条目实际字节数超过单文件上限 {MAX_SINGLE_ENTRY}: {dest:?}"
            )));
        }
        if *total_written > MAX_UNCOMPRESSED_TOTAL {
            return Err(PackError::TooLarge(format!(
                "解包总字节数超过上限 {MAX_UNCOMPRESSED_TOTAL}"
            )));
        }
        file.write_all(&buf[..n])
            .map_err(|e| PackError::Unreadable(format!("写入解包文件失败 {dest:?}: {e}")))?;
    }
    file.flush()
        .map_err(|e| PackError::Unreadable(format!("刷新解包文件失败 {dest:?}: {e}")))?;
    Ok(written)
}

/// Join `dest` onto `root` only if it resolves inside `root`. `dest` arrives
/// already normalised by [`unpack_entries_to`] (forward-slash, traversal-free),
/// but the root containment is re-checked here rather than trusted: the whole
/// point of this layer is that *no* destination escapes its assigned root, so
/// a subtle map bug is still refused, not written.
pub(crate) fn confine(root: &Path, dest: &Path) -> Result<PathBuf, PackError> {
    // Lexical normalisation of the joined path: `a/b/../../c` has no `..`
    // after resolution, but if it *still* carries one or a root/prefix it
    // never belonged under `root` in the first place.
    let joined = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        root.join(dest)
    };
    let normalized = normalize_lexical(&joined);
    let root_norm = normalize_lexical(root);
    if normalized.starts_with(&root_norm) {
        Ok(normalized)
    } else {
        Err(PackError::PathTraversal(format!(
            "解包目标越出实例目录: {}",
            joined.display()
        )))
    }
}

/// Lexical path normalisation (no filesystem access, no symlink resolution):
/// collapse `.` and resolve `..` component by component so a containment
/// check is not fooled by an unnormalised `a/./b` vs `a/b`.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut stack: Vec<std::path::Component> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // Pop a normal component; a leading Prefix/RootDir stays.
                if matches!(stack.last(), Some(std::path::Component::Normal(_))) {
                    stack.pop();
                }
            }
            other => stack.push(other),
        }
    }
    stack.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_refuses_escapes_and_accepts_inside() {
        let root = Path::new("/home/me/inst");
        // Inside, clean and with `.` components that must resolve away.
        assert!(confine(root, Path::new("sessions/a/b.json")).is_ok());
        assert!(confine(root, Path::new("./sessions/x")).is_ok());
        // An absolute dest *outside* the root is refused…
        assert!(matches!(
            confine(root, Path::new("/etc/passwd")),
            Err(PackError::PathTraversal(_))
        ));
        // …as a `..` that climbs out, however it is spelled.
        assert!(matches!(
            confine(root, Path::new("../../outside")),
            Err(PackError::PathTraversal(_))
        ));
        assert!(matches!(
            confine(root, Path::new("sessions/../../../outside")),
            Err(PackError::PathTraversal(_))
        ));
    }

    #[test]
    fn lexical_normalisation_collapses_dot_and_dotdot() {
        assert_eq!(
            normalize_lexical(Path::new("a/./b/../c")),
            PathBuf::from("a/c")
        );
        assert_eq!(normalize_lexical(Path::new("./x")), PathBuf::from("x"));
    }

    #[test]
    fn streaming_unpack_observes_cancel_and_removes_partial_file() {
        use std::io::Write as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use zip::write::SimpleFileOptions;

        let dir =
            std::env::temp_dir().join(format!("phl-pack-cancel-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pack = dir.join("large.phlpack");
        let file = std::fs::File::create(&pack).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("assets/large.bin", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&vec![0x33u8; 1024 * 1024]).unwrap();
        zip.finish().unwrap();

        let out = dir.join("out");
        let partial = out.join("assets/large.bin");
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 3;
        let err =
            unpack_entries_to_with_cancel(&pack, |rel| Some((out.join(rel), out.clone())), &cancel)
                .unwrap_err();
        assert_eq!(err, PackError::Cancelled);
        assert!(
            !partial.exists(),
            "cancelled extraction left a partial file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
