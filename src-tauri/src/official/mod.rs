//! The official DeepSeek Harness desktop app as a machine singleton PHL
//! helps operate: discover → show → launch → (confirm-then-force) quit.
//!
//! Scope (planning input 2026-09-29 §8, the user-approved v1): the official
//! app is *one* install per machine, self-updating on the official nightly
//! channel. PHL is a remote control, not a babysitter — it never writes into
//! the install or `~/.dsh`, never manages updates, never offers the official
//! version as an instance version source. Because the app self-updates under
//! PHL's feet, every fact is probed live on every call; nothing is cached.
//!
//! Fail-closed rules this module keeps:
//! - a probe that cannot be trusted reports "unknown" (`running: None`,
//!   status `unknown`), never a guess;
//! - before anything signals a pid, the pid's image path must still be the
//!   official executable — twice: once when picked from the snapshot, once
//!   immediately before the signal (pid reuse window);
//! - the polite quit only *requests* (WM_CLOSE / SIGTERM) and waits, because
//!   the official app may be running quit-inspection of its own (agents,
//!   jobs) that PHL cannot see; a forced kill needs the user's confirmation,
//!   which the frontend collects between the two outcomes.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(windows)]
pub(crate) mod windows;

// The macOS half is pure portable std (`ps`, filesystem), so test builds
// include it on every other host (the discovery L-12 trick): its parse logic
// is unit-tested cross-host even though it only *runs* on macOS. The Windows
// half is raw Win32 FFI and cannot compile elsewhere — its decisions are
// pulled into the pure helpers below instead, tested on both Rust CI
// platforms, with the FFI shell exercised by the Windows CI job.
#[cfg(all(test, not(target_os = "macos")))]
#[path = "macos.rs"]
#[allow(dead_code)]
mod macos_check;

use serde::Serialize;

#[cfg(any(windows, target_os = "macos"))]
use std::path::Path;

#[cfg(target_os = "macos")]
use self::macos as platform;
#[cfg(windows)]
use self::windows as platform;

// Used only by the gated snapshot logic below; ungated would warn on the
// platforms without official builds.
#[cfg(any(windows, target_os = "macos"))]
use crate::launch::probe::{probe_process, ProcessState};

/* ------------------------------ wire types ------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OfficialDesktopStatus {
    NotInstalled,
    /// Official desktop builds exist for Windows and macOS only; the feature
    /// says so instead of pretending to scan. Constructed by the platform
    /// stub commands below, which only exist where official builds don't —
    /// hence dead on the Windows/macOS lib builds.
    #[allow(dead_code)]
    Unsupported,
    Installed,
    /// Discovery itself could not be trusted (probe infrastructure failed).
    Unknown,
}

/// The live snapshot behind the 「官方桌面端」 card. Optional fields are the
/// fail-closed channel: `running: None` reads as 「无法确认」, not 「未运行」.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfficialDesktopInfo {
    pub status: OfficialDesktopStatus,
    pub root: Option<String>,
    pub main_exe: Option<String>,
    pub version: Option<String>,
    pub running: Option<bool>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QuitOutcome {
    /// An exit we observed, not one we assume.
    Exited,
    /// Still alive after the polite request; the frontend must ask the user
    /// before coming back with `force`.
    StillRunning,
}

/* --------------------------- singleton snapshot -------------------------- */

/// The discovered official install. `root` is the install directory on
/// Windows and the `.app` bundle on macOS; `main_exe` is the real executable
/// every identity check compares against.
#[derive(Debug, Clone)]
pub(crate) struct Install {
    pub root: std::path::PathBuf,
    pub main_exe: std::path::PathBuf,
    pub version: Option<String>,
}

/// One row of a process snapshot. `exe` is the image file name on Windows
/// (Toolhelp carries no path) and the full command path on macOS (`ps comm`).
#[derive(Debug, Clone)]
pub(crate) struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
}

/// The Electron main process picked out of a snapshot. `Ambiguous` is a
/// refusal, not an error message: two same-named mains means PHL cannot know
/// which one to signal, so nothing gets signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainPick {
    None,
    One(u32),
    Ambiguous,
}

/* ------------------------- pure selection logic -------------------------- */

