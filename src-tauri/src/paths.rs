//! The backend-owned data root and filesystem containment.
//!
//! The frontend may *suggest* a root — the first-run chooser and the storage
//! settings both do — but the authoritative value lives here, in `PhlState`,
//! persisted to a pointer file outside the data root so it survives restarts
//! and relocations. Destructive commands resolve ids into concrete paths only
//! in Rust and confine every one of them under the root.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use tauri::State;

/* ---------------------------- path segments ---------------------------- */

/// Ids, profile names and snapshot ids are pasted straight into filesystem
/// paths, so they get a whitelist: anything that is not a plain segment is
/// rejected rather than normalized.
pub(crate) fn sanitize_segment(value: &str, label: &str) -> Result<String, String> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && value != "."
        && value != "..";
    if ok {
        Ok(value.to_string())
    } else {
        Err(format!("非法的{label}: {value}"))
    }
}

/* ----------------------------- containment ------------------------------ */

/// Strips the extended-length prefix `std::fs::canonicalize` adds on Windows
/// (`\\?\C:\...`, `\\?\UNC\server\share\...`). Without this, a canonicalized
/// child never lexically starts with a plain-form root and every containment
/// check would false-positive on Windows.
pub(crate) fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    let stripped = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(|rest| rest.to_string()));
    match stripped {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// Refuses `path` unless it lives inside `root` — including through aliases.
///
/// Two layers, on purpose. The lexical check is component-based, so a sibling
/// like `<root>-evil` cannot prefix-match its way in and `..` is refused
/// outright. The canonicalized check then runs whenever both paths exist,
/// which is what catches a symlink or Windows junction planted inside the
/// tree: resolution walks to the real destination, and that destination has
/// to still be under the root. A target that does not exist yet can only be
/// judged lexically; callers performing destructive work on it re-check once
/// it exists.
pub(crate) fn ensure_under_root(root: &Path, path: &Path) -> Result<(), String> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("拒绝访问数据目录之外的路径: {}", path.display()));
    }
    match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
        (Ok(root_real), Ok(path_real)) => {
            if strip_verbatim(&path_real).starts_with(strip_verbatim(&root_real)) {
                Ok(())
            } else {
                Err(format!("拒绝访问数据目录之外的路径: {}", path.display()))
            }
        }
        // Canonicalization failed (a path does not exist yet, or is not
        // accessible): fall back to the lexical component check, which is
        // still prefix-safe — Path::starts_with compares whole components.
        _ => {
            let base = std::fs::canonicalize(root)
                .map(|p| strip_verbatim(&p))
                .unwrap_or_else(|_| root.to_path_buf());
            if path.starts_with(&base) || path.starts_with(root) {
                Ok(())
            } else {
                Err(format!("拒绝访问数据目录之外的路径: {}", path.display()))
            }
        }
    }
}

/* ------------------------------ data root ------------------------------- */

/// Where PHL keeps everything when nothing else has been chosen.
pub(crate) fn default_root() -> PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join("PHL"))
        .unwrap_or_else(|| PathBuf::from("PHL"))
}

/// The backend's single source of truth for where PHL data lives.
///
/// Managed Tauri state; commands resolve ids against `root()` instead of
/// trusting a root path from the IPC boundary. The choice is persisted to a
/// pointer file *outside* the data root (`{config}/PHL/root.json`), so
/// relocating the data does not strand the pointer inside the old tree.
pub struct PhlState {
    root: RwLock<PathBuf>,
    pointer: Option<PathBuf>,
    /// True until the pointer file exists. While true, the frontend's boot
    /// handshake (`init_phl_root`) may adopt its stored root — that is what
    /// carries existing installs through the upgrade from frontend-owned
    /// roots without their data appearing to vanish.
    provisional: AtomicBool,
}

impl PhlState {
    /// Loads the state at app start. Never fails: an unreadable pointer just
    /// means the default root is used until the handshake runs.
    pub fn load() -> Self {
        Self::with_pointer(dirs::config_dir().map(|d| d.join("PHL").join("root.json")))
    }

