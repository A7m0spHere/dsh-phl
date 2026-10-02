//! Windows half of the official-desktop singleton: uninstall-registry
//! discovery (raw advapi32, the house FFI style — no registry crate) and a
//! Toolhelp32 process snapshot. Only this file may touch Win32; everything it
//! *decides* goes through the pure helpers in `super`, and every pid is
//! re-verified by the full-path identity gate before anything signals it.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

use tokio::time::timeout;

use super::{Install, ProcRow};
use crate::launch::CREATE_NO_WINDOW;

/// The official executable at the install root. The uninstall entry's
/// `DisplayIcon` confirms the name (planning doc §1.1: `D:\dsh\DeepSeek
/// Harness.exe,0`).
const MAIN_EXE_NAME: &str = "DeepSeek Harness.exe";

/// The fragment every official uninstall row carries in `DisplayName`
/// (`DeepSeek Harness 0.2.0-rc.2`, 2026-10-03 machine check).
const OFFICIAL_NAME: &str = "deepseek harness";

/// One uninstall-registry subkey's interesting values. Absent values stay
/// `None`; a row without an install location is unusable and gets dropped by
/// [`select_uninstall`] rather than guessed around.
#[derive(Debug)]
struct UninstallRow {
    display_name: String,
    display_version: Option<String>,
    install_location: Option<String>,
}

/* ------------------------------ discovery -------------------------------- */

/// Registry-only discovery. The 2026-10-03 machine check found the entry in
/// the per-user HKCU view; the machine views are scanned as well so both
/// install scopes are seen. All three views are best-effort: a view that
/// cannot be read is skipped, and an exe that is not at the recorded location
/// means "not installed" — a stale uninstall key is not a usable install.
pub(crate) fn discover() -> Result<Option<Install>, String> {
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const HIVES: [(usize, &str); 3] = [
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (HKEY_CURRENT_USER, UNINSTALL),
    ];

    let mut rows = Vec::new();
    for (hive, path) in HIVES {
        collect_uninstall_rows(hive, path, &mut rows);
    }
    let Some(row) = select_uninstall(rows)? else {
        return Ok(None);
    };
    let location = PathBuf::from(row.install_location.unwrap_or_default());
    let main_exe = location.join(MAIN_EXE_NAME);
    if !main_exe.is_file() {
        return Ok(None);
    }
    Ok(Some(Install {
        root: location,
        main_exe,
        version: row.display_version,
    }))
}

/// Judge the rows down to the one official install. Same-location duplicates
/// collapse (the machine and per-user views can both carry the entry); two
/// *distinct* locations refuse the guess — PHL operates exactly one, and
/// "which one?" is the user's call, not a heuristic's.
fn select_uninstall(rows: Vec<UninstallRow>) -> Result<Option<UninstallRow>, String> {
    let mut best: Option<UninstallRow> = None;
    let mut seen_locations: Vec<String> = Vec::new();
    for row in rows {
        if !row.display_name.to_lowercase().contains(OFFICIAL_NAME) {
            continue;
        }
        let Some(location) = row
            .install_location
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        // Trailing slashes and case differ between views ("D:\dsh" vs
        // "D:\DSH\"); the key is what decides "same install".
        let key = location.trim_end_matches(['\\', '/']).to_lowercase();
        if seen_locations.contains(&key) {
            // Same install registered twice (machine view + per-user view, or
            // a stale leftover key): the location — and with it everything
            // operational — is identical, so only the version display can
            // differ. Keep the higher parseable version instead of
            // first-seen, so a leftover key cannot outrank the current one.
            if let (Some(current), Some(candidate)) = (best.as_mut(), row.display_version) {
                let better = match semver::Version::parse(&candidate).ok() {
                    None => false,
                    Some(candidate_v) => match &current.display_version {
                        None => true,
                        Some(current_v) => match semver::Version::parse(current_v) {
                            Ok(current_v) => candidate_v > current_v,
                            // Current text is not a version at all; a real
                            // one beats it for display purposes.
                            Err(_) => true,
                        },
                    },
                };
                if better {
                    current.display_version = Some(candidate);
                }
            }
            continue;
        }
        seen_locations.push(key);
        best = Some(UninstallRow {
            install_location: Some(location),
            ..row
        });
    }
    match (best, seen_locations.len()) {
        (None, _) => Ok(None),
        (Some(row), 1) => Ok(Some(row)),
        (Some(_), _) => Err("检测到多处官方桌面端安装，PHL 只操作一个，已拒绝猜测".into()),
    }
}