/// The Electron main process is the one whose parent is not itself part of
/// the official-app set: every `--type=` child is parented by the main
/// process, so no command-line parsing is needed. Two degenerate cases and
/// where they land: a same-named binary from another location becomes a
/// second "main" (`Ambiguous` — refuse), and a crashed main leaves children
/// parented outside the set, one of which would be picked — the full-path
/// identity gate before any signal keeps that honest.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn pick_main_process(rows: &[ProcRow], main_exe: &Path) -> MainPick {
    let members: Vec<u32> = rows
        .iter()
        .filter(|r| exe_file_name_matches(&r.exe, main_exe))
        .map(|r| r.pid)
        .collect();
    let mains: Vec<u32> = rows
        .iter()
        .filter(|r| exe_file_name_matches(&r.exe, main_exe) && !members.contains(&r.ppid))
        .map(|r| r.pid)
        .collect();
    match mains.as_slice() {
        [] => MainPick::None,
        [pid] => MainPick::One(*pid),
        _ => MainPick::Ambiguous,
    }
}

/// Snapshot rows carry the image name (Windows) or a full path (macOS);
/// compare on the file-name level here, and on the *full* path via
/// [`same_path`] wherever the comparison decides who gets signalled.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn exe_file_name_matches(row_exe: &str, main_exe: &Path) -> bool {
    let Some(expected) = main_exe.file_name() else {
        return false;
    };
    let Some(actual) = Path::new(row_exe).file_name() else {
        return false;
    };
    expected
        .to_string_lossy()
        .eq_ignore_ascii_case(&actual.to_string_lossy())
}

/// Windows paths compare case- and slash-insensitively; macOS paths arrive
/// canonicalized from the kernel probe, but the same normalization is
/// harmless there.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn same_path(candidate: &str, expected: &Path) -> bool {
    let normalize = |s: &str| s.replace('/', "\\").to_lowercase();
    normalize(candidate) == normalize(&expected.to_string_lossy())
}

/* ------------------------------- commands -------------------------------- */