    pub(crate) fn with_pointer(pointer: Option<PathBuf>) -> Self {
        let (root, provisional) = pointer
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(|raw| (PathBuf::from(raw), false))
            .unwrap_or_else(|| (default_root(), true));
        Self {
            root: RwLock::new(root),
            pointer,
            provisional: AtomicBool::new(provisional),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.root.read().expect("phl root lock").clone()
    }

    /// A file that must survive a data-root relocation lives next to the
    /// pointer (which itself lives outside the root). `None` when the pointer
    /// is unavailable (no config dir) — callers degrade to non-persistent
    /// behaviour rather than parking state inside the tree being moved.
    pub(crate) fn sibling_file(&self, name: &str) -> Option<PathBuf> {
        self.pointer.as_ref().map(|p| p.with_file_name(name))
    }

    fn is_provisional(&self) -> bool {
        self.provisional.load(Ordering::SeqCst)
    }

    /// The boot handshake. A pointer file wins outright; only a still-
    /// provisional root may be re-pointed by the frontend's stored value.
    pub fn adopt(&self, hint: Option<&str>) -> Result<PathBuf, String> {
        if !self.is_provisional() {
            return Ok(self.root());
        }
        match hint.map(str::trim).filter(|h| !h.is_empty()) {
            Some(hint) => self.set(validate_root(hint)?),
            None => {
                let root = self.root();
                self.persist(&root)?;
                Ok(root)
            }
        }
    }

    /// An explicit root change from the settings UI.
    pub fn set_root(&self, root: &str) -> Result<PathBuf, String> {
        self.set(validate_root(root)?)
    }

    fn set(&self, root: PathBuf) -> Result<PathBuf, String> {
        self.persist(&root)?;
        *self.root.write().expect("phl root lock") = root.clone();
        self.provisional.store(false, Ordering::SeqCst);
        Ok(root)
    }

    /// The pointer is written *before* the in-memory root moves: a failed
    /// persist leaves the current root in charge rather than a state that
    /// forgets itself on the next restart.
    fn persist(&self, root: &Path) -> Result<(), String> {
        let Some(pointer) = &self.pointer else {
            return Ok(());
        };
        if let Some(parent) = pointer.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("无法创建配置目录: {e}"))?;
        }
        std::fs::write(pointer, root.to_string_lossy().as_bytes())
            .map_err(|e| format!("无法记录数据目录: {e}"))
    }
}

fn validate_root(root: &str) -> Result<PathBuf, String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return Err("数据目录不能为空".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(format!("数据目录必须是绝对路径: {trimmed}"));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("数据目录不能包含相对路径段: {trimmed}"));
    }
    Ok(path)
}

/* ------------------------------ commands ------------------------------- */

/// The boot handshake: adopts the frontend's stored root if — and only if —
/// the backend has no pointer file yet, then reports the authoritative root.
#[tauri::command]
pub fn init_phl_root(hint: Option<String>, state: State<'_, PhlState>) -> Result<String, String> {
    state
        .adopt(hint.as_deref())
        .map(|root| root.to_string_lossy().into_owned())
}

