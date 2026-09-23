//! Pid probing split out of `registry` (O-08 / macOS port): the platform
//! half of "is this pid still *our* process", with an identical result type
//! on every host so `decide` stays pure.

/// Query failures are not evidence that a process exited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProcessState {
    Alive,
    Exited,
    #[default]
    Unknown,
}

/// What a pid probe found at adoption time.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub state: ProcessState,
    /// Executable identity. macOS is kernel-canonicalized; non-macOS Unix
    /// retains its pre-existing ps command-text behavior; Windows reports the
    /// process image path.
    pub exe_path: Option<String>,
    pub created_at_ms: Option<i64>,
    /// Kernel process-birth token. PID reuse, including reuse by the same
    /// executable, changes this value.
    pub process_start_token: Option<String>,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct OsIdentity {
    exe_path: String,
    start_token: String,
    created_at_ms: Option<i64>,
}

#[cfg(target_os = "macos")]
pub(crate) fn same_process_start_token(stored: &Option<String>, observed: &Option<String>) -> bool {
    matches!((stored.as_deref(), observed.as_deref()), (Some(a), Some(b)) if a == b)
}

/// Is this pid still the same process we recorded? Windows uses a manual
/// advapi32-style FFI query (matching `credentials.rs`). Unix uses `ps` for
/// liveness/zombie state; macOS replaces `comm` identity with a kernel path and
/// birth token. Other Unix targets retain the existing ps-based behavior.
pub(crate) fn probe_process(pid: u32) -> Probe {
    #[cfg(windows)]
    {
        win::probe(pid)
    }
    #[cfg(not(windows))]
    {
        unix::probe(pid)
    }
}

/// Pure half of the Unix probe: turn one `ps -o state=,pid=,etime=` line
/// (empty = the kernel has no such pid) into a Probe, given the requested pid
/// and "now". Not
/// cfg-gated: `unix::probe` calls it at runtime and the unit tests pin the
/// parse on a Windows host too.
///
/// The state column comes first. Trailing command text is retained only for
/// legacy non-macOS Unix callers; macOS production probes omit it. A zombie
/// reports `Exited` — it has already terminated, and the row survives only
/// until its parent reaps it. For a stop that distinction is the whole game:
/// treating a zombie as running makes a successful termination wait out its
/// whole window and then report a failure.
#[cfg(any(not(windows), test))]
pub(crate) fn probe_from_ps_line(line: &str, expected_pid: u32, now_ms: i64) -> Probe {
    let line = line.trim();
    if line.is_empty() {
        return Probe {
            state: ProcessState::Exited,
            ..Probe::default()
        };
    }
    let Some((state, rest)) = line.split_once(char::is_whitespace) else {
        return Probe::default();
    };
    let Some((field_pid, rest)) = rest.trim_start().split_once(char::is_whitespace) else {
        return Probe::default();
    };
    let Some(state_char) = state.chars().next().map(|ch| ch.to_ascii_uppercase()) else {
        return Probe::default();
    };
    if !matches!(
        state_char,
        'R' | 'S' | 'I' | 'T' | 'U' | 'Z' | 'D' | 'L' | 'W' | 'P' | 'X'
    ) {
        return Probe::default();
    }
    let rest = rest.trim_start();
    let (etime, comm) = match rest.find(char::is_whitespace) {
        Some(etime_end) => (&rest[..etime_end], rest[etime_end..].trim()),
        None => (rest, ""),
    };
    // A test seam or unexpected ps format must not attribute another row to
    // the requested numeric PID.
    if field_pid.parse::<u32>().ok() != Some(expected_pid) {
        return Probe::default();
    }
    if state.starts_with(['Z', 'z']) {
        return Probe {
            state: ProcessState::Exited,
            ..Probe::default()
        };
    }
    let Some(elapsed_ms) = parse_ps_etime(etime) else {
        return Probe::default();
    };
    Probe {
        state: ProcessState::Alive,
        // On macOS production probes omit comm and fill identity only from
        // libproc. Other Unix targets retain the prior command-text behavior.
        exe_path: (!comm.is_empty()).then(|| comm.to_string()),
        // On macOS the exact process_start_token, not rounded etime, gates
        // adoption; other Unix targets retain their existing policy.
        created_at_ms: Some(now_ms - elapsed_ms),
        process_start_token: None,
    }
}