/* --------------------------- process snapshot ----------------------------- */

/// Every running process as `(pid, ppid, image file name)`. Toolhelp carries
/// no full path on purpose: the identity check re-probes the picked pid with
/// `QueryFullProcessImageNameW` (via `launch::probe`), so the snapshot only
/// needs names and parentage to narrow the field.
pub(crate) fn enumerate() -> Result<Vec<ProcRow>, String> {
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!("进程快照失败: Win32 错误码 {}", last_error_code()));
        }
        let mut entry = ProcessEntry32W {
            dw_size: std::mem::size_of::<ProcessEntry32W>() as u32,
            cnt_usage: 0,
            th32_process_id: 0,
            th32_default_heap_id: 0,
            th32_module_id: 0,
            cnt_threads: 0,
            th32_parent_process_id: 0,
            pc_pri_class_base: 0,
            dw_flags: 0,
            sz_exe_file: [0; 260],
        };
        let mut rows = Vec::new();
        // A live Windows machine always has processes: a first-call failure
        // is an API-level lie (bad length, access, wow64…) and must carry its
        // error code instead of reading as "not running".
        if Process32FirstW(snapshot, &mut entry) == 0 {
            let code = last_error_code();
            let _ = CloseHandle(snapshot as *mut core::ffi::c_void);
            return Err(format!(
                "进程快照失败（Process32FirstW）: Win32 错误码 {code}"
            ));
        }
        loop {
            let len = entry
                .sz_exe_file
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.sz_exe_file.len());
            rows.push(ProcRow {
                pid: entry.th32_process_id,
                ppid: entry.th32_parent_process_id,
                exe: String::from_utf16_lossy(&entry.sz_exe_file[..len]),
            });
            if Process32NextW(snapshot, &mut entry) == 0 {
                // FALSE means "done" (ERROR_NO_MORE_FILES) *or* "failed" —
                // concluding "not running" from a truncated snapshot is
                // exactly the guess this module must not make.
                let code = last_error_code();
                let _ = CloseHandle(snapshot as *mut core::ffi::c_void);
                if code != ERROR_NO_MORE_FILES {
                    return Err(format!(
                        "进程快照失败（Process32NextW）: Win32 错误码 {code}"
                    ));
                }
                break;
            }
        }
        let _ = CloseHandle(snapshot as *mut core::ffi::c_void);
        Ok(rows)
    }
}

/* ------------------------------ quit / launch ----------------------------- */

/// How long the polite `taskkill` itself may take before the request is
/// declared lost. The user-visible quit budget is `super::QUIT_GRACE`, on top
/// of this.
const QUIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The polite quit: `taskkill` *without* `/F` posts WM_CLOSE to the app's
/// top-level windows — the same request the ✕ button delivers — so the
/// official app runs its own quit-inspection and may pop its own dialog. The
/// confirm window in `quit_official_desktop` is the budget for answering it;
/// only the user's explicit confirmation comes back with `/F`.
pub(crate) async fn request_quit(pid: u32) -> Result<(), String> {
    let output = timeout(
        QUIT_REQUEST_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        }),
    )
    .await
    .map_err(|_| "请求官方桌面端退出超时（taskkill 无响应）".to_string())?
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "官方桌面端拒绝退出请求（退出码 {:?}）: {}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

