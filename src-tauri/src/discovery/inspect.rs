//! Turning a directory into evidence about a DSH environment.
//!
//! Every fact here is *read*, never assumed — the shape of the checks comes
//! from the Session Spike (`docs/research/dsh-session-integration.md`),
//! which verified these markers against two installed DSH versions and real
//! homes (`~/.dsh`, a PHL instance home). All functions are synchronous
//! filesystem work, run from the command handlers on a blocking thread.
//!
//! Deliberate limits (spec §2.2, spike §8): no full-disk scan, no zstd
//! decoding (session counts are a filename walk), no running the discovered
//! executable (version comes from files beside it).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::candidate::{CandidateSource, Confidence};

/// The harness's own bundle scope. Entries inside it are the product, not
/// user plugins, and are filtered out of every plugin count.
pub(crate) const CORE_SCOPE: &str = "@deepseek-ai/";

/// Minimum a directory must show to be a DSH home at all: a settings file
/// beside a profiles tree. Weaker combinations report `Low`/`Medium` with
/// explicit warnings so the UI can say what was found; only the complete
/// pair is `High`.
pub(crate) fn classify_home(dir: &Path) -> Confidence {
    if !dir.is_dir() {
        return Confidence::Invalid;
    }
    let settings = dir.join("settings.yaml").is_file();
    let profiles = dir.join("profiles").is_dir();
    let sessions = dir.join("sessions").is_dir();
    let storages = dir.join("storages").is_dir();
    match (settings, profiles) {
        (true, true) => Confidence::High,
        (true, false) | (false, true) if sessions || storages => Confidence::Medium,
        (true, false) | (false, true) => Confidence::Low,
        (false, false) if sessions || storages => Confidence::Low,
        _ => Confidence::Invalid,
    }
}

/// The profile to inspect inside a home: `web` when present (the profile PHL
/// launches every instance with), else the first plain profile directory.
/// Returns the profile name and its directory path.
pub(crate) fn pick_profile(home: &Path) -> Option<(String, PathBuf)> {
    let profiles = home.join("profiles");
    let mut fallback: Option<(String, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&profiles) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // `node_modules` beside profile dirs is the shared bundle store,
            // not a profile.
            if name == "node_modules" || name.starts_with('.') {
                continue;
            }
            if name == "web" {
                return Some((name, path));
            }
            let replace = fallback.as_ref().map(|(n, _)| name < *n).unwrap_or(true);
            if replace {
                fallback = Some((name, path));
            }
        }
    }
    fallback
}

