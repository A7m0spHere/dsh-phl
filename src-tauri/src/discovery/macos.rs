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

use super::inspect::{fnm_bin_dirs, nvm_bin_dirs};

/// Unix: npm's global install drops a bare `dsh` shim in the prefix's bin.
pub(crate) fn executable_names() -> &'static [&'static str] {
    &["dsh"]
}

/// Well-known DSH command locations beyond the (GUI-restricted) PATH.
///
/// The order is a preference, and it puts the *user's own* toolchains first: a
/// version manager's bin directory is an explicit "this is my node", while a
/// package manager's copy is incidental — Homebrew's `node` ships without npm
/// on some installs, so letting `/opt/homebrew/bin` win would hand a launch a
/// Node that cannot install anything. fnm leads the user group because its
/// binaries live *only* here: it injects a per-shell shim dir into PATH instead
/// of linking into a well-known bin (`fnm_bin_dirs`).
pub(crate) fn extra_executable_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![
            // Homebrew: /usr/local (Intel) and /opt/homebrew (Apple Silicon).
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ];
    };
    let mut roots = fnm_bin_dirs(&home);
    roots.extend(nvm_bin_dirs(&home.join(".nvm")));
    roots.push(home.join(".volta").join("bin"));
    roots.push(home.join(".bun").join("bin"));
    roots.push(home.join(".npm-global").join("bin"));
    // pnpm's macOS global bin: `pnpm add -g` lands here, not in ~/.local.
    roots.push(home.join("Library").join("pnpm"));
    roots.push(home.join(".local").join("bin"));
    // System-wide package managers last.
    roots.push(PathBuf::from("/usr/local/bin"));
    roots.push(PathBuf::from("/opt/homebrew/bin"));
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
