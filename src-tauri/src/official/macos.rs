//! macOS half of the official-desktop singleton. Pure portable std — bundle
//! lookup, Info.plist scan, a `ps` snapshot — so test builds include it on
//! every non-macOS host (the discovery L-12 trick) and the parse logic is
//! unit-tested cross-host even though it only *runs* on macOS. Real-device
//! acceptance is still pending (planning doc §8.5).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Install, ProcRow};

/// The bundle name under `/Applications` (preferred) or `~/Applications`.
const APP_BUNDLE: &str = "DeepSeek Harness.app";

/// First hit wins: the machine singleton, system-wide preferred over the
/// user-local copy. A bundle whose `Contents/MacOS` is unreadable or empty is
/// not operable and does not count as installed.
pub(crate) fn discover() -> Result<Option<Install>, String> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    for root in roots {
        let bundle = root.join(APP_BUNDLE);
        let Some(main_exe) = bundle_executable(&bundle) else {
            continue;
        };
        let version = std::fs::read_to_string(bundle.join("Contents/Info.plist"))
            .ok()
            .and_then(|text| version_from_plist(&text));
        return Ok(Some(Install {
            root: bundle,
            main_exe,
            version,
        }));
    }
    Ok(None)
}

/// An Electron bundle ships exactly one executable in `Contents/MacOS`; the
/// first regular file there is it.
fn bundle_executable(bundle: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(bundle.join("Contents/MacOS")).ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_file())
}

/// `CFBundleShortVersionString` out of an Info.plist *text*. Binary plists
/// (and any layout this scan does not recognize) simply degrade to `None` —
/// the version display loses, nothing else does.
fn version_from_plist(text: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if !line.contains("<key>CFBundleShortVersionString</key>") {
            continue;
        }
        // The <string> follows the <key> inside the same dict.
        for line in lines.by_ref() {
            if line.contains("</dict>") {
                break;
            }
            let Some(start) = line.find("<string>") else {
                continue;
            };
            let rest = &line[start + "<string>".len()..];
            let Some(end) = rest.find("</string>") else {
                continue;
            };
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Every running process as `(pid, ppid, command path)`.
pub(crate) fn enumerate() -> Result<Vec<ProcRow>, String> {
    let output = Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,comm="])
        .output()
        .map_err(|e| format!("进程快照失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "进程快照失败: ps 退出码 {:?}",
            output.status.code()
        ));
    }
    let rows = parse_ps_rows(&String::from_utf8_lossy(&output.stdout));
    if rows.is_empty() {
        // `ps` on a live machine always lists processes; an empty parse means
        // the format drifted and no conclusion may be drawn from it.
        return Err("进程快照为空，无法确认官方桌面端状态".into());
    }
    Ok(rows)
}

/// Fixed columns first, the command path last — and possibly containing
/// spaces (`/Applications/DeepSeek Harness.app/Contents/MacOS/…`), so the
/// path is everything after the second whitespace run, never split further.
fn parse_ps_rows(text: &str) -> Vec<ProcRow> {
    text.lines().filter_map(parse_ps_row).collect()
}

fn parse_ps_row(line: &str) -> Option<ProcRow> {
    let line = line.trim_start();
    let (pid, rest) = line.split_once(char::is_whitespace)?;
    let (ppid, exe) = rest.trim_start().split_once(char::is_whitespace)?;
    Some(ProcRow {
        pid: pid.parse().ok()?,
        ppid: ppid.parse().ok()?,
        exe: exe.trim().to_string(),
    })
}

/// The polite quit: SIGTERM to the single pid. This is deliberately *not* the
/// process-group path (AGENTS.md) — the app was not launched by PHL, so PHL
/// cannot vouch for its group; a forced quit goes through
/// `terminate_tree`, which still only signals a group the pid itself leads.
pub(crate) async fn request_quit(pid: u32) -> Result<(), String> {
    let output = tokio::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()
        .await
        .map_err(|e| format!("发送退出请求失败: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    // The pid may have exited between the pick and this signal — that is the
    // quit we asked for, not a failure. A still-alive pid is a real refusal.
    if crate::launch::probe::probe_process(pid).state == crate::launch::probe::ProcessState::Exited
    {
        return Ok(());
    }
    Err(format!(
        "kill -TERM {pid} 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    ))
}

/// `open` hands the bundle to launchd the way Finder would: no env, no cwd,
/// and the app survives PHL quitting by construction.
pub(crate) async fn spawn_app(install: &Install) -> Result<(), String> {
    let output = tokio::process::Command::new("open")
        .arg(&install.root)
        .output()
        .await
        .map_err(|e| format!("启动官方桌面端失败: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "启动官方桌面端失败: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_rows_keep_paths_with_spaces_intact() {
        let rows = parse_ps_rows(
            "  101    75 /Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness\n\
             \x201024     1 /usr/sbin/cfprefsd\n\
             2050  101 /Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness Helper (Renderer)\n",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pid, 101);
        assert_eq!(rows[0].ppid, 75);
        assert_eq!(
            rows[0].exe,
            "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness"
        );
        assert_eq!(rows[1].exe, "/usr/sbin/cfprefsd");
        assert!(rows[2].exe.ends_with("Helper (Renderer)"));
    }

    #[test]
    fn malformed_ps_lines_are_skipped_not_fatal() {
        let rows = parse_ps_rows("not numbers here\n101 \n\n2050 101 /x/y\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 2050);
    }

    #[test]
    fn the_plist_version_is_the_string_after_the_key() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>com.deepseek.harness</string>
	<key>CFBundleShortVersionString</key>
	<string>0.2.0-rc.2</string>
	<key>CFBundleVersion</key>
	<string>20260929.1</string>
</dict>
</plist>
"#;
        assert_eq!(version_from_plist(plist).as_deref(), Some("0.2.0-rc.2"));
    }

    #[test]
    fn binary_or_unfamiliar_plists_yield_no_version() {
        assert_eq!(version_from_plist("bplist00\x01\x02garbage"), None);
        assert_eq!(
            version_from_plist("<key>Other</key>\n<string>1</string>"),
            None
        );
    }

    #[test]
    fn bundle_executable_needs_contents_macos_with_a_file() {
        let dir = std::env::temp_dir().join(format!("phl-official-bundle-{}", std::process::id()));
        let bundle = dir.join("DeepSeek Harness.app");
        let macos_dir = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::write(macos_dir.join("DeepSeek Harness"), b"MZ").unwrap();
        let found = bundle_executable(&bundle).expect("the executable is found");
        assert!(found.ends_with("DeepSeek Harness"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