/// Launch the official app as the user's own daily app: no env injection, no
/// cwd games, and no `CREATE_NO_WINDOW` — the window is the point. Stdio is
/// severed explicitly: a GUI child inheriting PHL's (or a test host's) pipes
/// would hold them open forever — the parent only sees EOF when the app
/// exits. Dropping the Child detaches; PHL quitting never takes the official
/// app down with it, and a double click is absorbed by the official
/// single-instance lock.
pub(crate) async fn spawn_app(install: &Install) -> Result<(), String> {
    use std::process::Stdio;
    tokio::process::Command::new(&install.main_exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动官方桌面端失败: {e}"))
}

/* ------------------------------ registry FFI ------------------------------ */

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const KEY_READ: u32 = 0x0002_0019;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_MORE_DATA: i32 = 234;
const ERROR_NO_MORE_ITEMS: i32 = 259;
/// `Process32NextW` uses this one to say the snapshot is *complete*; any
/// other error means the list is truncated and may not be concluded from.
const ERROR_NO_MORE_FILES: i32 = 18;

extern "system" {
    fn RegOpenKeyExW(
        hkey: usize,
        subkey: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegCloseKey(hkey: usize) -> i32;
    fn RegEnumKeyExW(
        hkey: usize,
        index: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        last_write_time: *mut u64,
    ) -> i32;
    fn RegQueryValueExW(
        hkey: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_len: *mut u32,
    ) -> i32;
}

/// `PROCESSENTRY32W`. `pc_pri_class_base` is a plain `LONG` (4 bytes) in the
/// Win32 header — declaring it pointer-sized pads the struct to 576 bytes and
/// every `Process32FirstW` fails with `ERROR_BAD_LENGTH` (24); the honest
/// size is 568. Field names are snake-cased like `credentials.rs`'s
/// `CredentialW`, not the C spelling.
#[repr(C)]
struct ProcessEntry32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}

extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    // Same signature `launch::probe` declares: HANDLE is `*mut c_void`.
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain([0]).collect()
}

