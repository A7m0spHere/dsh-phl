//! Process / Port Manager: real DSH launch, stop and crash detection.
//!
//! Verified against `@deepseek-ai/dsh` 0.1.1-rc.2 (bin.js, web startup,
//! profile boot):
//!
//! - `dsh web` is the only entry that parses `--port`; it boots
//!   `$DSH_HOME/profiles/web`. The `--profile <name>` path parses no port, so
//!   per-instance port isolation goes through the `web` subcommand.
//! - `DSH_HOME` is resolved per call via `resolveDshHome()`, whose doc notes
//!   it "may be set by the test or **launcher** after import" — setting it on
//!   the spawned process is the sanctioned isolation point.
//! - `--no-open` keeps DSH from opening a browser itself; PHL owns the
//!   「打开 WebUI」 action instead.
//!
//! So a launch is `<runtime>/node <version>/lib/bin.js web --port <p>
//! --no-open` with `DSH_HOME=<instance>/dsh-home`. Plugins load from
//! `profiles/web/node_modules` of that home — exactly where the plugin
//! installer puts them, which is why instances pin profile `web`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::paths::PhlState;
use crate::versions::{now_iso, sanitize_version};

pub(crate) mod probe;
pub(crate) mod process;
pub(crate) mod registry;

pub(crate) use registry::{
    decide, latest_launch_log, probe_process, Adoption, PersistedProcess, Probe, ProcessState,
    Registry,
};

pub(crate) use process::{
    allocate_port, build_command, confirm_exit, kill_tree, log_tail, node_binary,
    read_web_url_once, resolve_node, terminate_tree, TERM_GRACE,
};
// Only defined (and only referenced, at `creation_flags` call sites) on
// Windows; an unconditional re-export breaks `cargo check` on macOS.
#[cfg(windows)]
pub(crate) use process::CREATE_NO_WINDOW;
#[cfg(test)]
pub(crate) use process::{
    check_kill_output, parse_web_url, port_free, redact_web_token, MIN_WEB_PORT,
};

/// Emitted when a launched DSH process exits for any reason — crash, manual
/// stop, normal shutdown. The frontend folds this into the instance's UI
/// state, so a crash and a stop take the same road.
const INSTANCE_EXITED: &str = "phl://instance-exited";

/// DSH gives up on a wedged boot after this long; the log tail travels with
/// the error so the failure is diagnosable without opening the file.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/* ----------------------------- wire types ----------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOutcome {
    pub pid: u32,
    pub port: u16,
    /// The authenticated URL `dsh web` prints to its log. Opening the bare
    /// host:port hits "dsh web authentication required", so the launcher
    /// hands the full tokenised URL to the frontend and "打开 WebUI" uses it.
    pub web_url: Option<String>,
}

/// `stage` mirrors the frontend `LaunchPhase` union.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchEvent {
    pub stage: String,
    /// 0..1 across the whole launch.
    pub progress: f64,
    pub detail: Option<String>,
}

/// The slice of the global launch timeline owned by the install-deps
/// self-heal. `prepare-home` (the next stage) is pinned at 0.12, so the
/// window must stay below that floor with room to spare.
const DEPS_PROGRESS_WINDOW: (f64, f64) = (0.02, 0.10);

