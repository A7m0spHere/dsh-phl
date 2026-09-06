//! macOS-specific discovery inputs (development spec §2.1's platform split).
//!
//! The macOS facts that matter here: an app launched from Finder inherits
//! launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) — NOT the user's shell
//! PATH — so the npm/Homebrew bin dirs a terminal user would have on PATH
//! must be probed explicitly. These locations are the standard, documented
//! prefixes of the node toolchains DSH is typically installed with (npm -g
//! default, Homebrew, nvm, Volta, Bun, ~/.local); nothing here guesses a DSH
//! *data* path beyond the `~/.dsh` default, which is identical to every
//! other platform's (`dirs::home_dir`).

use std::path::PathBuf;

use super::inspect::nvm_bin_dirs;

/// Unix: npm's global install drops a bare `dsh` shim in the prefix's bin.
pub(crate) fn executable_names() -> &'static [&'static str] {
    &["dsh"]
}

/// Well-known DSH command locations beyond the (GUI-restricted) PATH.
pub(crate) fn extra_executable_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        // Homebrew: /usr/local (Intel) and /opt/homebrew (Apple Silicon).
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".npm-global").join("bin"));
        roots.push(home.join(".volta").join("bin"));
        roots.push(home.join(".bun").join("bin"));
        roots.push(home.join(".local").join("bin"));
        roots.extend(nvm_bin_dirs(&home.join(".nvm")));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_unix_shim() {
        assert_eq!(executable_names(), &["dsh"]);
    }

    #[test]
    fn roots_include_the_bins_a_gui_launch_cannot_see_on_path() {
        let roots = extra_executable_roots();
        assert!(
            roots.contains(&PathBuf::from("/usr/local/bin")),
            "Homebrew Intel"
        );
        assert!(
            roots.contains(&PathBuf::from("/opt/homebrew/bin")),
            "Homebrew Apple Silicon"
        );
    }
}
