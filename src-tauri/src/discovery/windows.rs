//! Windows-specific discovery inputs (development spec §2.1's platform split).
//!
//! npm's user-global prefix on Windows is `%APPDATA%\npm`, and a command
//! there appears as a `.cmd`/`.exe`/`.bat` shim — none of which `is_file()`
//! resolution on Unix would produce. Everything else about a DSH environment
//! (home shape, profiles, sessions) is read identically across platforms.

use std::path::PathBuf;

/// The file names a DSH command can appear as here. `.cmd` is what npm's
/// global install writes; the bare `dsh` is listed last as a long shot for
/// non-npm layouts.
pub(crate) fn executable_names() -> &'static [&'static str] {
    &["dsh.cmd", "dsh.exe", "dsh.bat", "dsh"]
}

/// Well-known DSH command locations beyond PATH. PATH under a GUI launch on
/// Windows usually includes `%APPDATA%\npm` already (user env is merged into
/// the session), but the prefix is added explicitly so a scan is not at the
/// mercy of how PHL was started.
pub(crate) fn extra_executable_roots() -> Vec<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return vec![PathBuf::from(appdata).join("npm")];
    }
    Vec::new()
}