/// Live snapshot of the official desktop install. Read-only, nothing cached:
/// the official nightly channel may have replaced the install since the last
/// poll, and the card must never show a version that no longer exists.
#[cfg(any(windows, target_os = "macos"))]
#[tauri::command]
pub async fn inspect_official_desktop() -> OfficialDesktopInfo {
    match inspect_snapshot().await {
        Ok(info) => info,
        Err(e) => {
            eprintln!("[phl] official desktop inspect failed: {e}");
            OfficialDesktopInfo {
                status: OfficialDesktopStatus::Unknown,
                root: None,
                main_exe: None,
                version: None,
                running: None,
                pid: None,
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[tauri::command]
pub async fn inspect_official_desktop() -> OfficialDesktopInfo {
    OfficialDesktopInfo {
        status: OfficialDesktopStatus::Unsupported,
        root: None,
        main_exe: None,
        version: None,
        running: None,
        pid: None,
    }
}

#[cfg(any(windows, target_os = "macos"))]
async fn inspect_snapshot() -> Result<OfficialDesktopInfo, String> {
    tokio::task::spawn_blocking(|| {
        let install = platform::discover()?;
        let Some(install) = install else {
            return Ok(OfficialDesktopInfo {
                status: OfficialDesktopStatus::NotInstalled,
                root: None,
                main_exe: None,
                version: None,
                running: None,
                pid: None,
            });
        };
        let (running, pid) = match running_pid(&install) {
            Ok(Some(pid)) => (Some(true), Some(pid)),
            Ok(None) => (Some(false), None),
            // Installed, but the running probe cannot be trusted: show the
            // install facts and admit the state is unknown. Defaulting to
            // "not running" would invite a second launch.
            Err(_) => (None, None),
        };
        Ok(OfficialDesktopInfo {
            status: OfficialDesktopStatus::Installed,
            root: Some(install.root.to_string_lossy().into_owned()),
            main_exe: Some(install.main_exe.to_string_lossy().into_owned()),
            version: install.version,
            running,
            pid,
        })
    })
    .await
    .map_err(|e| format!("官方桌面端探测失败: {e}"))?
}

/// Launch the official app as-is: no environment injection, no working
/// directory games, no flags. PHL drives the user's real daily app, not a
/// sandbox copy. A double click stays harmless — the official shell's
/// single-instance lock focuses the existing window instead of starting a
/// second app.
#[cfg(any(windows, target_os = "macos"))]
#[tauri::command]
pub async fn launch_official_desktop() -> Result<(), String> {
    let (install, running) = tokio::task::spawn_blocking(|| -> Result<(Install, bool), String> {
        let install = platform::discover()?.ok_or("未检测到官方桌面端安装")?;
        let running = running_pid(&install)?.is_some();
        Ok((install, running))
    })
    .await
    .map_err(|e| format!("官方桌面端探测失败: {e}"))??;
    if running {
        return Err("官方桌面端已在运行，无需重复启动".into());
    }
    platform::spawn_app(&install).await
}

#[cfg(not(any(windows, target_os = "macos")))]
#[tauri::command]
pub async fn launch_official_desktop() -> Result<(), String> {
    Err("官方桌面端不支持当前平台".into())
}

/// How long a politely-asked official app gets before the answer becomes
/// "still running". 0.2.0-rc.2 on Windows honors WM_CLOSE but has been
/// observed taking longer than 8 s to be fully gone, and it may instead be
/// showing its own quit-inspection dialog (running agents, scheduled jobs)
/// that only the user can answer — PHL must not out-shout either with a
/// force kill.
#[cfg(any(windows, target_os = "macos"))]
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

/// How long to wait before re-reading an ambiguous snapshot. A main that is
/// mid-shutdown leaves orphaned children that each *look* like a main until
/// they die; one remnant window later the set usually collapses to nothing.
#[cfg(any(windows, target_os = "macos"))]
const REMNANT_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(any(windows, target_os = "macos"))]
#[tauri::command]
pub async fn quit_official_desktop(force: bool) -> Result<QuitOutcome, String> {
    // Re-probe live; a pid remembered from the UI's last poll is exactly how
    // the wrong process gets signalled.
    let target = tokio::task::spawn_blocking(|| {
        let install = platform::discover()?.ok_or("未检测到官方桌面端安装")?;
        running_pid(&install).map(|pid| pid.map(|pid| (install, pid)))
    })
    .await
    .map_err(|e| format!("官方桌面端探测失败: {e}"))??;
    let Some((install, pid)) = target else {
        // Nothing there: an exit we have observed, not a fake success.
        return Ok(QuitOutcome::Exited);
    };
    // Identity gate #2, as close to the signal as possible: the pid picked
    // from the snapshot could have exited and been reused in between.
    let probe = probe_process(pid);
    if probe.state != ProcessState::Alive
        || !probe
            .exe_path
            .as_deref()
            .is_some_and(|p| same_path(p, &install.main_exe))
    {
        return Err("官方桌面端进程身份无法确认，已拒绝退出操作".into());
    }
    if force {
        // taskkill /T /F on Windows; on Unix the group path only when the pid
        // itself leads a group — `terminate_tree` already encodes that
        // caution, and an app PHL did not spawn usually takes the
        // direct-child fallback.
        crate::launch::process::terminate_tree(pid, crate::launch::process::TERM_GRACE).await?;
    } else {
        platform::request_quit(pid).await?;
    }
    let gone = crate::launch::process::confirm_exit(|| probe_process(pid).state, QUIT_GRACE).await;
    Ok(if gone {
        QuitOutcome::Exited
    } else {
        QuitOutcome::StillRunning
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
#[tauri::command]
pub async fn quit_official_desktop(_force: bool) -> Result<QuitOutcome, String> {
    Err("官方桌面端不支持当前平台".into())
}

/* ------------------------------ live probe ------------------------------- */

/// Whether the official app is running, with the full-path identity check
/// that turns "a process named like ours" into "the official app".
/// `Ok(None)` = not running; `Ok(Some(pid))` = running, identity verified;
/// `Err` = cannot determine — never guess.
#[cfg(any(windows, target_os = "macos"))]
fn running_pid(install: &Install) -> Result<Option<u32>, String> {
    // Identity confirmed against the install's own executable path; a
    // different binary merely named like ours (or one that died between
    // snapshot and probe) reads as not running.
    fn verified(pid: u32, main_exe: &Path) -> Result<Option<u32>, String> {
        let probe = probe_process(pid);
        match (probe.state, probe.exe_path.as_deref()) {
            (ProcessState::Alive, Some(path)) if same_path(path, main_exe) => Ok(Some(pid)),
            (ProcessState::Alive, Some(_)) | (ProcessState::Exited, _) => Ok(None),
            _ => Err("进程身份探测失败，无法确认官方桌面端是否在运行".into()),
        }
    }

    let rows = platform::enumerate()?;
    let main_exe = install.main_exe.clone();
    match pick_main_process(&rows, &main_exe) {
        MainPick::None => Ok(None),
        MainPick::One(pid) => verified(pid, &main_exe),
        MainPick::Ambiguous => {
            // Wait out one remnant window before refusing: a main that is
            // mid-shutdown leaves orphaned children which each look like a
            // main until they die — a shutdown race collapses to nothing,
            // while two genuinely independent instances stay ambiguous and
            // get the refusal.
            std::thread::sleep(REMNANT_WINDOW);
            let rows = platform::enumerate()?;
            match pick_main_process(&rows, &main_exe) {
                MainPick::None => Ok(None),
                MainPick::One(pid) => verified(pid, &main_exe),
                MainPick::Ambiguous => {
                    Err("检测到多个官方桌面端主进程，无法确认哪一个在运行".into())
                }
            }
        }
    }
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, exe: &str) -> ProcRow {
        ProcRow {
            pid,
            ppid,
            exe: exe.to_string(),
        }
    }

    fn main_exe(name: &str) -> std::path::PathBuf {
        // A platform-neutral directory keeps `file_name` behaving identically
        // on Windows and macOS test runs.
        std::path::Path::new("/any/install").join(name)
    }

    #[test]
    fn the_main_process_is_the_one_whose_parent_is_not_the_app() {
        let main = main_exe("DeepSeek Harness.exe");
        let rows = vec![
            row(100, 1, "DeepSeek Harness.exe"),   // main, parent is explorer
            row(101, 100, "DeepSeek Harness.exe"), // renderer
            row(102, 100, "DeepSeek Harness.exe"), // gpu
        ];
        assert_eq!(pick_main_process(&rows, &main), MainPick::One(100));
    }

    #[test]
    fn a_crashed_main_leaves_no_signalable_root() {
        // Main (100) died: its children are now parented outside the set and
        // every one of them qualifies as a "main" — that ambiguity is a
        // refusal, not a guess.
        let main = main_exe("DeepSeek Harness.exe");
        let rows = vec![
            row(101, 100, "DeepSeek Harness.exe"),
            row(102, 100, "DeepSeek Harness.exe"),
        ];
        assert_eq!(pick_main_process(&rows, &main), MainPick::Ambiguous);
    }

    #[test]
    fn two_independent_app_instances_are_ambiguous() {
        let main = main_exe("DeepSeek Harness.exe");
        let rows = vec![
            row(100, 1, "DeepSeek Harness.exe"),
            row(200, 1, "DeepSeek Harness.exe"),
        ];
        assert_eq!(pick_main_process(&rows, &main), MainPick::Ambiguous);
    }

    #[test]
    fn foreign_processes_are_none() {
        let main = main_exe("DeepSeek Harness.exe");
        let rows = vec![row(100, 1, "explorer.exe"), row(200, 1, "Code.exe")];
        assert_eq!(pick_main_process(&rows, &main), MainPick::None);
    }

    #[test]
    fn file_name_match_takes_both_row_shapes() {
        let windows_main = main_exe("DeepSeek Harness.exe");
        assert!(exe_file_name_matches("DeepSeek Harness.exe", &windows_main));
        assert!(!exe_file_name_matches(
            "DeepSeek Harness Helper.exe",
            &windows_main
        ));

        let mac_main = main_exe("DeepSeek Harness");
        assert!(exe_file_name_matches(
            "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
            &mac_main
        ));
    }

    #[test]
    fn same_path_ignores_case_and_separator_direction() {
        let expected = std::path::Path::new("D:\\dsh\\DeepSeek Harness.exe");
        assert!(same_path("d:/dsh/DeepSeek Harness.exe", expected));
        assert!(same_path("D:\\DSH\\deepseek harness.EXE", expected));
        assert!(!same_path("D:\\other\\DeepSeek Harness.exe", expected));
    }
}