/// Only the no-match exit with no diagnostic proves absence. Other failures
/// must keep the registration: a failed query is not a successful stop.
#[cfg(any(not(windows), test))]
fn ps_failure_state(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> ProcessState {
    if code == Some(1)
        && stdout.iter().all(u8::is_ascii_whitespace)
        && stderr.iter().all(u8::is_ascii_whitespace)
    {
        ProcessState::Exited
    } else {
        ProcessState::Unknown
    }
}

/// `[[dd-]hh:]mm:ss` → milliseconds (one-second resolution).
#[cfg(any(not(windows), test))]
fn parse_ps_etime(text: &str) -> Option<i64> {
    let (days, rest) = match text.split_once('-') {
        Some((d, r)) => (d.parse::<i64>().ok()?, r),
        None => (0, text),
    };
    let mut parts: Vec<i64> = Vec::with_capacity(3);
    for p in rest.split(':') {
        parts.push(p.parse().ok()?);
    }
    let seconds = match parts.as_slice() {
        [s] => *s,
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some((days * 86_400 + seconds) * 1000)
}

/// The process group `pid` *leads*, when the kernel says it leads one.
///
/// A stop has to reach what DSH started, and on Unix the only handle on that
/// tree is the process group. This answers "may we signal a group at all": a
/// process that leads one (`pgid == pid`) got it from whoever spawned it with
/// `Command::process_group(0)`, and its members are the processes that
/// descended from the pid `decide` already identified as ours. A pid sharing
/// *another* group — PHL's own, a row written before PHL set one apart, a
/// process that never got its own — must never be signalled as a group:
/// `kill -TERM -<pgid>` would reach every member, PHL included.
///
/// Windows has no equivalent (`taskkill /T` walks the tree instead), so this is
/// Unix-only and always `None` there.
#[cfg(not(windows))]
pub(crate) fn owned_group_of(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pgid="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ps_pgid(&String::from_utf8_lossy(&out.stdout), pid)
}

/// Pure half of the group probe: the `pgid=` column, accepted only when it
/// names the pid itself. Not cfg-gated, so the rule is pinned by a test on
/// every host.
#[cfg(any(not(windows), test))]
pub(crate) fn parse_ps_pgid(stdout: &str, pid: u32) -> Option<u32> {
    let group = stdout.trim().parse::<u32>().ok()?;
    (group == pid).then_some(group)
}

#[cfg(not(windows))]
mod unix {
    use super::{probe_from_ps_line, ps_failure_state, Probe};

    pub fn probe(pid: u32) -> Probe {
        let mut command = std::process::Command::new("ps");
        command.args(["-p", &pid.to_string(), "-o"]);
        #[cfg(target_os = "macos")]
        command.arg("state=,pid=,etime=");
        #[cfg(not(target_os = "macos"))]
        command.arg("state=,pid=,etime=,comm=");
        let out = match command.output() {
            Ok(out) => out,
            Err(_) => return Probe::default(), // no `ps`: stay Unknown
        };
        // BSD ps is process-centric: with `-p` it succeeds only when *that*
        // pid exists; no match is a failing exit with empty output. Trusting
        // "did it print a row" instead would call every dead pid Alive.
        if !out.status.success() {
            return Probe {
                state: ps_failure_state(out.status.code(), &out.stdout, &out.stderr),
                ..Probe::default()
            };
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut probe = probe_from_ps_line(line, pid, now_ms);
        #[cfg(target_os = "macos")]
        if probe.state == super::ProcessState::Alive {
            // A ps row only establishes that this PID is alive. Do not infer
            // executable identity from `comm`, argv[0], or elapsed seconds.
            // Each supported OS module verifies the PID instance around its
            // kernel-backed path query. Failure leaves an alive process
            // unverifiable, which registry::decide treats as fail-closed.
            probe.exe_path = None;
            probe.process_start_token = None;
            if let Some(identity) = super::os_identity::read(pid) {
                probe.exe_path = Some(identity.exe_path);
                probe.process_start_token = Some(identity.start_token);
                probe.created_at_ms = identity.created_at_ms;
            } else {
                probe.created_at_ms = None;
            }
        }
        probe
    }
}

#[cfg(target_os = "macos")]
mod os_identity {
    use std::ffi::{c_char, c_void, CStr};
    use std::mem::{size_of, MaybeUninit};
    use std::path::Path;

    // Apple’s libproc interfaces are private, not a stable SDK contract. They
    // are used here because they report the kernel's executable path and
    // process birth time; any unavailable, changed, or inconsistent result
    // fails closed (the process stays running but PHL will not adopt/stop it).
    // Apple DTS explicitly warns these interfaces can change without notice.
    // Apple's libproc.h marks proc_pidpath/proc_pidinfo as available since
    // macOS 10.5; the macOS CI runner compiles and runs the actual probes.
    #[repr(C)]
    #[allow(dead_code)] // Unused members are required to preserve proc_bsdinfo ABI offsets.
    #[derive(Clone, Copy)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [c_char; 16],
        pbi_name: [c_char; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    #[link(name = "proc")]
    extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    }

    const PROC_PIDTBSDINFO: i32 = 3;

    fn bsd_info(pid: i32) -> Option<ProcBsdInfo> {
        let mut out = MaybeUninit::<ProcBsdInfo>::zeroed();
        // SAFETY: the buffer matches Apple's proc_bsdinfo ABI layout in
        // sys/proc_info.h; size and pbi_pid are checked before use.
        let size = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTBSDINFO,
                0,
                out.as_mut_ptr().cast(),
                size_of::<ProcBsdInfo>() as i32,
            )
        };
        if size != size_of::<ProcBsdInfo>() as i32 {
            return None;
        }
        // SAFETY: proc_pidinfo returned the exact requested structure size.
        let info = unsafe { out.assume_init() };
        (info.pbi_pid == pid as u32
            && info.pbi_start_tvsec > 0
            && info.pbi_start_tvusec < 1_000_000)
            .then_some(info)
    }

    fn start_token(info: &ProcBsdInfo) -> String {
        format!("macos:{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec)
    }

    pub fn read(pid: u32) -> Option<super::OsIdentity> {
        let pid = i32::try_from(pid).ok()?;
        let before = bsd_info(pid)?;
        let mut path = [0u8; 4096];
        // SAFETY: path is a writable buffer of the declared size.
        let written =
            unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), path.len().try_into().ok()?) };
        if written <= 0 || written as usize >= path.len() {
            return None;
        }
        let nul = path.iter().position(|byte| *byte == 0)?;
        // The terminator is found within our fixed buffer, so construction
        // cannot read beyond it even if libproc changes its output contract.
        let raw_path = CStr::from_bytes_with_nul(&path[..=nul])
            .ok()?
            .to_str()
            .ok()?;
        let exe_path = std::fs::canonicalize(Path::new(raw_path))
            .ok()?
            .to_string_lossy()
            .into_owned();
        let after = bsd_info(pid)?;
        if before.pbi_pid != after.pbi_pid || start_token(&before) != start_token(&after) {
            return None;
        }
        let created_at_ms = i64::try_from(before.pbi_start_tvsec)
            .ok()?
            .checked_mul(1_000)?
            .checked_add(i64::try_from(before.pbi_start_tvusec / 1_000).ok()?)?;
        Some(super::OsIdentity {
            exe_path,
            start_token: start_token(&before),
            created_at_ms: Some(created_at_ms),
        })
    }
}

