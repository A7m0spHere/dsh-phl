//! Retarget managed version-tree links after a trial copy.

use std::path::{Path, PathBuf};

/// Retargets directory links under `staging_home` that point into the shared
/// `versions/<old>/` tree onto `versions/<new>/`, preserving the rest of the
/// path, and verifies every retargeted link resolves. Returns (redirected,
/// failures): a failure names a link that now does not resolve — the honest
/// signal that a plugin needs its dependencies rebuilt, not something to
/// hide behind a green result.
pub(super) async fn retarget_version_links(
    home: &Path,
    root: &Path,
    old_bare: &str,
    new_bare: &str,
) -> (usize, Vec<String>) {
    if old_bare == new_bare {
        return (0, Vec::new());
    }
    let mut redirects = 0usize;
    let mut failures = Vec::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
                continue;
            };
            // A reparse point covers both real symlinks and Windows
            // junctions; the copy engine lays both down as the latter.
            #[cfg(windows)]
            let is_reparse = {
                use std::os::windows::fs::MetadataExt;
                (meta.file_attributes() & 0x400) != 0 // FILE_ATTRIBUTE_REPARSE_POINT
            };
            #[cfg(not(windows))]
            let is_reparse = meta.file_type().is_symlink();
            if is_reparse {
                // Candidate: read where it points.
                if let Some(target) = read_link_target(&path) {
                    let versions_prefix = root.join("versions").join(old_bare);
                    if let Some(rel) = strip_prefix_link(&target, &versions_prefix) {
                        let new_target = root.join("versions").join(new_bare).join(rel);
                        if new_target.exists() {
                            // A junction is a directory reparse point:
                            // remove_dir drops the link, not the target.
                            let _ = tokio::fs::remove_dir(&path).await;
                            let _ = tokio::fs::remove_file(&path).await;
                            match super::super::copy::recreate_link(&path, &new_target, true) {
                                Ok(()) => redirects += 1,
                                Err(e) => failures.push(format!(
                                    "{}: 重建失败 {e}",
                                    path.strip_prefix(home).unwrap_or(&path).display()
                                )),
                            }
                        } else {
                            failures.push(format!(
                                "{}: 新版本下不存在 {}（上游可能已改名或移除该包，\
                                 可在副本内卸载后按需重装）",
                                path.strip_prefix(home).unwrap_or(&path).display(),
                                new_target.display()
                            ));
                        }
                    }
                }
                continue; // never descend into a reparse point
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    (redirects, failures)
}

/// Where a link points, as an absolute path (junctions resolve through
/// canonicalize; symlink_metadata + read_link covers the rest).
fn read_link_target(path: &Path) -> Option<PathBuf> {
    if let Ok(target) = std::fs::read_link(path) {
        if target.is_absolute() {
            return Some(target);
        }
        if let Some(parent) = path.parent() {
            return Some(parent.join(target));
        }
        return None;
    }
    // A Windows junction is a directory reparse point; read_link covers it,
    // but canonicalize is the fallback that resolves the true location.
    std::fs::canonicalize(path).ok()
}

/// `path.strip_prefix(base)` for a link target against a versions prefix,
/// tolerating the two spellings Windows gives the SAME directory.
///
/// The junctions npm/PHL lay down carry the target spelling `mklink` got —
/// and PHL's copy engine derives that target from `canonicalize`, which
/// expands 8.3 short names (`RUNNER~1` → `runneradmin`) and normalizes
/// case to what the volume reports. The versions prefix, however, is built
/// from the data-root string the caller holds, which can be the short-name
/// or differently-cased spelling (a CI runner's TEMP is
/// `C:\Users\RUNNER~1\AppData\...`). `Path::starts_with` compares
/// byte-for-byte, so the retarget would silently skip a link it owns and
/// leave the copy pointing at the OLD version tree.
///
/// Resolution: compare against the canonical spelling of the prefix when it
/// exists on disk (same expansion, same case), plus the raw spelling
/// case-insensitively for a target that names an already-deleted tree (the
/// missing-content failure path must still classify and report the link).
#[cfg(windows)]
fn strip_prefix_link(target: &Path, prefix: &Path) -> Option<PathBuf> {
    /// Case-insensitive component-wise prefix test on verbatim-stripped paths.
    fn is_prefix_ci(path: &Path, base: &Path) -> bool {
        let mut pc = path.components();
        let mut bc = base.components();
        loop {
            match (bc.next(), pc.next()) {
                (Some(b), Some(p)) => {
                    let same = p == b
                        || p.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy());
                    if !same {
                        return false;
                    }
                }
                // Base exhausted, path still has components: that IS the
                // prefix match. The other exhaustion (path shorter) falls
                // through to the arm below.
                (None, _) => return true,
                (Some(_), None) => return false,
            }
        }
    }

    fn strip_ci(path: &Path, base: &Path) -> Option<PathBuf> {
        let p = crate::paths::strip_verbatim(path);
        let b = crate::paths::strip_verbatim(base);
        if !is_prefix_ci(&p, &b) {
            return None;
        }
        // The remainder borrows from `p`; return an owned slice of it instead.
        Some(p.components().skip(b.components().count()).collect())
    }

    // The raw spelling first: it is what the target text actually says, and
    // the only option when the prefix tree no longer exists (the failure
    // path — canonicalize of a deleted dir errors).
    if let Some(rel) = strip_ci(target, prefix) {
        return Some(rel);
    }
    // Otherwise the same directory may be spelled differently on each side;
    // canonicalize both (existing) sides and compare the canonical forms.
    let canon_target = std::fs::canonicalize(target).ok()?;
    let canon_prefix = std::fs::canonicalize(prefix).ok()?;
    strip_ci(
        &crate::paths::strip_verbatim(&canon_target),
        &crate::paths::strip_verbatim(&canon_prefix),
    )
}

/// Unix paths are case-sensitive, but macOS may spell the same temp directory
/// as `/var` and `/private/var`. The copy engine canonicalizes managed link
/// targets, while the data root may retain its original spelling.
#[cfg(not(windows))]
fn strip_prefix_link(target: &Path, prefix: &Path) -> Option<PathBuf> {
    if let Ok(rel) = target.strip_prefix(prefix) {
        return Some(rel.to_path_buf());
    }
    let canonical_target = std::fs::canonicalize(target).ok()?;
    let canonical_prefix = std::fs::canonicalize(prefix).ok()?;
    canonical_target
        .strip_prefix(canonical_prefix)
        .ok()
        .map(PathBuf::from)
}