fn last_error_code() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Best-effort row collection: one unreadable hive view or subkey (access
/// denied is normal on machine entries) is skipped, because the other views
/// or the exe-existence check still decide the outcome.
fn collect_uninstall_rows(hive: usize, path: &str, rows: &mut Vec<UninstallRow>) {
    let Ok(subkeys) = enum_subkeys(hive, path) else {
        return;
    };
    for subkey in subkeys {
        let full = format!("{path}\\{subkey}");
        let Ok(hkey) = open_key(hive, &full) else {
            continue;
        };
        let display_name = read_string(hkey, "DisplayName");
        let display_version = read_string(hkey, "DisplayVersion");
        let install_location = read_string(hkey, "InstallLocation");
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if let Ok(Some(display_name)) = display_name {
            rows.push(UninstallRow {
                display_name,
                display_version: display_version.ok().flatten(),
                install_location: install_location.ok().flatten(),
            });
        }
    }
}

fn open_key(hive: usize, path: &str) -> Result<usize, i32> {
    let path_w = wide(path);
    let mut hkey: usize = 0;
    let code = unsafe { RegOpenKeyExW(hive, path_w.as_ptr(), 0, KEY_READ, &mut hkey) };
    if code != ERROR_SUCCESS {
        return Err(code);
    }
    Ok(hkey)
}

fn enum_subkeys(hive: usize, path: &str) -> Result<Vec<String>, String> {
    let hkey = match open_key(hive, path) {
        Ok(hkey) => hkey,
        // A missing view (e.g. no 32-bit entries on this machine) is an empty
        // scan, not an error.
        Err(ERROR_FILE_NOT_FOUND) => return Ok(Vec::new()),
        Err(code) => return Err(format!("打开注册表 {path} 失败: 错误码 {code}")),
    };
    let mut names = Vec::new();
    let mut index: u32 = 0;
    loop {
        let mut buf = [0u16; 256];
        let mut len: u32 = buf.len() as u32;
        let code = unsafe {
            RegEnumKeyExW(
                hkey,
                index,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        if code != ERROR_SUCCESS {
            unsafe {
                let _ = RegCloseKey(hkey);
            }
            return Err(format!("枚举注册表 {path} 失败: 错误码 {code}"));
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
        index += 1;
        if index > 10_000 {
            // Not a real machine's uninstall tree; stop before a corrupt hive
            // spins forever.
            break;
        }
    }
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    Ok(names)
}

fn read_string(hkey: usize, name: &str) -> Result<Option<String>, String> {
    let name_w = wide(name);
    let mut kind: u32 = 0;
    let mut size: u32 = 0;
    let code = unsafe {
        RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != ERROR_SUCCESS && code != ERROR_MORE_DATA {
        return Err(format!("读取注册表值 {name} 失败: 错误码 {code}"));
    }
    // A non-string value under a name we expected text at is treated as
    // absent; `DisplayVersion` being a DWORD is nobody's contract.
    // REG_EXPAND_SZ is accepted but *not* expanded: a `%VAR%` location then
    // fails the exe-existence check and reads as not installed — the honest
    // answer rather than a guess at the environment.
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Ok(None);
    }
    if size == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u8; size as usize];
    let code = unsafe {
        RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut size,
        )
    };
    if code != ERROR_SUCCESS {
        return Err(format!("读取注册表值 {name} 失败: 错误码 {code}"));
    }
    buf.truncate(size as usize);
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(Some(
        String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, version: Option<&str>, location: Option<&str>) -> UninstallRow {
        UninstallRow {
            display_name: name.to_string(),
            display_version: version.map(str::to_string),
            install_location: location.map(str::to_string),
        }
    }

    #[test]
    fn the_official_row_is_found_case_insensitively() {
        let picked = select_uninstall(vec![
            row("Discord", Some("1.0"), Some("C:\\Discord")),
            row(
                "DeepSeek Harness 0.2.0-rc.2",
                Some("0.2.0-rc.2"),
                Some("D:\\dsh"),
            ),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(picked.install_location.as_deref(), Some("D:\\dsh"));
        assert_eq!(picked.display_version.as_deref(), Some("0.2.0-rc.2"));
    }

    #[test]
    fn the_same_install_in_two_views_collapses_to_one() {
        let picked = select_uninstall(vec![
            row(
                "DeepSeek Harness 0.2.0-rc.2",
                Some("0.2.0-rc.2"),
                Some("D:\\dsh"),
            ),
            row(
                "DeepSeek Harness 0.2.0-rc.2",
                None,
                Some("D:\\DSH\\"), // case and trailing slash differ per view
            ),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(picked.display_version.as_deref(), Some("0.2.0-rc.2"));
    }

    #[test]
    fn a_newer_versioned_key_outranks_a_stale_leftover_at_the_same_location() {
        let picked = select_uninstall(vec![
            row(
                "DeepSeek Harness 0.1.6-alpha.1",
                Some("0.1.6-alpha.1"),
                Some("D:\\dsh"),
            ),
            row(
                "DeepSeek Harness 0.2.0-rc.2",
                Some("0.2.0-rc.2"),
                Some("D:\\dsh"),
            ),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(picked.display_version.as_deref(), Some("0.2.0-rc.2"));
    }

    #[test]
    fn a_versioned_key_outranks_a_versionless_leftover_at_the_same_location() {
        let picked = select_uninstall(vec![
            row("DeepSeek Harness", None, Some("D:\\dsh")),
            row(
                "DeepSeek Harness 0.2.0-rc.2",
                Some("0.2.0-rc.2"),
                Some("D:\\dsh"),
            ),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(picked.display_version.as_deref(), Some("0.2.0-rc.2"));
    }

    #[test]
    fn two_distinct_installs_refuse_to_guess() {
        let err = select_uninstall(vec![
            row("DeepSeek Harness 0.2.0-rc.2", None, Some("D:\\dsh")),
            row("DeepSeek Harness 0.3.0", None, Some("E:\\dsh-new")),
        ])
        .unwrap_err();
        assert!(err.contains("只操作一个"));
    }

    #[test]
    fn locationless_and_foreign_rows_are_ignored() {
        let picked = select_uninstall(vec![
            row("DeepSeek Harness 0.2.0-rc.2", Some("0.2.0-rc.2"), None),
            row("DeepSeek Browser", None, Some("C:\\dsb")),
        ])
        .unwrap();
        assert!(picked.is_none());
    }
}