/// Count session artifacts under `<home>/sessions`: files named
/// `session.jsonl` or `session.jsonl.zstd` inside `<project>/<session>/`
/// directories. A pure filename walk — the log content is never read
/// (spike §8: P0 must not take on the zstd/packed-row parsing obligation).
pub(crate) fn count_sessions(home: &Path) -> usize {
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut count = 0;
    for project in projects.flatten() {
        let project_path = project.path();
        let Ok(sessions) = std::fs::read_dir(&project_path) else {
            continue;
        };
        for session in sessions.flatten() {
            let session_path = session.path();
            let Ok(files) = std::fs::read_dir(&session_path) else {
                continue;
            };
            for file in files.flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if name == "session.jsonl" || name == "session.jsonl.zstd" {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Plugins declared through DSH's own two registration channels:
/// `dsh.profile.bundles` in the profile's `package.json`, and top-level
/// `- id:` entries in `cordis.patch.yml` (PHL's channel). Core-scope bundles
/// are the product itself and are excluded. A name is only counted when its
/// package is actually present under the profile's `node_modules` —
/// a declared-but-missing bundle is a warning, not a plugin.
///
/// `cordis.patch.yml` is only read with a line-level scan: it can contain
/// `!!js` tags that a YAML parser rejects, and entry ids are line-shaped by
/// construction in every writer this project has observed.
pub(crate) fn declared_plugins(profile_dir: &Path) -> Result<(usize, Vec<String>), String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut warnings = Vec::new();

    let pkg_path = profile_dir.join("package.json");
    if let Ok(raw) = std::fs::read_to_string(&pkg_path) {
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("profile package.json 解析失败: {e}"))?;
        if let Some(bundles) = value
            .pointer("/dsh/profile/bundles")
            .and_then(|v| v.as_array())
        {
            for bundle in bundles {
                if let Some(name) = bundle.as_str() {
                    declared.insert(name.to_string());
                }
            }
        }
    }

    if let Ok(patch) = std::fs::read_to_string(profile_dir.join("cordis.patch.yml")) {
        for line in patch.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("- id:") {
                let id = rest.trim().trim_matches('"').trim_matches('\'');
                if !id.is_empty() {
                    declared.insert(id.to_string());
                }
            }
        }
    }

    let node_modules = profile_dir.join("node_modules");
    let mut installed = 0;
    for name in declared {
        if name.starts_with(CORE_SCOPE) {
            continue;
        }
        if node_modules.join(&name).exists() {
            installed += 1;
        } else {
            warnings.push(format!("插件 {name} 已声明但 node_modules 中不存在"));
        }
    }
    Ok((installed, warnings))
}

/// The harness version this environment boots: `@deepseek-ai/dsh-base`'s
/// package.json, looked for in the profile's own store first, then the
/// shared `profiles/node_modules` store. `None` means no version evidence,
/// not "unknown version" — callers add the warning.
pub(crate) fn detect_version(home: &Path, profile_dir: Option<&Path>) -> Option<String> {
    let candidates = [
        profile_dir.map(|p| p.join("node_modules")),
        Some(home.join("profiles").join("node_modules")),
    ];
    for base in candidates.into_iter().flatten() {
        let pkg = base
            .join("@deepseek-ai")
            .join("dsh-base")
            .join("package.json");
        if let Ok(raw) = std::fs::read_to_string(pkg) {
            if let Some(v) = read_package_version(&raw) {
                return Some(v);
            }
        }
    }
    None
}

fn read_package_version(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

/// `node` as found on PATH, with its `--version` (leading `v` stripped).
/// The `MAJOR.MINOR.PATCH` gate mirrors `runtimes::system_node_version`:
/// shims that print anything else report nothing rather than noise.
pub(crate) fn probe_node() -> (Option<String>, Option<String>) {
    let output = std::process::Command::new("node")
        .arg("--version")
        .output()
        .ok();
    let Some(output) = output else {
        return (None, None);
    };
    if !output.status.success() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let Some(version) = text.strip_prefix('v') else {
        return (None, None);
    };
    let ok =
        version.split('.').count() == 3 && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    if ok {
        // Report the command name, not a resolved absolute path: PATH entries
        // can be shims, and `node` is what would actually run.
        (Some("node".to_string()), Some(version.to_string()))
    } else {
        (None, None)
    }
}

#[cfg(target_os = "linux")]
use super::linux as platform;
#[cfg(target_os = "macos")]
use super::macos as platform;
/// The platform module behind the name-specific bits of a command scan
/// (spec §2.1's windows/macos/linux split). `nvm_bin_dirs` is kept here,
/// not in the platform files, so it is testable on every host.
#[cfg(windows)]
use super::windows as platform;

/// Locations that can hold a DSH command, in scan order: PATH first (a
/// terminal user's install is already on it), then the platform module's
/// well-known bin roots — a Finder/desktop-launched app does NOT inherit the
/// shell PATH, so on macOS/Linux those dirs are how a global npm install is
/// reachable at all. Nothing here walks a whole drive.
pub(crate) fn executable_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        roots.extend(std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()));
    }
    roots.extend(platform::extra_executable_roots());
    roots
}

/// The file names a DSH command can appear as on this platform.
fn executable_names() -> &'static [&'static str] {
    platform::executable_names()
}

/// `~/.nvm/versions/node/<v>/bin` for every version nvm has installed.
/// One bounded level (never recursed): an install listing, not a disk walk.
/// Lives here — rather than in each platform file — so tests can exercise it
/// on any host with a fixture directory; consumers are the Unix platform
/// modules, which is why it is dead on a non-test Windows build.
#[cfg(any(not(windows), test))]
pub(crate) fn nvm_bin_dirs(nvm_home: &Path) -> Vec<PathBuf> {
    let versions = nvm_home.join("versions").join("node");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path().join("bin"))
        .filter(|p| p.is_dir())
        .collect()
}

/// First DSH command found across the roots, if any.
pub(crate) fn find_dsh_executable() -> Option<PathBuf> {
    for dir in executable_roots() {
        for name in executable_names() {
            let hit = dir.join(name);
            if hit.is_file() {
                return Some(hit);
            }
        }
    }
    None
}

/// The DSH home an environment *would* resolve to, mirroring dsh-home-paths
/// precedence: configured > `$DSH_HOME` (blank = unset) > `~/.dsh`. Returns
/// the scan source alongside so candidates keep provenance.
pub(crate) fn default_homes() -> Vec<(PathBuf, CandidateSource)> {
    let mut out = Vec::new();
    if let Some(env_home) = std::env::var_os("DSH_HOME") {
        let trimmed = env_home.to_string_lossy().trim().to_string();
        if !trimmed.is_empty() {
            out.push((PathBuf::from(trimmed), CandidateSource::Env));
        }
    }
    if let Some(home) = dirs::home_dir() {
        out.push((home.join(".dsh"), CandidateSource::DefaultHome));
    }
    out
}

/// The `~/.dsh` display spelling reused for default-home candidates: the
/// symbolic form whenever the path actually is under the OS home.
pub(crate) fn symbolic_display(home: &Path) -> Option<String> {
    let home_dir = dirs::home_dir()?;
    let rest = home.strip_prefix(home_dir).ok()?;
    let rest = rest.to_string_lossy().replace('\\', "/");
    Some(format!("~/{rest}"))
}

