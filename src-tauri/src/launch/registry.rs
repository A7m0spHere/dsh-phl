//! The durable process registry (O-08).
//!
//! `Processes` is in memory: when PHL restarts, DSH children keep running
//! and the previous code simply forgot them — the port looked free, stop
//! could not reach them, and the UI showed "stopped" over a live server.
//!
//! This registry persists one record per managed child next to the root
//! pointer (outside the data root, like the migration journal): pid, port,
//! spawn time, and the canonical exe path. On the next boot,
//! `adopt_processes` probes each pid — alive, same executable, creation time
//! matching the record within tolerance — and either adopts the child (stop
//! and "打开 WebUI" work again, no duplicate launch, port stays reserved) or
//! forgets it. A pid whose identity cannot be confirmed is *never* touched:
//! PID reuse must not let PHL kill some stranger's process.
//!
//! The decision function is pure over an injected [`Probe`] so it can be
//! unit-tested without spawning long-lived processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Everything an adoption needs to answer "is this pid still *our* DSH, or
/// has the OS handed the number to someone else since".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProcess {
    pub instance_id: String,
    pub pid: u32,
    pub port: u16,
    /// Wall-clock milliseconds at spawn, for the creation-time comparison.
    pub started_at_ms: i64,
    /// Canonicalized node binary path, for the identity comparison.
    pub exe_path: String,
}

/// What a pid probe found at adoption time.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub alive: bool,
    /// `None` when the query itself failed (protected process, no rights).
    pub exe_path: Option<String>,
    pub created_at_ms: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Adoption {
    Adopt,
    Forget { reason: String, keep_running: bool },
}

/// The ± tolerance between our spawn stamp and the kernel creation time.
/// Generous enough for a slow first disk, narrow enough to reject a reused
/// pid (the same exe would have to be relaunched within the window by hand).
const CREATION_TOLERANCE_MS: i64 = 10 * 60 * 1000;

