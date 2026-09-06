//! Linux-specific discovery inputs (development spec §2.1's platform split).
//!
//! Same rationale as macOS: a desktop-entry launch goes through the systemd
//! user session, whose PATH is not the login shell's — well-known install
//! bins are probed explicitly. Flatpak/snap confinement redirects `$HOME`
//! and is deliberately out of scope (a scan must not invent homes inside
//! sandbox trees; the user can always point at one manually, spec §2.2).

use std::path::PathBuf;

use super::inspect::nvm_bin_dirs;

/// Unix: npm's global install drops a bare `dsh` shim in the prefix's bin.
pub(crate) fn executable_names() -> &'static [&'static str] {
    &["dsh"]
}

/// Well-known DSH command locations beyond the session PATH.
pub(crate) fn extra_executable_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        // Distro-neutral and Homebrew-on-Linux locations; `/usr/bin` is in
        // every default PATH but naming it costs one stat and closes the
        // "custom minimal session PATH" hole.
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
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
    fn roots_cover_the_standard_local_bins() {
        let roots = extra_executable_roots();
        assert!(roots.contains(&PathBuf::from("/usr/local/bin")));
        assert!(roots.contains(&PathBuf::from("/usr/bin")));
    }
}
