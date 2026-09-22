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
    /// `None` when the query itself failed (protected process, no rights).
    pub exe_path: Option<String>,
    pub created_at_ms: Option<i64>,
}

/// Is this pid still the same process we recorded? Windows uses a manual
/// advapi32-style FFI query (matching `credentials.rs`); Unix shells out to
/// `ps`, which reports liveness, the executable, and — through elapsed
/// time — a creation stamp. A pid `ps` does not list is definitively gone
/// (`Exited`), which is what makes stop-and-confirm work there; a failed
/// `ps` call stays `Unknown`, which adoption treats as keep-running.
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

/// Pure half of the Unix probe: turn one `ps -o state=,pid=,etime=,comm=` line
/// (empty = the kernel has no such pid) into a Probe, given "now". Not
/// cfg-gated: `unix::probe` calls it at runtime and the unit tests pin the
/// parse on a Windows host too.
///
/// The state column comes *first* on purpose: `comm` is a path and may contain
/// spaces, so anything appended after it could not be split back off. A zombie
/// reports `Exited` — it has already terminated, and the row survives only
/// until its parent reaps it. For a stop that distinction is the whole game:
/// treating a zombie as running makes a successful termination wait out its
/// whole window and then report a failure.
#[cfg(any(not(windows), test))]
pub(crate) fn probe_from_ps_line(line: &str, now_ms: i64) -> Probe {
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
    let Some((etime, comm)) = rest.trim_start().split_once(char::is_whitespace) else {
        return Probe::default();
    };
    // The row must be about a plausible pid, not an unrelated one.
    if field_pid.parse::<u32>().ok().filter(|p| *p > 0).is_none() {
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
    let comm = comm.trim();
    Probe {
        state: ProcessState::Alive,
        exe_path: (!comm.is_empty()).then(|| comm.to_string()),
        // `etime` has one-second resolution; the creation-tolerance gate
        // is minutes wide, so the rounding is noise here.
        created_at_ms: Some(now_ms - elapsed_ms),
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
        let out = match std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "state=,pid=,etime=,comm="])
            .output()
        {
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
        probe_from_ps_line(line, now_ms)
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
    fn padded_columns_preserve_the_full_executable_path() {
        let p = probe_from_ps_line("  S   42   01:02  /Applications/My App/node  ", 100_000);
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
            let p = probe_from_ps_line(&line, 1_700_000_000_000);
            assert_eq!(p.state, ProcessState::Exited, "state {state}");
            assert_eq!(p.exe_path, None);
        }
    }

    #[test]
    fn a_ps_row_parses_into_an_alive_probe_on_any_host() {
        // The Unix probe's pure half, pinned with fixtures so a Windows CI
        // run covers it too. `now_ms` = 1_700_000_000_000.
        let p = probe_from_ps_line("S 4242 01:02:03 /usr/local/bin/node", 1_700_000_000_000);
        assert_eq!(p.state, ProcessState::Alive);
        assert_eq!(p.exe_path.as_deref(), Some("/usr/local/bin/node"));
        assert_eq!(p.created_at_ms, Some(1_700_000_000_000 - 3_723_000));

        // Sub-minute and day-bearing forms.
        let p = probe_from_ps_line("R 7 45 node", 1_000);
        assert_eq!(p.created_at_ms, Some(1_000 - 45_000));
        let p = probe_from_ps_line("Ss 7 2-03:04:05 /bin/sh", 1_000);
        assert_eq!(
            p.created_at_ms,
            Some(1_000 - (2 * 86_400 + 3 * 3_600 + 4 * 60 + 5) * 1_000)
        );

        // Paths with spaces survive: the state column leads precisely so this
        // one can stay last.
        let p = probe_from_ps_line("S 9 10:00 /Applications/My App/helper", 1_000);
        assert_eq!(p.exe_path.as_deref(), Some("/Applications/My App/helper"));

        // Empty output is proof of absence — the half the old `kill -0`
        // probe could never give the stop-confirmation loop.
        assert_eq!(probe_from_ps_line("", 1_000).state, ProcessState::Exited);
        // Garbage is NOT proof of anything: stay Unknown, touch nothing.
        assert_eq!(
            probe_from_ps_line("?? 99 1", 1_000).state,
            ProcessState::Unknown
        );
        assert_eq!(
            probe_from_ps_line("S 1 abc /bin/x", 1_000).state,
            ProcessState::Unknown
        );
    }
}