/// Points PHL at a new data root (settings → 存储). Persists the choice;
/// moving the data itself is `move_root_data`'s job.
#[tauri::command]
pub fn set_phl_root(root: String, state: State<'_, PhlState>) -> Result<String, String> {
    state
        .set_root(&root)
        .map(|root| root.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-paths-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn segments_reject_path_tricks() {
        assert!(sanitize_segment("instance-a1b2", "id").is_ok());
        assert!(sanitize_segment("default", "profile").is_ok());
        assert!(sanitize_segment("..", "id").is_err());
        assert!(sanitize_segment("a/b", "id").is_err());
        assert!(sanitize_segment("a\\b", "id").is_err());
        assert!(sanitize_segment("", "id").is_err());
    }

    #[test]
    fn containment_accepts_children_and_refuses_escapes() {
        let root = temp_root("contain");
        let inner = root.join("instances").join("demo");
        std::fs::create_dir_all(&inner).unwrap();

        assert!(ensure_under_root(&root, &inner).is_ok());
        assert!(ensure_under_root(&root, &root).is_ok());

        // A sibling whose *name* shares a prefix must not slip through —
        // Path::starts_with compares components, not characters.
        let sibling = root.with_file_name(format!(
            "{}-evil",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(ensure_under_root(&root, &sibling).is_err());

        // `..` is refused outright, in any position.
        assert!(ensure_under_root(&root, &root.join("a").join("..").join("b")).is_err());

        // An absolute path elsewhere fails the canonical check.
        assert!(ensure_under_root(&root, &std::env::temp_dir()).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn containment_survives_the_verbatim_prefix_canonicalization_adds() {
        let root = temp_root("verbatim");
        let inner = root.join("child");
        std::fs::create_dir_all(&inner).unwrap();

        // canonicalize returns `\\?\C:\...` on Windows; the comparison must
        // strip it or every real path would be rejected.
        let canonical = std::fs::canonicalize(&root).unwrap();
        assert_ne!(canonical, root, "precondition: prefix form differs");
        assert!(ensure_under_root(&root, &inner).is_ok());
        assert!(ensure_under_root(&root, &root.join("missing-child")).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_symlink_inside_the_tree_cannot_redirect_deletion() {
        let root = temp_root("symlink");
        // The target lives outside the data root entirely — a sibling of it.
        let outside =
            std::env::temp_dir().join(format!("phl-paths-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(root.join("instances")).unwrap();
        let link = std::os::windows::fs::symlink_dir(&outside, root.join("instances").join("link"));
        // Creating symlinks on Windows needs developer mode or admin; skip
        // rather than fail on machines without the privilege.
        if link.is_err() {
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }

        let linked = root.join("instances").join("link");
        // Lexically inside; canonicalized, the target is outside the root.
        assert!(linked.starts_with(root.join("instances")));
        assert!(ensure_under_root(&root, &linked).is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn phl_state_prefers_the_pointer_and_persists_changes() {
        let dir = temp_root("state");
        let pointer = dir.join("nested").join("root.json");

        // No pointer yet: the default root is provisional and not persisted.
        let state = PhlState::with_pointer(Some(pointer.clone()));
        assert_eq!(state.root(), default_root());
        assert!(state.is_provisional());
        assert!(!pointer.exists(), "boot alone must not write the pointer");

        // The handshake adopts the frontend's stored root once.
        let adopted = state.adopt(Some("C:\\PHL Data")).unwrap();
        assert_eq!(adopted, PathBuf::from("C:\\PHL Data"));
        assert_eq!(std::fs::read_to_string(&pointer).unwrap(), "C:\\PHL Data");
        // …and only once: a later handshake cannot re-point a decided root.
        state.adopt(Some("D:\\Elsewhere")).unwrap();
        assert_eq!(state.root(), PathBuf::from("C:\\PHL Data"));
        assert_eq!(std::fs::read_to_string(&pointer).unwrap(), "C:\\PHL Data");

        // An explicit set always wins and persists.
        state.set_root("E:\\Moved").unwrap();
        assert_eq!(state.root(), PathBuf::from("E:\\Moved"));
        assert_eq!(std::fs::read_to_string(&pointer).unwrap(), "E:\\Moved");

        // The pointer file is authoritative on the next boot.
        let reloaded = PhlState::with_pointer(Some(pointer));
        assert_eq!(reloaded.root(), PathBuf::from("E:\\Moved"));
        assert!(!reloaded.is_provisional());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_validation_refuses_relative_and_traversing_paths() {
        assert!(validate_root("").is_err());
        assert!(validate_root("  ").is_err());
        assert!(validate_root("relative\\path").is_err());
        assert!(validate_root("C:\\a\\..\\b").is_err());
        // `.` never reaches validate_root: Path::components normalizes it
        // away, which is exactly why only `..` needs refusing lexically.
        assert_eq!(
            Path::new("C:\\a\\.\\b").components().next_back(),
            Some(Component::Normal(OsStr::new("b")))
        );
        assert!(validate_root("C:\\PHL Data").is_ok());
    }

    #[test]
    fn a_failed_persist_keeps_the_current_root() {
        let dir = temp_root("persist");
        // A pointer whose parent is a *file* makes every write fail.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let pointer = blocker.join("root.json");

        let state = PhlState::with_pointer(Some(pointer));
        let adopted = state.adopt(Some("C:\\PHL Data"));
        assert!(adopted.is_err());
        assert_eq!(state.root(), default_root(), "root unchanged after failure");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_is_stripped_for_comparison() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\PHL\Data")),
            PathBuf::from(r"C:\PHL\Data")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\PHL")),
            PathBuf::from(r"\\server\share\PHL")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"C:\PHL\Data")),
            PathBuf::from(r"C:\PHL\Data")
        );
    }
}