/// Map a *phase-local* 0..1 progress (what `install_version_deps` reports:
/// an indeterminate ramp per npm attempt, ending at 1.0 on success) onto the
/// global launch timeline. Handing the raw value to the frontend is what made
/// the launch bar regress — `95% → 100% → prepare-home 12%` — because the
/// installer's number is progress *inside* its phase, not across the launch.
pub(crate) fn map_launch_phase_progress(stage: &str, phase_progress: f64) -> f64 {
    match stage {
        "install-deps" => {
            let (lo, hi) = DEPS_PROGRESS_WINDOW;
            lo + (hi - lo) * phase_progress.clamp(0.0, 1.0)
        }
        _ => phase_progress,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessEntry {
    pub pid: u32,
    pub port: u16,
}

impl Processes {
    pub(crate) fn set(&self, instance_id: &str, entry: ProcessEntry) {
        self.0
            .lock()
            .expect("processes lock")
            .insert(instance_id.to_string(), entry);
    }

    /// Remove only if the entry still points at this pid — a relaunched
    /// instance's fresh entry must never be deleted by a stale watcher or a
    /// stale stop.
    pub(crate) fn remove_if_pid(&self, instance_id: &str, pid: u32) {
        let mut map = self.0.lock().expect("processes lock");
        if map.get(instance_id).map(|e| e.pid) == Some(pid) {
            map.remove(instance_id);
        }
    }

    /// True when `pid` is still the instance's current process, or when no
    /// process is registered at all. A watcher uses this to decide whether it
    /// may close the instance's WebUI window: a *newer* launch must not lose
    /// its window to the previous process's exit, while a plain stop (which
    /// removes the entry itself) still must.
    pub(crate) fn is_current_or_empty(&self, instance_id: &str, pid: u32) -> bool {
        self.0
            .lock()
            .expect("processes lock")
            .get(instance_id)
            .map_or(true, |entry| entry.pid == pid)
    }

    pub(crate) fn entry_of(&self, instance_id: &str) -> Option<ProcessEntry> {
        self.0
            .lock()
            .expect("processes lock")
            .get(instance_id)
            .cloned()
    }
}

/// instance id → live process. Memory is the working set; `registry` mirrors
/// it to disk so a PHL restart can re-adopt the children it launched (see
/// `registry` and `adopt_processes`).
#[derive(Default)]
pub struct Processes(pub Mutex<HashMap<String, ProcessEntry>>);

/// Per-attempt cancel flags, the same shape as `versions::Transfers`: ids are
/// unique per attempt, so a cancel racing the spawn is still observed and a
/// finished launch leaves a flag nobody will read again.
#[derive(Default)]
pub struct Launches(pub Mutex<HashMap<String, Arc<AtomicBool>>>);

impl Launches {
    // `pub(crate)` for the release E2E gate's launch shell, which mirrors the
    // `launch_instance` command's flag bookkeeping without a Tauri runtime.
    pub(crate) fn take(&self, id: &str) -> Arc<AtomicBool> {
        self.0
            .lock()
            .expect("launches lock")
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    fn cancel(&self, id: &str) {
        self.0
            .lock()
            .expect("launches lock")
            .entry(id.to_string())
            .or_default()
            .store(true, Ordering::SeqCst);
    }

    pub(crate) fn release(&self, id: &str) {
        self.0.lock().expect("launches lock").remove(id);
    }

    fn is_cancelled(flag: &AtomicBool) -> bool {
        flag.load(Ordering::SeqCst)
    }
}

/* ------------------------------ commands ------------------------------ */

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn launch_instance(
    app: AppHandle,
    launches: State<'_, Launches>,
    processes: State<'_, Processes>,
    registry: State<'_, Registry>,
    phl: State<'_, PhlState>,
    locks: State<'_, crate::resources::ResourceLocks>,
    creds: State<'_, crate::credentials::Creds>,
    transfer_id: String,
    instance_id: String,
    version_name: String,
    runtime_name: String,
    registry_base: String,
    profile: String,
    port: u16,
    auto_port: bool,
    env: HashMap<String, String>,
    args: Vec<String>,
    // The UI's current binding, authoritative against the manifest: an
    // edit-save persists through a fire-and-forget promise, so a launch
    // fired immediately after could otherwise read a stale manifest.
    api: Option<crate::api_config::ApiBinding>,
    on_progress: Channel<LaunchEvent>,
) -> Result<LaunchOutcome, String> {
    let instance_id = sanitize_version(&instance_id)?;
    let _held = locks
        .acquire(&[crate::resources::Resource::Instance(instance_id.clone())])
        .map_err(|e| crate::errors::coded(crate::errors::ErrCode::Busy, e.to_string()))?;
    ensure_launch_available(&processes, &registry, &instance_id, probe_process)?;
    let cancel = launches.take(&transfer_id);
    // The one job the concrete `AppHandle` does inside `run_launch`: hand the
    // retained child to the GUI exit watcher. Factoring it into a callback
    // keeps the startup pipeline itself runtime-free, so the release E2E
    // gate can drive the *same* code headlessly (`release_e2e::driver`).
    let retain_child = {
        let app = app.clone();
        move |id: &str, pid: u32, child: tokio::process::Child| {
            watch_child_process(app.clone(), id.to_string(), pid, child)
        }
    };
    let result = run_launch(
        &processes,
        &registry,
        &cancel,
        &retain_child,
        &phl.root(),
        &instance_id,
        &version_name,
        &runtime_name,
        &registry_base,
        &profile,
        port,
        auto_port,
        env,
        args,
        api,
        &creds,
        &on_progress,
    )
    .await;
    launches.release(&transfer_id);
    result
}

/// The UI can be stale after an uncertain adoption or a failed cancellation.
/// Check the registration under the instance lock before any startup writes:
/// another spawn must not overwrite the handle of an unconfirmed process.
/// `pub(crate)` for the release E2E gate's launch shell (`release_e2e::driver`).
pub(crate) fn ensure_launch_available(
    processes: &Processes,
    registry: &Registry,
    id: &str,
    probe: impl FnOnce(u32) -> registry::Probe,
) -> Result<(), String> {
    let record = registry.record_of(id);
    let entry = processes.entry_of(id).or_else(|| {
        record.as_ref().map(|r| ProcessEntry {
            pid: r.pid,
            port: r.port,
        })
    });
    let Some(entry) = entry else { return Ok(()) };
    let observed = probe(entry.pid);
    let reused = observed.state == ProcessState::Alive
        && record.as_ref().is_some_and(|r| {
            r.pid == entry.pid
                && matches!(
                    decide(r, &observed),
                    Adoption::Forget {
                        keep_running: true,
                        ..
                    }
                )
                && observed.exe_path.is_some()
                && observed.created_at_ms.is_some()
        });
    if observed.state == ProcessState::Exited || reused {
        processes.remove_if_pid(id, entry.pid);
        registry.forget_pid(id, entry.pid);
        return Ok(());
    }
    // Also reserve a durable-only record so the kept-alive UI's stop action
    // reaches this PID. stop_permission still refuses unknown identities.
    processes.set(
        id,
        ProcessEntry {
            pid: entry.pid,
            port: entry.port,
        },
    );
    Err(crate::errors::coded(
        crate::errors::ErrCode::State,
        format!(
            "kept-alive {} {}：该实例仍有未退出或无法确认退出的进程登记，未重复启动；请先重试停止",
            entry.pid, entry.port,
        ),
    ))
}

fn set_isolated_home(command: &mut tokio::process::Command, home: &Path) {
    // Must run after all instance/provider environment injection. On Windows
    // Command treats DSH_HOME and dsh_home as the same key.
    command.env("DSH_HOME", home);
}

/// How long to wait for a force-killed process to be confirmed gone before
/// declaring termination unverified (R3). `taskkill /F` returning success is
/// a request honored, not an exit observed.
const EXIT_CONFIRM_WINDOW: Duration = Duration::from_secs(5);

/// Only an observed exit authorizes removing a registration. The injected
/// probe and window also let tests exercise denied/hung termination without
/// touching any real DSH process. (`confirm_exit` itself lives in `process`:
/// the stop path's confirm window and the grace before a hard kill are the
/// same question asked twice.)
#[allow(clippy::too_many_arguments)]
async fn finish_termination(
    processes: &Processes,
    registry: &Registry,
    instance_id: &str,
    pid: u32,
    kill: Result<(), String>,
    probe: impl FnMut() -> ProcessState,
    window: Duration,
) -> Result<(), String> {
    if confirm_exit(probe, window).await {
        processes.remove_if_pid(instance_id, pid);
        registry.forget_pid(instance_id, pid);
        return Ok(());
    }
    Err(kill
        .err()
        .unwrap_or_else(|| format!("进程 {pid} 的退出尚未确认")))
}

/// Terminate a process we launched and forget its registration — but the
/// forgetting is *conditional on confirmed exit*. Both in-memory and
/// persisted state are kept when the kill failed or the pid is still alive:
/// the port stays reserved, the UI keeps a stoppable handle, a relaunch
/// cannot strand a second live DSH, and the next PHL boot can still adopt
/// it. This is `stop_instance`'s read-kill-then-forget rule applied to the
/// launch-failure paths, which used to ignore `kill_tree`'s result and
/// delete the rows regardless.
async fn terminate_and_forget(
    processes: &Processes,
    registry: &Registry,
    instance_id: &str,
    pid: u32,
    port: u16,
    context: &str,
    child: &mut tokio::process::Child,
) -> Result<(), String> {
    let kill = terminate_tree(pid, TERM_GRACE).await;
    // The retained child handle refers to this launch, even if querying the
    // numeric pid is denied or the OS later reuses that number.
    let why = match finish_termination(
        processes,
        registry,
        instance_id,
        pid,
        kill,
        || match child.try_wait() {
            Ok(Some(_)) => ProcessState::Exited,
            Ok(None) => ProcessState::Alive,
            Err(_) => ProcessState::Unknown,
        },
        EXIT_CONFIRM_WINDOW,
    )
    .await
    {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    // `kept-alive <pid> <port>` is a machine-readable header on a coded
    // message: the repository parses the pid/port from it and maps the launch
    // to a *running, stoppable* instance state (R3's retryable stop entry)
    // instead of a failed start the UI would strand without a stop button.
    // The instance stays registered in memory and on disk, so stop-by-id
    // terminates the real process whenever the user asks.
    Err(crate::errors::coded(
        crate::errors::ErrCode::State,
        format!(
            "kept-alive {pid} {port}：{context}，但{why}；PHL 保留了该实例的登记与端口占用，可稍后重试停止"
        ),
    ))
}

/// May this pid be killed on behalf of `instance_id`?
///
/// Killing is the one irreversible action in PHL, so the same identity gate
/// that decides adoption is applied *before* the kill: the persisted record
/// says which process we launched, and a pid that no longer matches it has
/// been reused by somebody else. A process with no record is ours by
/// construction (PHL started it in this session); a process that is already
/// gone needs no permission at all — the caller still cleans up.
/// (pub(crate) for the release E2E gate: same stop semantics the UI exercises.)
pub(crate) fn stop_permission(
    registry: &Registry,
    instance_id: &str,
    pid: u32,
) -> Result<(), String> {
    stop_permission_with_probe(registry, instance_id, pid, probe_process)
}

fn stop_permission_with_probe(
    registry: &Registry,
    instance_id: &str,
    pid: u32,
    probe: impl FnOnce(u32) -> Probe,
) -> Result<(), String> {
    let Some(rec) = registry.record_of(instance_id) else {
        return Ok(());
    };
    match decide(&rec, &probe(pid)) {
        Adoption::Adopt => Ok(()),
        Adoption::Forget {
            keep_running: true,
            reason,
        } => Err(crate::errors::coded(
            crate::errors::ErrCode::State,
            format!(
                "实例 {instance_id} 的进程身份无法确认（{reason}，PID {pid}）：已保留登记，本次未终止它，可稍后重试停止。"
            ),
        )),
        Adoption::Forget { .. } => Ok(()),
    }
}
/// `stop_instance`'s core: kill the tree, and only forget the rows once the
/// process is verifiably not running. A kill that fails against a pid the
/// kernel already reports gone IS the desired outcome — typically a kept-alive
/// instance (R3) that exited on its own since the launch failure — so the
/// retry succeeds instead of bouncing the user off the same error forever.
/// `pub(crate)` for the release E2E gate's stop shell (`release_e2e::driver`).
pub(crate) async fn terminate_or_gone(
    processes: &Processes,
    registry: &Registry,
    instance_id: &str,
    pid: u32,
) -> Result<(), String> {
    let kill = terminate_tree(pid, TERM_GRACE).await;
    finish_termination(
        processes,
        registry,
        instance_id,
        pid,
        kill,
        || probe_process(pid).state,
        EXIT_CONFIRM_WINDOW,
    )
    .await
}

/// Not running in the map → nothing to do; the exit event (or its absence)
/// keeps the frontend state honest either way.
#[tauri::command]
pub async fn stop_instance(
    processes: State<'_, Processes>,
    registry: State<'_, Registry>,
    instance_id: String,
) -> Result<(), String> {
    // Read, kill, and only then forget. Removing first meant a failed
    // `taskkill` (elevated child, access denied) left the process alive with
    // PHL no longer tracking it: its port looked free, the close guard stopped
    // listing it, and nothing in the UI could stop it any more.
    let entry = processes.entry_of(&instance_id);
    if let Some(entry) = entry {
        stop_permission(&registry, &instance_id, entry.pid)?;
        terminate_or_gone(&processes, &registry, &instance_id, entry.pid).await?;
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_launch(launches: State<'_, Launches>, transfer_id: String) {
    launches.cancel(&transfer_id);
}

/* --------------------------- restart adoption --------------------------- */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptedProcess {
    pub instance_id: String,
    pub pid: u32,
    pub port: u16,
    pub web_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroppedProcess {
    pub instance_id: String,
    pub pid: u32,
    pub reason: String,
    /// False = the process is gone; true = it may still be running but PHL
    /// will not manage (let alone kill) it. The UI wordings differ.
    pub kept_running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptReport {
    pub adopted: Vec<AdoptedProcess>,
    pub dropped: Vec<DroppedProcess>,
}

/// The boot reconciliation of O-08: children launched by a previous PHL are
/// re-adopted when their identity checks out, and forgotten — without ever
/// being terminated — when it does not. A record whose instance no longer
/// exists in this root is dropped likewise (its directory may have been
/// deleted while PHL was away).
#[tauri::command]
pub async fn adopt_processes<R: tauri::Runtime>(
    app: AppHandle<R>,
    processes: State<'_, Processes>,
    registry: State<'_, Registry>,
    phl: State<'_, PhlState>,
) -> Result<AdoptReport, String> {
    let root = phl.root();
    let mut report = AdoptReport {
        adopted: Vec::new(),
        dropped: Vec::new(),
    };
    for rec in registry.snapshot() {
        let instance_dir = root.join("instances").join(&rec.instance_id);
        if !instance_dir.join("instance.json").exists() {
            // The instance is gone; a stray child from a deleted instance is
            // outside PHL's reach by design — report, never terminate.
            // `forget` clears the row by instance id without the pid guard
            // `forget_pid` uses; that is safe because adoption is the boot
            // chain's last step and runs before any command of this session
            // can have re-registered the instance.
            let alive = probe_process(rec.pid).state != ProcessState::Exited;
            registry.forget(&rec.instance_id);
            report.dropped.push(DroppedProcess {
                instance_id: rec.instance_id,
                pid: rec.pid,
                reason: "实例已不存在".into(),
                kept_running: alive,
            });
            continue;
        }
        let probe = probe_process(rec.pid);
        match decide(&rec, &probe) {
            Adoption::Adopt => {
                processes.set(
                    &rec.instance_id,
                    ProcessEntry {
                        pid: rec.pid,
                        port: rec.port,
                    },
                );
                let web_url = match latest_launch_log(&instance_dir.join("logs")).await {
                    Some(log) => process::read_web_url_once(&log).await,
                    None => None,
                };
                report.adopted.push(AdoptedProcess {
                    instance_id: rec.instance_id.clone(),
                    pid: rec.pid,
                    port: rec.port,
                    web_url,
                });
                // Adopted must not mean unobserved: without this, an adopted
                // DSH that crashes after the PHL restart keeps its entry, its
                // registry row and its WebUI window forever.
                watch_adopted_process(app.clone(), rec);
            }
            Adoption::Forget {
                reason,
                keep_running,
            } => {
                if probe.state != ProcessState::Unknown {
                    registry.forget_pid(&rec.instance_id, rec.pid);
                }
                report.dropped.push(DroppedProcess {
                    instance_id: rec.instance_id,
                    pid: rec.pid,
                    reason,
                    kept_running: keep_running,
                });
            }
        }
    }
    Ok(report)
}

/// An adopted process is not this PHL's child, so there is no `wait()` to
/// await. Poll the same identity gate that adopted it instead: while
/// `decide` still says Adopt the process is ours and alive; the moment it
/// says otherwise (gone, or the pid handed to a stranger) run the launch
/// watcher's cleanup, so an adopted DSH clears its state and its window
/// exactly like one launched in this session.
fn watch_adopted_process<R: tauri::Runtime>(app: AppHandle<R>, rec: PersistedProcess) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let probe = probe_process(rec.pid);
            if probe.state == ProcessState::Unknown
                || matches!(decide(&rec, &probe), Adoption::Adopt)
            {
                continue;
            }
            let instance_id = rec.instance_id.clone();
            let ours = app
                .try_state::<Processes>()
                .map(|processes| processes.is_current_or_empty(&instance_id, rec.pid))
                .unwrap_or(false);
            if let Some(processes) = app.try_state::<Processes>() {
                processes.remove_if_pid(&instance_id, rec.pid);
            }
            if let Some(registry) = app.try_state::<Registry>() {
                registry.forget_pid(&instance_id, rec.pid);
            }
            if ours {
                crate::webui::close_for_instance(&app, &instance_id);
            }
            let _ = app.emit(
                INSTANCE_EXITED,
                serde_json::json!({
                    "instanceId": instance_id,
                    "pid": rec.pid,
                    "code": serde_json::Value::Null,
                }),
            );
            return;
        }
    });
}

/* ------------------------------- launch ------------------------------- */

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_launch(
    processes: &Processes,
    registry: &Registry,
    cancel: &AtomicBool,
    // Hands a *retained* child (kept registration after cancel/timeout, or
    // the successfully launched process) to whoever watches its exit. The GUI
    // app passes `watch_child_process`; the headless gate passes a recorder —
    // the child stays alive and the gate stops it through the production
    // `terminate_or_gone` path.
    retain_child: &(dyn Fn(&str, u32, tokio::process::Child) + Send + Sync),
    root: &Path,
    instance_id: &str,
    version_name: &str,
    runtime_name: &str,
    registry_base: &str,
    profile: &str,
    port: u16,
    auto_port: bool,
    env: HashMap<String, String>,
    args: Vec<String>,
    api: Option<crate::api_config::ApiBinding>,
    creds: &crate::credentials::Creds,
    on_progress: &Channel<LaunchEvent>,
) -> Result<LaunchOutcome, String> {
    // Every one of these is pasted into a path or an argv — same whitelist as
    // everywhere else front-end input lands in the filesystem.
    let instance_id = sanitize_version(instance_id)?;
    let version_name = sanitize_version(version_name)?;
    let runtime_name = sanitize_version(runtime_name)?;
    let profile = sanitize_version(profile)?;
    if Launches::is_cancelled(cancel) {
        return Err("cancelled".into());
    }

    let instance_dir = root.join("instances").join(&instance_id);
    if !instance_dir.join("instance.json").exists() {
        return Err(format!("实例不存在或缺少清单: {instance_id}"));
    }
    if profile != "web" {
        // `dsh web` boots profile `web` and is the only `--port` entry; a
        // differently named profile would start headless and never become
        // ready. Fail loudly instead of launching something the UI can't see.
        return Err(format!(
            "PHL 目前仅支持 web 面启动（--port 只在 `dsh web` 上解析），实例 profile 是「{profile}」"
        ));
    }

    // A version extracted before dependency installs existed (or whose
    // node_modules went missing) can never boot: ESM resolution from
    // `versions/<name>/lib/bin.js` only looks up the version's own tree.
    // Repair it on the spot so existing installs self-heal on first launch.
    let version_dir = root.join("versions").join(&version_name);
    if crate::versions::version_deps_missing(&version_dir) {
        // The installer's ramp restarts per npm attempt, so the *global*
        // timeline is additionally floored at the highest value already sent:
        // within its window the launch bar only ever moves forward.
        let emitted = std::sync::Mutex::new(map_launch_phase_progress("install-deps", 0.0));
        let _ = on_progress.send(LaunchEvent {
            stage: "install-deps".into(),
            progress: *emitted.lock().unwrap(),
            detail: Some(version_name.clone()),
        });
        let node = resolve_node(root, &runtime_name)?;
        let floor = &emitted;
        let version_for_detail = version_name.clone();
        crate::versions::install_version_deps(&node, &version_dir, registry_base, cancel, |p| {
            let mapped = map_launch_phase_progress("install-deps", p);
            let mut last = floor.lock().unwrap();
            if mapped > *last {
                *last = mapped;
            } else {
                return;
            }
            let _ = on_progress.send(LaunchEvent {
                stage: "install-deps".into(),
                progress: mapped,
                detail: Some(version_for_detail.clone()),
            });
        })
        .await
        .map_err(|e| format!("安装 DSH {version_name} 的依赖失败: {e}"))?;
    }

    // The home follows the manifest: a managed copy lives inside the instance
    // directory, an externally-adopted instance points at its own DSH_HOME.
    // Creating `profiles/web` under it is idempotent (external homes already
    // have it; this only matters for a freshly created copy).
    let manifest = crate::instances::read_manifest(&instance_dir).await;
    let dsh_home = match &manifest {
        Some(m) => crate::instances::home_of(&instance_dir, m),
        None => instance_dir.join("dsh-home"),
    };
    let workspace = instance_dir.join("workspace");
    for dir in [dsh_home.join("profiles").join(&profile), workspace.clone()] {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    if !manifest.as_ref().is_some_and(crate::instances::is_external) {
        // launch_instance holds the same instance lock as plugin writes.
        crate::plugins::install::recover_profile_transaction(
            &dsh_home.join("profiles").join(&profile),
        )
        .await?;
    }

    // Pre-launch API handling. Two things happen, and the difference between
    // them is the whole design:
    //  - the UI's binding wins over a stale on-disk manifest (an edit-save is
    //    fire-and-forget and can still be in flight);
    //  - PHL materializes only when the instance has NO settings.yaml at all
    //    (a create that ran without a library, or a deleted file). Anything
    //    that already exists on disk is the instance's truth: local edits are
    //    observed and logged, never rewritten — overwrite is a human click.
    let mut launch_binding: Option<crate::api_config::ApiBinding> = None;
    if let Some(mut manifest) = crate::instances::read_manifest(&instance_dir).await {
        if let Some(api) = api {
            if manifest.api.as_ref() != Some(&api) {
                let _ = crate::instances::set_instance_api(root, &instance_id, &api).await;
                manifest.api = Some(api);
            }
        }
        if let Some(binding) = manifest.api.clone() {
            if binding.inheritance != "none"
                && binding.synced_hash.is_none()
                && !dsh_home.join("settings.yaml").exists()
            {
                if let Some(applied) = crate::api_config::apply_create_binding(
                    root,
                    &instance_dir,
                    &instance_id,
                    binding,
                )
                .await
                {
                    eprintln!("[phl] api config materialized into {instance_id} at launch");
                    manifest.api = Some(applied);
                }
            }
        }
        launch_binding = manifest.api.clone();
        crate::api_config::note_launch_drift(root, &instance_dir, &manifest).await;
    }
    let _ = on_progress.send(LaunchEvent {
        stage: "prepare-home".into(),
        progress: 0.12,
        detail: Some(dsh_home.to_string_lossy().into_owned()),
    });

    // One plugin definition for the whole app: `instances::scan_plugins` is
    // what the instance page lists and what DSH's own paperwork (install
    // marker / cordis mount) says is a plugin. This used to count top-level
    // `node_modules` entries — transitive deps counted, `@scope` collapsed
    // to one, and the launch timeline's "N 个插件" disagreed with the page
    // about the same concept.
    let plugin_count = crate::instances::scan_plugins(&dsh_home.join("profiles").join(&profile))
        .await
        .len();
    let _ = on_progress.send(LaunchEvent {
        stage: "link-plugins".into(),
        progress: 0.24,
        detail: Some(format!("{plugin_count} 个插件")),
    });

    let port = allocate_port(processes, &instance_id, port, auto_port)?;
    let _ = on_progress.send(LaunchEvent {
        stage: "allocate-port".into(),
        progress: 0.3,
        detail: Some(format!(":{port}")),
    });

    let (program, cmd_args, env_pairs) =
        build_command(root, &version_name, &runtime_name, port, &args, &dsh_home)?;
    let _ = on_progress.send(LaunchEvent {
        stage: "spawn".into(),
        progress: 0.36,
        detail: None,
    });

    let logs_dir = instance_dir.join("logs");
    tokio::fs::create_dir_all(&logs_dir)
        .await
        .map_err(|e| e.to_string())?;
    // Rotation/retention: the newest 10 launch logs survive; older ones are
    // swept so a long-lived instance's `logs/` cannot grow without bound.
    process::prune_launch_logs(&logs_dir, 10).await;
    let log_path = logs_dir.join(format!("launch-{}.log", now_iso().replace(':', "-")));
    let log = process::open_launch_log(&log_path).map_err(|e| format!("无法打开日志文件: {e}"))?;

    let mut command = tokio::process::Command::new(&program);
    command.args(&cmd_args);
    for (key, value) in &env_pairs {
        command.env(key, value);
    }
    // Instance env last, so a user-provided variable overrides the defaults —
    // except DSH_HOME: it IS the isolation boundary, and an instance env that
    // quietly redirected it would point the process into some other instance.
    for (key, value) in &env {
        if !key.eq_ignore_ascii_case("DSH_HOME") {
            if runtime_name == "node-system" && key.eq_ignore_ascii_case("PATH") {
                // Keep the instance's PATH first, matching its existing
                // override priority, and append the verified system Node path
                // as a fallback so DSH's child processes can still run Node.
                let launch_path = env_pairs
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default();
                let merged = process::merge_path_values(
                    std::ffi::OsStr::new(value),
                    std::ffi::OsStr::new(launch_path),
                )?;
                command.env("PATH", merged);
            } else {
                command.env(key, value);
            }
        }
    }
    // Launch-time key injection (the cc-switch model): providers bound to
    // this instance may carry a key stored in PHL's local library. DSH reads
    // keys via each provider's `apiKeyEnv`, so we materialize the stored key
    // as that process variable — and only as that: the instance's files
    // never contain the secret. An explicit instance env or a real system
    // variable of the same name always wins; the stored copy is the fallback
    // that makes "paste the key once in PHL" work for every instance.
    if let Some(binding) = &launch_binding {
        if let Some(config) = crate::api_config::load_config_file(root).await {
            for (name, value) in
                crate::api_config::resolve_launch_keys(&config, binding, creds).await
            {
                if env.contains_key(&name) || std::env::var(&name).is_ok() {
                    continue;
                }
                command.env(name, value);
            }
        }
    }
    set_isolated_home(&mut command, &dsh_home);
    command
        .current_dir(&workspace)
        .stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| e.to_string())?,
        ))
        .stderr(std::process::Stdio::from(log));
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    // Its own process group, so a stop can take the whole tree with it. Unix
    // has no `taskkill /T`, and `probe::owned_group_of` only ever signals a
    // group the pid *leads* — which is exactly what this call makes true.
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 DSH 进程: {e}"))?;
    let pid = child.id().ok_or("进程已启动但没有 PID")?;
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // Record the executable *as the kernel sees it*: adoption compares against
    // the full image path, which argv is not — argv can hold a bare name, a
    // wrapper or a symlink.
    let identity = probe_process(pid);
    let exe_path = identity
        .exe_path
        .unwrap_or_else(|| program.to_string_lossy().into_owned());
    processes.set(&instance_id, ProcessEntry { pid, port });
    // The durable half of the pairing: recorded right at spawn so a PHL that
    // crashes a second later still knows what to adopt (or forget) at boot.
    registry.remember(PersistedProcess {
        instance_id: instance_id.clone(),
        pid,
        port,
        started_at_ms,
        exe_path,
        process_start_token: identity.process_start_token,
    });

    // Readiness is the port answering *and* this boot's own `dsh web:` line —
    // see `process::await_readiness` for why the URL is part of it. The loop
    // lives in `process.rs` so the wait itself is unit-testable; the exit
    // reasons come back here, where the termination and log-tail policy is.
    let readiness = process::await_readiness(
        &mut child,
        port,
        &log_path,
        READY_TIMEOUT,
        process::WEB_URL_GRACE,
        cancel,
        &mut |elapsed| {
            let _ = on_progress.send(LaunchEvent {
                stage: "await-ready".into(),
                progress: (0.36 + 0.61 * (elapsed.as_secs_f64() / READY_TIMEOUT.as_secs_f64()))
                    .min(0.97),
                detail: None,
            });
        },
    )
    .await;
    let web_url = match readiness {
        process::Readiness::Ready { web_url } => web_url,
        process::Readiness::Cancelled => {
            // The cancellation outcome must not lie: "cancelled" is fine when
            // the process is verifiably gone, but an abort whose termination
            // could not be confirmed keeps the registration (R3) — the coded
            // `kept-alive` error says so and stays stoppable in the UI.
            return match terminate_and_forget(
                processes,
                registry,
                &instance_id,
                pid,
                port,
                "启动已取消",
                &mut child,
            )
            .await
            {
                Ok(()) => Err("cancelled".into()),
                Err(e) => {
                    retain_child(&instance_id, pid, child);
                    Err(e)
                }
            };
        }
        process::Readiness::Exited { code } => {
            processes.remove_if_pid(&instance_id, pid);
            registry.forget_pid(&instance_id, pid);
            let tail = log_tail(&log_path, 30).await;
            return Err(format!(
                "DSH 进程在就绪前退出（code {}）。\n--- 日志末尾 ---\n{tail}",
                code.unwrap_or(-1)
            ));
        }
        process::Readiness::Timeout => {
            // The old wording claimed the process had been terminated before
            // anyone had observed it. Now the claim depends on proof: the
            // kept-alive case surfaces as a coded, still-stoppable instance.
            let tail = log_tail(&log_path, 30).await;
            return match terminate_and_forget(
                processes,
                registry,
                &instance_id,
                pid,
                port,
                "等待 120 秒仍未就绪",
                &mut child,
            )
            .await
            {
                Ok(()) => Err(format!(
                    "等待 120 秒仍未就绪，已终止进程。\n--- 日志末尾 ---\n{tail}"
                )),
                Err(e) => {
                    retain_child(&instance_id, pid, child);
                    Err(format!("{e}。\n--- 日志末尾 ---\n{tail}"))
                }
            };
        }
    };

    // The connect may have won a race against a process that exited right
    // after opening the socket (or the socket may have been DSH's shutdown
    // linger). One final liveness check before reporting success keeps the
    // frontend from displaying "running" for a corpse.
    if let Ok(Some(status)) = child.try_wait() {
        processes.remove_if_pid(&instance_id, pid);
        registry.forget_pid(&instance_id, pid);
        let tail = log_tail(&log_path, 30).await;
        return Err(format!(
            "DSH 进程在就绪后立即退出（code {}）。\n--- 日志末尾 ---\n{tail}",
            status.code().unwrap_or(-1)
        ));
    }

    let _ = on_progress.send(LaunchEvent {
        stage: "await-ready".into(),
        progress: 1.0,
        detail: Some(format!("localhost:{port}")),
    });

    retain_child(&instance_id, pid, child);

    Ok(LaunchOutcome { pid, port, web_url })
}

/// Keep owning the child handle on every path that retains a registration.
/// A wait error is an unknown observation, never an exit event.
async fn observe_child_exit(
    mut child: tokio::process::Child,
    on_exit: impl FnOnce(std::process::ExitStatus),
) {
    loop {
        match child.wait().await {
            Ok(status) => {
                on_exit(status);
                return;
            }
            Err(e) => {
                eprintln!("[phl] 等待实例退出失败，保留登记并重试: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

fn forget_exited_process(processes: &Processes, registry: &Registry, id: &str, pid: u32) -> bool {
    let ours = processes.is_current_or_empty(id, pid);
    processes.remove_if_pid(id, pid);
    registry.forget_pid(id, pid);
    ours
}

fn watch_child_process<R: tauri::Runtime>(
    watcher_app: AppHandle<R>,
    watcher_id: String,
    pid: u32,
    child: tokio::process::Child,
) {
    // Single source of truth for "no longer running": the watcher removes the
    // map entry (by pid, so a relaunched instance's entry survives) and
    // notifies the frontend for both crashes and clean shutdowns.
    tokio::spawn(observe_child_exit(child, move |status| {
        // Decide before the entry is removed: a fast restart has already
        // registered the new pid, and this exit must not take that window down.
        let ours = match (
            watcher_app.try_state::<Processes>(),
            watcher_app.try_state::<Registry>(),
        ) {
            (Some(processes), Some(registry)) => {
                forget_exited_process(&processes, &registry, &watcher_id, pid)
            }
            _ => false,
        };
        // A dead instance must not keep its embedded WebUI window open:
        // closing the window never stops the process, but the process
        // exiting always closes the window (see `crate::webui`).
        if ours {
            crate::webui::close_for_instance(&watcher_app, &watcher_id);
        }
        let code = status.code();
        let _ = watcher_app.emit(
            INSTANCE_EXITED,
            serde_json::json!({ "instanceId": watcher_id, "pid": pid, "code": code }),
        );
    }));
}

/// Scan a launch log for the `dsh web:` line and lift the URL out of it.
#[cfg(test)]
mod tests;