/// Normalize for comparison: strip the Windows verbatim prefix and case,
/// unify separators.
pub(crate) fn normalize_exe(raw: &str) -> String {
    let text = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .or_else(|| raw.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or_else(|| raw.to_string());
    text.replace('/', "\\").to_lowercase()
}

pub fn decide(rec: &PersistedProcess, probe: &Probe) -> Adoption {
    if !probe.alive {
        return Adoption::Forget {
            reason: "进程已退出".into(),
            keep_running: false,
        };
    }
    match &probe.exe_path {
        // Cannot read who owns the pid: refuse to manage it, but also refuse
        // to stop it — "识别不可靠时不自动终止".
        None => Adoption::Forget {
            reason: "无法确认该进程的身份，PHL 不再管理它（也不会终止它）".into(),
            keep_running: true,
        },
        Some(got) => {
            if normalize_exe(got) != normalize_exe(&rec.exe_path) {
                return Adoption::Forget {
                    reason: "该 PID 已被其他程序复用".into(),
                    keep_running: true,
                };
            }
            if let Some(created) = probe.created_at_ms {
                let delta = (created - rec.started_at_ms).abs();
                if delta > CREATION_TOLERANCE_MS {
                    return Adoption::Forget {
                        reason: "进程创建时间与记录不符".into(),
                        keep_running: true,
                    };
                }
            }
            Adoption::Adopt
        }
    }
}

/// Latest `launch-<timestamp>.log` in an instance's logs dir. The ISO-style
/// file names sort lexicographically in time order.
pub(crate) async fn latest_launch_log(logs_dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(String, PathBuf)> = None;
    let Ok(mut dir) = tokio::fs::read_dir(logs_dir).await else {
        return None;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("launch-")
            && name.ends_with(".log")
            && best.as_ref().map_or(true, |(n, _)| &name > n)
        {
            best = Some((name, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/* ------------------------------ persisted map ------------------------------ */

#[derive(Default)]
pub struct Registry {
    entries: Mutex<HashMap<String, PersistedProcess>>,
    path: Mutex<Option<PathBuf>>,
}

impl Registry {
    /// Bind to the file and load whatever a previous run left behind. Called
    /// once at setup, before any command can see the state.
    pub(crate) fn bind(&self, path: PathBuf) {
        *self.path.lock().expect("registry path") = Some(path.clone());
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(list) = serde_json::from_str::<Vec<PersistedProcess>>(&raw) {
                let mut entries = self.entries.lock().expect("registry lock");
                for rec in list {
                    entries.insert(rec.instance_id.clone(), rec);
                }
            }
        }
    }

    pub(crate) fn remember(&self, rec: PersistedProcess) {
        self.entries
            .lock()
            .expect("registry lock")
            .insert(rec.instance_id.clone(), rec);
        self.persist();
    }

    pub(crate) fn forget(&self, instance_id: &str) {
        if self
            .entries
            .lock()
            .expect("registry lock")
            .remove(instance_id)
            .is_some()
        {
            self.persist();
        }
    }

    pub(crate) fn forget_pid(&self, instance_id: &str, pid: u32) {
        let mut entries = self.entries.lock().expect("registry lock");
        if entries.get(instance_id).is_some_and(|r| r.pid == pid) {
            entries.remove(instance_id);
            drop(entries);
            self.persist();
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<PersistedProcess> {
        self.entries
            .lock()
            .expect("registry lock")
            .values()
            .cloned()
            .collect()
    }

    /// tmp+rename like every other durable record: a torn registry file must
    /// not make all running children unmanageable.
    fn persist(&self) {
        let path = self.path.lock().expect("registry path").clone();
        let Some(path) = path else { return };
        let entries: Vec<PersistedProcess> = self
            .entries
            .lock()
            .expect("registry lock")
            .values()
            .cloned()
            .collect();
        let Ok(body) = serde_json::to_string_pretty(&entries) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/* ------------------------------ probing ------------------------------ */

/// Is this pid still the same process we recorded? Windows-only identity
/// query (manual advapi32-style FFI, matching `credentials.rs`; other
/// platforms report "unknown", which adoption treats as
/// `keep_running` — forget the record, touch nothing).
pub(crate) fn probe_process(pid: u32) -> Probe {
    #[cfg(windows)]
    {
        win::probe(pid)
    }
    #[cfg(not(windows))]
    {
        // A cheap portable approximation: signal 0 probes liveness only.
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        Probe {
            alive,
            exe_path: None,
            created_at_ms: None,
        }
    }
}

#[cfg(windows)]
mod win {
    use super::Probe;

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
                return out; // cannot open → treat as gone
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            if ok == 0 || code != STILL_ACTIVE {
                CloseHandle(handle);
                return out;
            }
            out.alive = true;
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

/* ------------------------------ tests ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: u32) -> PersistedProcess {
        PersistedProcess {
            instance_id: "main".into(),
            pid,
            port: 3080,
            started_at_ms: 1_700_000_000_000,
            exe_path: r"C:\PHL\runtimes\node-22\node.exe".into(),
        }
    }

    fn probe(exe: Option<&str>, created: Option<i64>) -> Probe {
        Probe {
            alive: true,
            exe_path: exe.map(str::to_string),
            created_at_ms: created,
        }
    }

    #[test]
    fn a_dead_pid_is_forgotten_without_fuss() {
        let out = decide(
            &rec(1),
            &Probe {
                alive: false,
                ..Default::default()
            },
        );
        assert!(matches!(
            out,
            Adoption::Forget {
                keep_running: false,
                ..
            }
        ));
    }

    #[test]
    fn an_unverifiable_pid_is_left_running() {
        let out = decide(&rec(1), &probe(None, None));
        match out {
            Adoption::Forget {
                reason,
                keep_running,
            } => {
                assert!(keep_running, "never terminate what cannot be identified");
                assert!(reason.contains("身份"), "{reason}");
            }
            Adoption::Adopt => panic!("must not adopt an unknown process"),
        }
    }

    #[test]
    fn pid_reuse_by_another_program_is_rejected() {
        let out = decide(
            &rec(1),
            &probe(
                Some(r"C:\Windows\System32\notepad.exe"),
                Some(1_700_000_000_000),
            ),
        );
        match out {
            Adoption::Forget {
                reason,
                keep_running,
            } => {
                assert!(keep_running);
                assert!(reason.contains("复用"), "{reason}");
            }
            Adoption::Adopt => panic!("must not adopt a reused pid"),
        }
    }

    #[test]
    fn same_exe_different_age_is_rejected() {
        let old = decide(
            &rec(1),
            &probe(
                Some(r"C:\PHL\runtimes\node-22\node.exe"),
                Some(1_700_000_000_000 + 60 * 60 * 1000),
            ),
        );
        assert!(matches!(
            old,
            Adoption::Forget {
                keep_running: true,
                ..
            }
        ));
    }

    #[test]
    fn matching_identity_and_age_is_adopted() {
        // Windows reports the verbatim `\\?\` form; the record stores the
        // plain path. Comparison must ignore prefix, separators and case.
        let out = decide(
            &rec(1),
            &probe(
                Some(r"\\?\C:\PHL\Runtimes\NODE-22\node.exe"),
                Some(1_700_000_000_000 + 400),
            ),
        );
        assert_eq!(out, Adoption::Adopt);
    }

    #[tokio::test]
    async fn the_latest_launch_log_is_the_newest_name() {
        let dir = std::env::temp_dir().join(format!("phl-reg-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("launch-2026-09-06T10-00-00.log"), b"a").unwrap();
        std::fs::write(dir.join("launch-2026-09-06T12-30-00.log"), b"b").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        let found = latest_launch_log(&dir).await.unwrap();
        assert!(found
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("12-30"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("phl-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("processes.json");

        let reg = Registry::default();
        reg.bind(path.clone());
        reg.remember(rec(4321));
        let fresh = Registry::default();
        fresh.bind(path.clone());
        let all = fresh.snapshot();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pid, 4321);

        fresh.forget_pid("main", 999);
        assert_eq!(fresh.snapshot().len(), 1, "wrong pid must not forget");
        fresh.forget_pid("main", 4321);
        assert!(fresh.snapshot().is_empty());
        let after_reload = Registry::default();
        after_reload.bind(path);
        assert!(after_reload.snapshot().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
