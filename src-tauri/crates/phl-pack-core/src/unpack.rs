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

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::PackError;

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
pub fn unpack_entries_to<F>(pack_path: &Path, mut map: F) -> Result<Vec<ExtractedFile>, PackError>
where
    F: FnMut(&Path) -> Option<(PathBuf, PathBuf)>,
{
    let file = std::fs::File::open(pack_path)
        .map_err(|e| PackError::Unreadable(format!("打开整合包失败: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| PackError::Unreadable(e.to_string()))?;
    let mut out = Vec::new();
    for index in 0..archive.len() {
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
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| PackError::Unreadable(format!("解压读取失败 {}: {e}", entry.name())))?;
        std::fs::write(&confined, &bytes)
            .map_err(|e| PackError::Unreadable(format!("写入解包文件失败 {confined:?}: {e}")))?;
        out.push(ExtractedFile {
            archive_name: entry.name().replace('\\', "/"),
            dest: confined,
            bytes: bytes.len() as u64,
        });
    }
    Ok(out)
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
}