#[cfg(windows)]
mod win {
    use super::{Probe, ProcessState};

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetLastError() -> u32;
        fn GetExitCodeProcess(handle: *mut core::ffi::c_void, exit_code: *mut u32) -> i32;
        fn QueryFullProcessImageNameW(
            handle: *mut core::ffi::c_void,
            flags: u32,
            buffer: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn GetProcessTimes(
            handle: *mut core::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    fn filetime_to_ms(t: FileTime) -> i64 {
        // 100 ns ticks since 1601-01-01 → ms since the Unix epoch.
        const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;
        let ticks = (((t.high as u64) << 32) | t.low as u64) as i64;
        (ticks - EPOCH_DIFF_100NS) / 10_000
    }

    pub fn probe(pid: u32) -> Probe {
        let mut out = Probe::default();
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                // Invalid pid is proof of absence. Access denied and every
                // other failure keep the default Unknown state.
                if GetLastError() == 87 {
                    out.state = ProcessState::Exited;
                }
                return out;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            if ok == 0 {
                CloseHandle(handle);
                return out;
            }
            if code != STILL_ACTIVE {
                out.state = ProcessState::Exited;
                CloseHandle(handle);
                return out;
            }
            out.state = ProcessState::Alive;
            let mut buf = [0u16; 32768];
            let mut size = buf.len() as u32;
            if QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) != 0 {
                out.exe_path = Some(String::from_utf16_lossy(&buf[..size as usize]));
            }
            let mut creation = FileTime::default();
            let mut exit = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0 {
                out.created_at_ms = Some(filetime_to_ms(creation));
            }
            CloseHandle(handle);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_query_errors_are_not_proof_of_exit() {
        assert_eq!(ps_failure_state(Some(1), b"", b""), ProcessState::Exited);
        assert_eq!(
            ps_failure_state(Some(1), b"", b"permission denied"),
            ProcessState::Unknown
        );
        assert_eq!(ps_failure_state(Some(2), b"", b""), ProcessState::Unknown);
        assert_eq!(ps_failure_state(None, b"", b""), ProcessState::Unknown);
        assert_eq!(
            ps_failure_state(Some(1), b"partial output", b""),
            ProcessState::Unknown
        );
    }

    #[test]
    fn legacy_process_records_omit_the_optional_birth_token() {
        let raw = r#"{"instanceId":"main","pid":1,"port":3080,"startedAtMs":1700000000000,"exePath":"node"}"#;
        let legacy: crate::launch::PersistedProcess = serde_json::from_str(raw).unwrap();
        assert_eq!(legacy.process_start_token, None);
        assert!(!serde_json::to_value(&legacy)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("processStartToken"));
    }

    #[test]
    fn padded_columns_preserve_the_full_executable_path() {
        let p = probe_from_ps_line("  S   42   01:02  /Applications/My App/node  ", 42, 100_000);
        assert_eq!(p.state, ProcessState::Alive);
        assert_eq!(p.created_at_ms, Some(38_000));
        assert_eq!(p.exe_path.as_deref(), Some("/Applications/My App/node"));
    }

    /// A zombie has terminated; only its exit status is still readable. It is
    /// not runnable, not adoptable and not something a stop should wait on —
    /// the row disappears the moment its parent reaps it. Counting it as Alive
    /// makes a *successful* stop sit out its whole confirm window and then
    /// report a failure.
    #[test]
    fn a_zombie_row_is_an_exit_not_a_running_process() {
        for state in ["Z", "Z+", "z"] {
            let line = format!("{state} 4242 01:02 /usr/local/bin/node");
            let p = probe_from_ps_line(&line, 4242, 1_700_000_000_000);
            assert_eq!(p.state, ProcessState::Exited, "state {state}");
            assert_eq!(p.exe_path, None);
        }
    }

    #[test]
    fn a_ps_row_parses_into_an_alive_probe_on_any_host() {
        // The Unix probe's pure half, pinned with fixtures so a Windows CI
        // run covers it too. `now_ms` = 1_700_000_000_000.
        let p = probe_from_ps_line(
            "S 4242 01:02:03 /usr/local/bin/node",
            4242,
            1_700_000_000_000,
        );
        assert_eq!(p.state, ProcessState::Alive);
        assert_eq!(p.exe_path.as_deref(), Some("/usr/local/bin/node"));
        assert_eq!(p.created_at_ms, Some(1_700_000_000_000 - 3_723_000));

        // Sub-minute and day-bearing forms.
        let p = probe_from_ps_line("R 7 45 node", 7, 1_000);
        assert_eq!(p.created_at_ms, Some(1_000 - 45_000));
        let p = probe_from_ps_line("Ss 7 2-03:04:05 /bin/sh", 7, 1_000);
        assert_eq!(
            p.created_at_ms,
            Some(1_000 - (2 * 86_400 + 3 * 3_600 + 4 * 60 + 5) * 1_000)
        );

        // Paths with spaces survive: the state column leads precisely so this
        // one can stay last.
        let p = probe_from_ps_line("S 9 10:00 /Applications/My App/helper", 9, 1_000);
        assert_eq!(p.exe_path.as_deref(), Some("/Applications/My App/helper"));

        // Empty output is proof of absence — the half the old `kill -0`
        // probe could never give the stop-confirmation loop.
        assert_eq!(probe_from_ps_line("", 1, 1_000).state, ProcessState::Exited);
        // Garbage is NOT proof of anything: stay Unknown, touch nothing.
        assert_eq!(
            probe_from_ps_line("?? 99 1", 99, 1_000).state,
            ProcessState::Unknown
        );
        assert_eq!(
            probe_from_ps_line("S 1 abc /bin/x", 1, 1_000).state,
            ProcessState::Unknown
        );
    }

    #[cfg(target_os = "macos")]
    struct HeldProbeProcess {
        child: std::process::Child,
        executable: std::path::PathBuf,
    }

    #[cfg(target_os = "macos")]
    impl Drop for HeldProbeProcess {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// This test is also the executable for the runtime probe tests below.
    /// The copied test binary keeps a real same-named executable alive so the
    /// tests exercise proc_pidpath/proc_pidinfo rather than a parsed fixture.
    #[cfg(target_os = "macos")]
    #[test]
    fn identity_probe_child_fixture() {
        if std::env::var_os("PHL_IDENTITY_PROBE_CHILD").is_none() {
            return;
        }
        use std::io::Write;
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "PHL_IDENTITY_PROBE_READY");
        let _ = stdout.flush();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    #[cfg(target_os = "macos")]
    fn spawn_probe_child(path: &std::path::Path) -> HeldProbeProcess {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), path).unwrap();
        start_probe_child(path)
    }

    #[cfg(target_os = "macos")]
    fn start_probe_child(path: &std::path::Path) -> HeldProbeProcess {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        let mut child = std::process::Command::new(path)
            .args([
                "--exact",
                "launch::probe::tests::identity_probe_child_fixture",
                "--nocapture",
            ])
            .env("PHL_IDENTITY_PROBE_CHILD", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        if ready_tx.send(line.clone()).is_err()
                            || line.contains("PHL_IDENTITY_PROBE_READY")
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = ready_tx.send(format!("probe-child-io-error: {e}"));
                        return;
                    }
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            match ready_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) if line.contains("PHL_IDENTITY_PROBE_READY") => {
                    ready = true;
                    break;
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !ready {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = reader_thread.join();
        assert!(ready, "probe child did not become ready");
        HeldProbeProcess {
            child,
            executable: std::fs::canonicalize(path).unwrap(),
        }
    }

    #[cfg(target_os = "macos")]
    fn probe_until_identified(child: &HeldProbeProcess) -> Probe {
        use std::time::{Duration, Instant};
        let pid = child.child.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let probe = probe_process(pid);
            if probe.state == ProcessState::Alive
                && probe.exe_path.as_deref() == child.executable.to_str()
                && probe.process_start_token.is_some()
            {
                return probe;
            }
            assert!(
                Instant::now() < deadline,
                "kernel identity unavailable: {probe:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_kernel_probe_distinguishes_same_name_processes_by_path_and_birth() {
        let root = std::env::temp_dir().join(format!("phl-probe-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let left = spawn_probe_child(&root.join("left").join("same-name"));
        let right = spawn_probe_child(&root.join("right").join("same-name"));
        let old_probe = probe_until_identified(&left);
        let new_probe = probe_until_identified(&right);

        assert_eq!(
            left.executable.file_name(),
            right.executable.file_name(),
            "fixture processes must have the same executable name"
        );
        assert_ne!(old_probe.exe_path, new_probe.exe_path);
        assert_ne!(old_probe.process_start_token, new_probe.process_start_token);

        let live_record = crate::launch::PersistedProcess {
            instance_id: "probe-fixture-live".into(),
            pid: left.child.id(),
            port: 3000,
            started_at_ms: old_probe.created_at_ms.unwrap(),
            exe_path: old_probe.exe_path.clone().unwrap(),
            process_start_token: old_probe.process_start_token.clone(),
        };
        assert_eq!(
            crate::launch::registry::decide(&live_record, &old_probe),
            crate::launch::registry::Adoption::Adopt,
            "the actual kernel path/token from launch must remain adoptable"
        );

        let record = crate::launch::PersistedProcess {
            instance_id: "probe-fixture".into(),
            pid: right.child.id(), // simulate the old PID being reused
            port: 3000,
            started_at_ms: old_probe.created_at_ms.unwrap(),
            exe_path: old_probe.exe_path.clone().unwrap(),
            process_start_token: old_probe.process_start_token.clone(),
        };
        assert!(matches!(
            crate::launch::registry::decide(&record, &new_probe),
            crate::launch::registry::Adoption::Forget {
                keep_running: true,
                ..
            }
        ));
        let same_named_new_path = crate::launch::PersistedProcess {
            instance_id: "probe-fixture-path-mismatch".into(),
            pid: right.child.id(),
            port: 3000,
            started_at_ms: old_probe.created_at_ms.unwrap(),
            exe_path: new_probe.exe_path.clone().unwrap(),
            process_start_token: old_probe.process_start_token.clone(),
        };
        assert!(matches!(
            crate::launch::registry::decide(&same_named_new_path, &new_probe),
            crate::launch::registry::Adoption::Forget {
                keep_running: true,
                ..
            }
        ));

        let mut legacy = record;
        legacy.exe_path = new_probe.exe_path.clone().unwrap();
        legacy.process_start_token = None;
        assert!(matches!(
            crate::launch::registry::decide(&legacy, &new_probe),
            crate::launch::registry::Adoption::Forget {
                keep_running: true,
                ..
            }
        ));

        // Same executable path is still insufficient if this PID slot is now
        // occupied by another process instance: the kernel birth token differs.
        let same_path = root.join("same-path").join("same-binary");
        let first_birth = spawn_probe_child(&same_path);
        let second_birth = start_probe_child(&same_path);
        let first_probe = probe_until_identified(&first_birth);
        let second_probe = probe_until_identified(&second_birth);
        assert_eq!(first_probe.exe_path, second_probe.exe_path);
        assert_ne!(
            first_probe.process_start_token,
            second_probe.process_start_token
        );
        let reused_same_path = crate::launch::PersistedProcess {
            instance_id: "probe-fixture-same-path-reuse".into(),
            pid: second_birth.child.id(),
            port: 3000,
            started_at_ms: first_probe.created_at_ms.unwrap(),
            exe_path: first_probe.exe_path.clone().unwrap(),
            process_start_token: first_probe.process_start_token.clone(),
        };
        assert!(matches!(
            crate::launch::registry::decide(&reused_same_path, &second_probe),
            crate::launch::registry::Adoption::Forget {
                keep_running: true,
                ..
            }
        ));
        drop((first_birth, second_birth));
        drop((left, right));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_live_process_without_kernel_identity_is_not_adoptable() {
        let record = crate::launch::PersistedProcess {
            instance_id: "unknown-identity".into(),
            pid: 1234,
            port: 3000,
            started_at_ms: 1_700_000_000_000,
            exe_path: "/tmp/node".into(),
            process_start_token: None,
        };
        let probe = Probe {
            state: ProcessState::Alive,
            exe_path: None,
            created_at_ms: None,
            process_start_token: None,
        };
        assert!(matches!(
            crate::launch::registry::decide(&record, &probe),
            crate::launch::registry::Adoption::Forget {
                keep_running: true,
                ..
            }
        ));
    }
}
