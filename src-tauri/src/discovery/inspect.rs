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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::candidate::{CandidateSource, Confidence};
use crate::launch::node_binary;

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

/// Count sessions under `<home>/sessions`: canonical artifacts
/// (`session[.vN].jsonl[.zstd]`, any format generation) inside
/// `<project>/<session>/` directories. A pure filename walk — the log content is
/// never read (spike §8: P0 must not take on the zstd/packed-row parsing
/// obligation).
///
/// The count is per session **directory**, not per file: a migrated session
/// keeps its older generation on disk beside the new one
/// (`session.jsonl.zstd` + `session.v3.jsonl.zstd`), and that is one
/// conversation, not two. Counting only `session.jsonl[.zstd]` undercounted
/// every home a current DSH has touched — the migration engine's blindness, in
/// its counting form.
pub(crate) fn count_sessions(home: &Path) -> usize {
    use crate::sessions::codec::{generation_of_filename, LogEncoding};
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut count = 0;
    for project in projects.flatten() {
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let Ok(files) = std::fs::read_dir(session.path()) else {
                continue;
            };
            let holds_a_session = files.flatten().any(|file| {
                let name = file.file_name().to_string_lossy().into_owned();
                generation_of_filename(&name, LogEncoding::Zstd).is_some()
                    || generation_of_filename(&name, LogEncoding::Plain).is_some()
            });
            if holds_a_session {
                count += 1;
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

/// The version a `node --version` reports, or `None` when the output is not a
/// bare `MAJOR.MINOR.PATCH`: shims and wrappers print their own things, and a
/// version nobody can parse must report nothing rather than noise.
pub(crate) fn parse_node_version(stdout: &str) -> Option<String> {
    let version = stdout.trim().strip_prefix('v')?;
    let ok =
        version.split('.').count() == 3 && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    ok.then(|| version.to_string())
}

/// `<candidate> --version`, run silently and *bounded*.
///
/// The bound is not decoration: this runs from launch and version-install paths,
/// which are async with no timeout above them, so a wedged binary (a wrapper
/// waiting on stdin) would park a runtime worker and hang the command. The
/// child is killed and reported unusable instead.
fn node_version_at(candidate: &Path, timeout: Duration) -> Option<String> {
    let mut command = std::process::Command::new(candidate);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Only console-subsystem binaries get a console window, and this probe
    // runs on every page load.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::launch::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            // A non-zero exit is not a Node we can run.
            Ok(Some(status)) if status.success() => {
                let mut text = String::new();
                child.stdout.take()?.read_to_string(&mut text).ok()?;
                return parse_node_version(&text);
            }
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                // Out of time, or the wait itself failed: leave no process.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The first root holding a `node` that exists *and* runs, in scan order.
///
/// Pure over an injected runner, the way `launch::registry::decide` is pure
/// over an injected probe: the scan is the part with behaviour worth pinning,
/// and driving it with fixture roots keeps a test from rewriting this
/// process's PATH — which every other test in this binary shares.
pub(crate) fn first_runnable_node(
    roots: &[PathBuf],
    version_of: &dyn Fn(&Path) -> Option<String>,
) -> Option<(PathBuf, String)> {
    for dir in roots {
        let candidate = dir.join(node_binary());
        if !candidate.is_file() {
            continue;
        }
        if let Some(version) = version_of(&candidate) {
            return Some((candidate, version));
        }
    }
    None
}

/// The real installation behind a resolved candidate path.
///
/// fnm and nvm both put a *shim* directory on PATH — `…/fnm_multishells/<id>/bin`
/// holds nothing but links, and it is destroyed with the shell that created it.
/// That path is worth neither reporting nor handing to `find_npm_cli`, which
/// looks for npm *beside* the binary it is given and would find a link farm
/// empty.
pub(crate) fn real_node_path(candidate: &Path) -> PathBuf {
    crate::paths::strip_verbatim(
        &std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf()),
    )
}

/// The system Node this machine has, as an absolute path plus the version it
/// reported — the one value every consumer of 「系统 Node」 must agree on.
///
/// PATH first, so a terminal launch resolves exactly what the shell would,
/// then the platform's well-known bin roots, because an app launched from
/// Finder or a desktop entry inherits launchd's PATH and cannot see the
/// toolchain a shell user has. The path is absolute and *verified* (that
/// binary answered `--version`), which is what lets the runtime page report
/// the version of the Node a launch will actually execute.
pub(crate) fn resolve_system_node() -> Option<(PathBuf, String)> {
    resolve_system_node_within(NODE_PROBE_TIMEOUT)
}

/// How long one candidate's `--version` may take. Long enough for a cold
/// binary on a busy disk, short enough that a wedged one cannot hold a launch.
pub(crate) const NODE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The resolution with an explicit probe budget (the seam its timeout test
/// needs; every production caller uses `resolve_system_node`).
pub(crate) fn resolve_system_node_within(timeout: Duration) -> Option<(PathBuf, String)> {
    let probe = move |candidate: &Path| node_version_at(candidate, timeout);
    let (path, version) = first_runnable_node(&executable_roots(), &probe)?;
    Some((real_node_path(&path), version))
}

/// `node` as this machine has it, with its `--version` (leading `v` stripped).
/// The path is the resolved absolute one — the same value a launch uses —
/// rather than a bare `node`, whose meaning is "whatever PATH resolves, if it
/// resolves at all": that name is exactly what a GUI launch cannot rely on.
pub(crate) fn probe_node() -> (Option<String>, Option<String>) {
    match resolve_system_node() {
        Some((path, version)) => (Some(path.to_string_lossy().into_owned()), Some(version)),
        None => (None, None),
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

/// `~/.nvm/versions/node/<v>/bin` for every version nvm has installed,
/// newest-first.
/// One bounded level (never recursed): an install listing, not a disk walk.
/// Lives here — rather than in each platform file — so tests can exercise it
/// on any host with a fixture directory; consumers are the Unix platform
/// modules, which is why it is dead on a non-test Windows build.
///
/// Sorted because the list is a *preference* order, not just a search space:
/// `resolve_system_node` takes the first runnable candidate, so on a machine
/// whose PATH has no `node` a readdir order would be what decides which Node an
/// instance runs and the page reports.
#[cfg(any(not(windows), test))]
pub(crate) fn nvm_bin_dirs(nvm_home: &Path) -> Vec<PathBuf> {
    let versions = nvm_home.join("versions").join("node");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };
    let mut dirs: Vec<(Option<semver::Version>, PathBuf)> = entries
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (version_of_dir_name(&name), e.path().join("bin"))
        })
        .filter(|(_, path)| path.is_dir())
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.into_iter().map(|(_, path)| path).collect()
}

/// The version a toolchain directory name spells, e.g. `v22.2.0` → `22.2.0`.
/// `None` for a name semver cannot read, which sorts it last in both
/// version-manager listings rather than letting it silently win the scan.
///
/// Gated like its only callers: a helper used solely by Unix-only (and
/// test-only) code is itself dead code on a Windows build, and `-D warnings`
/// says so.
#[cfg(any(not(windows), test))]
pub(crate) fn version_of_dir_name(name: &str) -> Option<semver::Version> {
    semver::Version::parse(name.trim_start_matches('v')).ok()
}

/// Node bin dirs under fnm's data directory: every alias first (they are the
/// user's own choice of default), then the installed versions newest-first.
///
/// fnm is the toolchain the well-known-bin list used to miss, and it is the
/// common one on macOS. It installs nothing into `/usr/local/bin` or `~/.nvm`:
/// it injects a per-shell shim directory into PATH, so a terminal resolves its
/// `node` while an app launched from Finder — which inherits launchd's PATH —
/// resolves none at all. Both data-dir conventions are probed because fnm
/// follows XDG on Linux and `~/Library/Application Support` on macOS.
///
/// Kept here beside `nvm_bin_dirs`, not in the platform files, so it is
/// testable on every host.
#[cfg(any(not(windows), test))]
pub(crate) fn fnm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let data_dirs = [
        home.join(".local").join("share").join("fnm"),
        home.join("Library").join("Application Support").join("fnm"),
    ];
    let mut aliases: Vec<PathBuf> = Vec::new();
    let mut versions: Vec<(Option<semver::Version>, PathBuf)> = Vec::new();
    for data in data_dirs {
        if let Ok(entries) = std::fs::read_dir(data.join("aliases")) {
            aliases.extend(entries.flatten().map(|e| e.path().join("bin")));
        }
        if let Ok(entries) = std::fs::read_dir(data.join("node-versions")) {
            versions.extend(entries.flatten().map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let version = version_of_dir_name(&name);
                (version, e.path().join("installation").join("bin"))
            }));
        }
    }
    aliases.sort();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    aliases
        .into_iter()
        .chain(versions.into_iter().map(|(_, path)| path))
        .filter(|path| path.is_dir())
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
            p
        };
        let first = s("--D-a--", "session-1");
        s("--D-a--", "session-2");
        s("--D-b--", "session-3");
        // A migrated session keeps its older generation beside the new one:
        // one conversation, not two.
        std::fs::write(first.join("session.v3.jsonl.zstd"), b"x").unwrap();
        // A newer generation without any v0 artifact counts too — a subagent
        // child's directory is its bare uuid, and a current DSH writes v3.
        let child = dir
            .join("sessions")
            .join("--D-b--")
            .join("3996cc7d-be3f-4a5a-bb24-93258dbe037c");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("session.v3.jsonl.zstd"), b"x").unwrap();
        // Junk that must not count.
        let other = dir.join("sessions").join("--D-c--").join("loose");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("sessions").join("session.jsonl"), b"x").unwrap();
        assert_eq!(count_sessions(&dir), 4);
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
    fn fnm_bin_dirs_puts_aliases_first_and_versions_newest_first() {
        let home = fixture("fnm");
        let data = home.join(".local").join("share").join("fnm");
        // v10 before v9 on purpose: descending *string* order puts `v9.0.0`
        // first, so this fixture only passes if the sort reads semver.
        for v in ["v22.2.0", "v26.3.0", "v9.0.0", "v10.0.0"] {
            std::fs::create_dir_all(
                data.join("node-versions")
                    .join(v)
                    .join("installation")
                    .join("bin"),
            )
            .unwrap();
        }
        // A version dir with no installation/bin is skipped, not guessed at.
        std::fs::create_dir_all(data.join("node-versions").join("v23.0.0")).unwrap();
        // A directory name semver cannot read must sort *last*, never first.
        let odd = data.join("node-versions").join("current");
        std::fs::create_dir_all(odd.join("installation").join("bin")).unwrap();
        std::fs::create_dir_all(data.join("aliases").join("default").join("bin")).unwrap();

        let found = fnm_bin_dirs(&home);
        assert_eq!(
            found.len(),
            6,
            "one alias + four versions + the unreadable name: {found:?}"
        );
        assert!(found[0].ends_with(Path::new("aliases/default/bin")));
        assert!(found[1].ends_with(Path::new("node-versions/v26.3.0/installation/bin")));
        assert!(found[2].ends_with(Path::new("node-versions/v22.2.0/installation/bin")));
        // v10 outranks v9 — the case a string sort gets backwards.
        assert!(found[3].ends_with(Path::new("node-versions/v10.0.0/installation/bin")));
        assert!(found[4].ends_with(Path::new("node-versions/v9.0.0/installation/bin")));
        assert!(found[5].ends_with(Path::new("node-versions/current/installation/bin")));
        assert!(fnm_bin_dirs(&home.join("nowhere")).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// nvm's listing feeds the same first-wins scan, so it has to be ordered
    /// too — readdir order would decide which Node an instance runs.
    #[test]
    fn nvm_bin_dirs_lists_versions_newest_first() {
        let dir = fixture("nvm-order");
        let versions = dir.join("versions").join("node");
        for v in ["v14.21.3", "v22.2.0", "v9.11.2", "v10.24.1"] {
            std::fs::create_dir_all(versions.join(v).join("bin")).unwrap();
        }
        let found = nvm_bin_dirs(&dir);
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["v22.2.0", "v14.21.3", "v10.24.1", "v9.11.2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The probe's budget is real: a binary that never answers is killed and
    /// reported unusable instead of holding the caller forever.
    #[cfg(unix)]
    #[test]
    fn a_wedged_node_binary_is_dropped_when_the_probe_budget_runs_out() {
        let dir = fixture("node-wedged");
        let fake = dir.join(node_binary());
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let started = Instant::now();
        assert_eq!(node_version_at(&fake, Duration::from_millis(250)), None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the probe must give up on its own budget, took {:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// …and a binary that answers normally still parses, so the bound above
    /// did not cost the happy path.
    #[cfg(unix)]
    #[test]
    fn a_real_version_string_survives_the_bounded_probe() {
        let dir = fixture("node-probe");
        let fake = dir.join(node_binary());
        std::fs::write(&fake, "#!/bin/sh\necho v22.11.0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            node_version_at(&fake, NODE_PROBE_TIMEOUT).as_deref(),
            Some("22.11.0")
        );
        // A binary that runs but prints something else is not a Node.
        std::fs::write(&fake, "#!/bin/sh\necho 'command not found'\n").unwrap();
        assert_eq!(node_version_at(&fake, NODE_PROBE_TIMEOUT), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_node_scan_stops_at_the_first_root_that_holds_a_runnable_node() {
        let dir = fixture("node-scan");
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        // Only `second` holds a binary; an empty dir must not answer.
        std::fs::write(second.join(node_binary()), "").unwrap();
        let version_of = |path: &Path| {
            Some(
                if path.starts_with(&second) {
                    "26.3.0"
                } else {
                    "1.0.0"
                }
                .to_string(),
            )
        };
        let hit = first_runnable_node(&[first.clone(), second.clone()], &version_of)
            .expect("second root holds a node");
        assert_eq!(hit.0, second.join(node_binary()));
        assert_eq!(hit.1, "26.3.0");

        // A candidate that exists but will not run is skipped, and an empty
        // scan is an honest `None` rather than a path nobody verified.
        std::fs::write(first.join(node_binary()), "").unwrap();
        let refuses = |_: &Path| None;
        assert!(first_runnable_node(&[first, second.clone()], &refuses).is_none());
        assert!(first_runnable_node(&[], &version_of).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_node_version_gates_shims_and_junk() {
        assert_eq!(parse_node_version("v22.11.0\n").as_deref(), Some("22.11.0"));
        // What a wrapper or a broken shim prints is not a version.
        assert_eq!(parse_node_version("node: command not found"), None);
        assert_eq!(parse_node_version("v22.11"), None);
        assert_eq!(parse_node_version("v22.11.0-rc.1"), None);
        assert_eq!(parse_node_version(""), None);
    }

    /// The shim dir a shell-based toolchain injects into PATH holds links, and
    /// `find_npm_cli` finds no npm beside a link.
    #[cfg(unix)]
    #[test]
    fn real_node_path_follows_a_shim_link_to_the_installation() {
        let dir = fixture("node-link");
        let real = dir.join("installation").join("bin").join("node");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "").unwrap();
        let shims = dir.join("multishells").join("1234").join("bin");
        std::fs::create_dir_all(&shims).unwrap();
        let shim = shims.join("node");
        std::os::unix::fs::symlink(&real, &shim).unwrap();

        assert_eq!(real_node_path(&shim), std::fs::canonicalize(&real).unwrap());
        // A path that cannot be resolved is reported as given, not dropped.
        let missing = dir.join("gone").join("node");
        assert_eq!(real_node_path(&missing), missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The 2026-09-22 review's repro, as a probe rather than an assertion CI
    /// can run: it needs a machine whose Node comes from a toolchain manager
    /// (fnm/nvm/Homebrew) and it rewrites this process's PATH, so it is
    /// `#[ignore]`d like the network probes. Run it explicitly:
    ///
    /// ```text
    /// cargo test --lib finder_launch -- --ignored --nocapture --test-threads=1
    /// ```
    #[test]
    #[ignore]
    fn finder_launch_still_resolves_the_system_node() {
        let before = std::env::var_os("PATH");
        // Exactly what a Finder-launched app inherits from launchd.
        std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        let found = resolve_system_node();
        match before {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }

        let (node, version) = found.expect("no node found with launchd's PATH");
        println!("launchd PATH → {} (v{version})", node.display());
        assert!(
            node.is_absolute(),
            "a bare name is what a GUI launch cannot use"
        );
        // The point of resolving through shim links: dependency installation
        // looks for npm *beside* the binary it is handed.
        assert!(
            crate::versions::dependencies::find_npm_cli(&node).is_some(),
            "the resolved node must carry its bundled npm"
        );
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