/// One-size walk for the adoption preview's cost estimate. Symlinks are not
/// followed (they'd re-enter the tree on linked plugins); the copy path
/// refuses symlinks anyway, so under-counting a copy-hostile tree is safe
/// in the honest direction.
pub(crate) fn tree_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            total += tree_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-discovery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn classification_needs_structure_not_names() {
        let dir = fixture("classify");
        assert_eq!(classify_home(&dir), Confidence::Invalid);
        std::fs::create_dir_all(dir.join("random")).unwrap();
        assert_eq!(classify_home(&dir), Confidence::Invalid);
        std::fs::write(dir.join("settings.yaml"), "ui: {}\n").unwrap();
        assert_eq!(classify_home(&dir), Confidence::Low);
        std::fs::create_dir_all(dir.join("profiles")).unwrap();
        assert_eq!(classify_home(&dir), Confidence::High);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_are_counted_by_filename_walk() {
        let dir = fixture("sessions");
        let s = |project: &str, name: &str| {
            let p = dir.join("sessions").join(project).join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("session.jsonl.zstd"), b"x").unwrap();
        };
        s("--D-a--", "session-1");
        s("--D-a--", "session-2");
        s("--D-b--", "session-3");
        // Junk that must not count.
        let other = dir.join("sessions").join("--D-c--").join("loose");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("sessions").join("session.jsonl"), b"x").unwrap();
        assert_eq!(count_sessions(&dir), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugins_merge_bundles_and_patch_excluding_core() {
        let dir = fixture("plugins");
        let profile = dir.join("profiles").join("web");
        std::fs::create_dir_all(profile.join("node_modules").join("dsh-balance")).unwrap();
        std::fs::create_dir_all(profile.join("node_modules").join("@dsh-external")).unwrap();
        std::fs::create_dir_all(
            profile
                .join("node_modules")
                .join("@dsh-external")
                .join("my-plugin"),
        )
        .unwrap();
        std::fs::write(
            profile.join("package.json"),
            r#"{"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app","dsh-balance","dsh-missing","@dsh-external/my-plugin"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            profile.join("cordis.patch.yml"),
            "# comment with - id: not-a-real-line\n- id: dshmarket\n  name: dshmarket\n- id: plain-config\n",
        )
        .unwrap();
        // dshmarket declared only in the patch and absent from disk: warning.
        // plain-config is a config-only patch entry — same treatment; the
        // count is honest about what resolves to a package.
        let (count, warnings) = declared_plugins(&profile).unwrap();
        // dsh-balance and @dsh-external/my-plugin resolve to packages; the
        // three declared-only names are reported as missing, not counted.
        assert_eq!(count, 2, "dsh-balance + @dsh-external/my-plugin");
        assert_eq!(warnings.len(), 3);
        assert!(warnings.iter().any(|w| w.contains("dshmarket")));
        assert!(warnings.iter().any(|w| w.contains("plain-config")));
        assert!(warnings.iter().any(|w| w.contains("dsh-missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_reads_dsh_base_from_either_store() {
        let dir = fixture("version");
        assert_eq!(detect_version(&dir, None), None);
        let shared = dir
            .join("profiles")
            .join("node_modules")
            .join("@deepseek-ai");
        std::fs::create_dir_all(shared.join("dsh-base")).unwrap();
        std::fs::write(
            shared.join("dsh-base").join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-base","version":"0.9.9-test"}"#,
        )
        .unwrap();
        assert_eq!(detect_version(&dir, None).as_deref(), Some("0.9.9-test"));
        // A profile-local store wins over the shared one.
        let local = dir
            .join("profiles")
            .join("web")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-base");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("package.json"), r#"{"version":"1.0.0-local"}"#).unwrap();
        assert_eq!(
            detect_version(&dir, Some(&dir.join("profiles").join("web"))).as_deref(),
            Some("1.0.0-local")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nvm_bin_dirs_enumerates_installed_version_bins_one_level_deep() {
        let dir = fixture("nvm");
        let versions = dir.join("versions").join("node");
        for v in ["v20.11.0", "v22.2.0"] {
            std::fs::create_dir_all(versions.join(v).join("bin")).unwrap();
        }
        // A half-installed version dir (no bin) is skipped, not guessed at.
        std::fs::create_dir_all(versions.join("v23.0.0")).unwrap();
        let found = nvm_bin_dirs(&dir);
        assert_eq!(found.len(), 2, "only complete bin dirs: {found:?}");
        assert!(nvm_bin_dirs(&dir.join("nowhere")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_selection_prefers_web_but_skips_stores() {
        let dir = fixture("profile");
        assert!(pick_profile(&dir).is_none());
        std::fs::create_dir_all(dir.join("profiles").join("node_modules")).unwrap();
        std::fs::create_dir_all(dir.join("profiles").join("zeta")).unwrap();
        let (name, _) = pick_profile(&dir).unwrap();
        assert_eq!(name, "zeta");
        std::fs::create_dir_all(dir.join("profiles").join("web")).unwrap();
        let (name, _) = pick_profile(&dir).unwrap();
        assert_eq!(name, "web");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
