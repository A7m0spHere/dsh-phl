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
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::paths::PhlState;
use crate::versions::{now_iso, sanitize_version};

pub(crate) mod process;
pub(crate) mod registry;

use registry::ProcessState;
pub(crate) use registry::{
    decide, latest_launch_log, probe_process, Adoption, PersistedProcess, Registry,
};

pub(crate) use process::{
    allocate_port, build_command, kill_tree, log_tail, node_binary, read_web_url, resolve_node,
    CREATE_NO_WINDOW,
};
#[cfg(test)]
pub(crate) use process::{check_kill_output, parse_web_url, port_free, redact_web_token};

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
    fn take(&self, id: &str) -> Arc<AtomicBool> {
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

    fn release(&self, id: &str) {
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
    let result = run_launch(
        &app,
        &processes,
        &registry,
        &cancel,
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
fn ensure_launch_available(
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
/// touching any real DSH process.
async fn confirm_exit(mut probe: impl FnMut() -> ProcessState, window: Duration) -> bool {
    let deadline = Instant::now() + window;
    loop {
        if probe() == ProcessState::Exited {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

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
    let kill = kill_tree(pid).await;
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
fn stop_permission(registry: &Registry, instance_id: &str, pid: u32) -> Result<(), String> {
    let Some(rec) = registry.record_of(instance_id) else {
        return Ok(());
    };
    match decide(&rec, &probe_process(pid)) {
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
async fn terminate_or_gone(
    processes: &Processes,
    registry: &Registry,
    instance_id: &str,
    pid: u32,
) -> Result<(), String> {
    let kill = kill_tree(pid).await;
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
async fn run_launch(
    app: &AppHandle,
    processes: &Processes,
    registry: &Registry,
    cancel: &AtomicBool,
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
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("无法打开日志文件: {e}"))?;

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
            command.env(key, value);
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

    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 DSH 进程: {e}"))?;
    let pid = child.id().ok_or("进程已启动但没有 PID")?;
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // Record the executable *as the kernel sees it*: `node-system` spawns a
    // bare `node` through PATH, and adoption compares against the full image
    // path — storing the argv spelling would reject every such child.
    let exe_path = probe_process(pid)
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
    });

    let started = Instant::now();
    loop {
        if Launches::is_cancelled(cancel) {
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
                    watch_child_process(app.clone(), instance_id.clone(), pid, child);
                    Err(e)
                }
            };
        }
        if let Ok(Some(status)) = child.try_wait() {
            processes.remove_if_pid(&instance_id, pid);
            registry.forget_pid(&instance_id, pid);
            let tail = log_tail(&log_path, 30).await;
            return Err(format!(
                "DSH 进程在就绪前退出（code {}）。\n--- 日志末尾 ---\n{tail}",
                status.code().unwrap_or(-1)
            ));
        }
        // A listening socket is the readiness signal; HTTP shape is DSH's
        // business, not ours.
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        let elapsed = started.elapsed();
        if elapsed >= READY_TIMEOUT {
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
                    watch_child_process(app.clone(), instance_id.clone(), pid, child);
                    Err(format!("{e}。\n--- 日志末尾 ---\n{tail}"))
                }
            };
        }
        let _ = on_progress.send(LaunchEvent {
            stage: "await-ready".into(),
            progress: (0.36 + 0.61 * (elapsed.as_secs_f64() / READY_TIMEOUT.as_secs_f64()))
                .min(0.97),
            detail: None,
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

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

    watch_child_process(app.clone(), instance_id.clone(), pid, child);

    // `dsh web` prints its authenticated URL before binding; a short poll
    // allows for delayed log flushing.
    let web_url = read_web_url(&log_path).await;
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
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;

    #[test]
    fn deps_selfheal_progress_maps_into_a_monotonic_global_window() {
        // The shape `install_version_deps` really emits: a per-attempt ramp
        // (capped at 0.95) that RESTARTS when a pruned 404 retries, and a
        // final 1.0 on success.
        let phase: Vec<f64> = vec![0.10, 0.55, 0.95, 0.05, 0.80, 0.95, 1.0];
        let mapped: Vec<f64> = phase
            .iter()
            .map(|p| map_launch_phase_progress("install-deps", *p))
            .collect();
        // Inside its window the ramp restarts, so raw mapped values may dip
        // — that is what the launch-side floor absorbs.
        let mut floor = 0.0f64;
        let global: Vec<f64> = mapped
            .iter()
            .map(|m| {
                floor = floor.max(*m);
                floor
            })
            .collect();
        for w in global.windows(2) {
            assert!(w[1] >= w[0], "global launch bar regressed: {global:?}");
        }
        // The window must sit strictly below every later stage's pinned
        // value, or the hand-off regresses at exactly the boundary.
        let deps_ceiling = *global.last().unwrap();
        for later in [0.12, 0.24, 0.30, 0.36, 0.97, 1.0] {
            assert!(
                deps_ceiling < later,
                "install-deps ceiling {deps_ceiling} must stay below the later stage at {later}"
            );
        }
        // Other stages are the global scale already: pass-through.
        for stage in [
            "prepare-home",
            "link-plugins",
            "allocate-port",
            "spawn",
            "await-ready",
        ] {
            assert_eq!(map_launch_phase_progress(stage, 0.42), 0.42);
        }
    }

    #[test]
    fn uncertain_or_live_registration_blocks_a_second_launch() {
        for state in [ProcessState::Alive, ProcessState::Unknown] {
            let processes = Processes::default();
            let registry = Registry::default();
            let rec = PersistedProcess {
                instance_id: "inst".into(),
                pid: 123,
                port: 3099,
                started_at_ms: 1000,
                exe_path: "node.exe".into(),
            };
            registry.remember(rec.clone());
            // Covers a durable record that boot could not adopt into memory.
            let err = ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
                state,
                exe_path: Some("node.exe".into()),
                created_at_ms: Some(1000),
            })
            .unwrap_err();
            assert!(err.contains("kept-alive 123 3099"));
            assert_eq!(registry.record_of("inst").unwrap(), rec);
            assert_eq!(
                processes.entry_of("inst").unwrap().pid,
                123,
                "the retry-stop entry must reach the preserved record"
            );
            processes.set(
                "inst",
                ProcessEntry {
                    pid: 123,
                    port: 3099,
                },
            );
            assert!(ensure_launch_available(&processes, &registry, "inst", |_| {
                registry::Probe::default()
            })
            .is_err());
            assert_eq!(processes.entry_of("inst").unwrap().pid, 123);
            ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
                state: ProcessState::Exited,
                ..Default::default()
            })
            .unwrap();
            assert!(processes.entry_of("inst").is_none());
            assert!(registry.record_of("inst").is_none());
        }
    }

    #[test]
    fn reused_registration_is_forgotten_without_touching_the_other_process() {
        let processes = Processes::default();
        let registry = Registry::default();
        registry.remember(PersistedProcess {
            instance_id: "inst".into(),
            pid: 123,
            port: 3099,
            started_at_ms: 1000,
            exe_path: "node.exe".into(),
        });
        ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
            state: ProcessState::Alive,
            exe_path: Some("unrelated.exe".into()),
            created_at_ms: Some(2000),
        })
        .unwrap();
        assert!(registry.record_of("inst").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn isolated_home_wins_over_instance_and_provider_case_variants() {
        let mut command = tokio::process::Command::new("node");
        command
            .env("DSH_HOME", "initial")
            .env("dsh_home", "other-instance")
            .env("Dsh_Home", "provider-value");
        set_isolated_home(&mut command, Path::new("C:/isolated-instance/dsh-home"));
        let values: Vec<_> = command
            .as_std()
            .get_envs()
            .filter(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("DSH_HOME"))
            .collect();
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].1.unwrap(),
            std::ffi::OsStr::new("C:/isolated-instance/dsh-home")
        );
    }

    #[tokio::test]
    async fn termination_faults_keep_rows_until_exit_is_observed() {
        for kill_ok in [false, true] {
            for state in [
                ProcessState::Alive,
                ProcessState::Unknown,
                ProcessState::Exited,
            ] {
                let processes = Processes::default();
                let registry = Registry::default();
                processes.set(
                    "inst",
                    ProcessEntry {
                        pid: 123,
                        port: 3099,
                    },
                );
                registry.remember(PersistedProcess {
                    instance_id: "inst".into(),
                    pid: 123,
                    port: 3099,
                    started_at_ms: 0,
                    exe_path: "x".into(),
                });
                let kill = if kill_ok {
                    Ok(())
                } else {
                    Err("injected access denied".into())
                };
                let result = finish_termination(
                    &processes,
                    &registry,
                    "inst",
                    123,
                    kill,
                    || state,
                    Duration::ZERO,
                )
                .await;
                let exited = state == ProcessState::Exited;
                assert_eq!(result.is_ok(), exited, "kill_ok={kill_ok}, state={state:?}");
                assert_eq!(processes.entry_of("inst").is_none(), exited);
                assert_eq!(registry.record_of("inst").is_none(), exited);
                // A later retry with an actual exit observation is sufficient,
                // even when the kill command still fails against the dead pid.
                finish_termination(
                    &processes,
                    &registry,
                    "inst",
                    123,
                    Err("already dead".into()),
                    || ProcessState::Exited,
                    Duration::ZERO,
                )
                .await
                .unwrap();
                assert!(registry.record_of("inst").is_none());
            }
        }
    }

    #[tokio::test]
    async fn kept_child_exit_clears_rows_and_delivers_one_notification() {
        let child = if cfg!(windows) {
            tokio::process::Command::new("cmd")
                .args(["/C", "exit 7"])
                .spawn()
                .unwrap()
        } else {
            tokio::process::Command::new("sh")
                .args(["-c", "exit 7"])
                .spawn()
                .unwrap()
        };
        let pid = child.id().unwrap();
        let processes = Processes::default();
        let registry = Registry::default();
        processes.set("inst", ProcessEntry { pid, port: 3099 });
        registry.remember(PersistedProcess {
            instance_id: "inst".into(),
            pid,
            port: 3099,
            started_at_ms: 0,
            exe_path: "x".into(),
        });
        assert!(finish_termination(
            &processes,
            &registry,
            "inst",
            pid,
            Err("injected kill timeout".into()),
            || ProcessState::Unknown,
            Duration::ZERO
        )
        .await
        .is_err());
        let mut events = Vec::new();
        observe_child_exit(child, |status| {
            assert!(forget_exited_process(&processes, &registry, "inst", pid));
            events.push((pid, status.code()));
        })
        .await;
        assert_eq!(events, vec![(pid, Some(7))]);
        assert!(processes.entry_of("inst").is_none());
        assert!(registry.record_of("inst").is_none());
    }

    #[test]
    fn failed_kill_command_is_not_success() {
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(1)
        };
        #[cfg(not(windows))]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(256)
        };
        let error = check_kill_output(
            123,
            std::process::Output {
                status,
                stdout: vec![],
                stderr: b"access denied".to_vec(),
            },
        )
        .unwrap_err();
        assert!(error.contains("123"));
        assert!(error.contains("access denied"));
    }

    #[test]
    fn port_zero_is_never_considered_free() {
        // `bind(0)` means "any ephemeral port" and always succeeds, so without
        // the explicit guard the allocator could hand DSH `--port 0`.
        assert!(!port_free(0));
    }

    #[test]
    fn auto_allocation_stops_at_the_top_of_the_range() {
        let processes = Processes::default();
        // Starting at 65535 there is nowhere to advance to; the old
        // `wrapping_add` rolled over to 0 and returned it as usable.
        let picked = allocate_port(&processes, "inst", 65535, true);
        // Failing to find a port is a fine outcome here; returning 0 is not.
        if let Ok(port) = picked {
            assert_ne!(port, 0, "port 0 must never be allocated");
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-launch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn web_url_parsed_from_dsh_log_line() {
        let log = "some boot noise\n\
                   dsh web: http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w\n\
                   later output here";
        assert_eq!(
            parse_web_url(log).as_deref(),
            Some("http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w")
        );
        // No URL line → None (frontend falls back to bare host:port).
        assert_eq!(parse_web_url("booting…\nready\n"), None);
        // A line that only mentions "http" elsewhere must not be mistaken.
        assert_eq!(parse_web_url("see docs at http://example.com"), None);
    }

    #[tokio::test]
    async fn read_web_url_finds_line_after_port_binds() {
        let dir = std::env::temp_dir().join(format!("phl-weburl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launch.log");
        std::fs::write(&path, "dsh web: http://127.0.0.1:4000/?token=abc123\n").unwrap();
        assert_eq!(
            read_web_url(&path).await.as_deref(),
            Some("http://127.0.0.1:4000/?token=abc123")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_is_built_for_the_web_entry() {
        let root = temp_dir("cmd");
        std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
        std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();
        std::fs::create_dir_all(root.join("runtimes/node-22")).unwrap();
        std::fs::write(root.join("runtimes/node-22/node.exe"), "bin").unwrap();

        let dsh_home = root.join("instances/i/dsh-home");
        let (program, args, env) = build_command(
            &root,
            "0.1.0",
            "node-22",
            3081,
            &["--verbose".into()],
            &dsh_home,
        )
        .unwrap();

        assert!(program.to_string_lossy().ends_with("node.exe"));
        // User args first, then our --port / --no-open — a duplicate --port
        // from the instance config can never override PHL's allocation.
        let expected_bin = root
            .join("versions")
            .join("0.1.0")
            .join("lib")
            .join("bin.js");
        assert_eq!(
            args,
            vec![
                expected_bin.to_string_lossy().into_owned(),
                "web".into(),
                "--verbose".into(),
                "--port".into(),
                "3081".into(),
                "--no-open".into(),
            ]
        );
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "DSH_HOME")
                .map(|(_, v)| v.as_str()),
            Some(dsh_home.to_string_lossy().as_ref())
        );
        // The runtime bin dir leads PATH so child processes resolve this node.
        let path = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
        assert!(path.starts_with(
            root.join("runtimes")
                .join("node-22")
                .to_string_lossy()
                .as_ref()
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_version_or_runtime_is_refused_up_front() {
        let root = temp_dir("missing");
        std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
        std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();

        let dsh_home = root.join("instances/i/dsh-home");
        let err = build_command(&root, "0.1.0", "node-22", 3080, &[], &dsh_home).unwrap_err();
        assert!(err.contains("Runtime node-22 未安装"), "{err}");

        std::fs::create_dir_all(root.join("runtimes/node-22")).unwrap();
        std::fs::write(root.join("runtimes/node-22/node.exe"), "bin").unwrap();
        std::fs::remove_dir_all(root.join("versions/0.1.0")).unwrap();
        let err = build_command(&root, "0.1.0", "node-22", 3080, &[], &dsh_home).unwrap_err();
        assert!(err.contains("lib/bin.js"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn system_runtime_runs_path_node() {
        let root = temp_dir("sys");
        std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
        std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();
        let (program, _, env) =
            build_command(&root, "0.1.0", "node-system", 3080, &[], &root.join("h")).unwrap();
        assert_eq!(program, PathBuf::from("node"));
        assert!(
            env.iter().all(|(k, _)| k != "PATH"),
            "system node is already on PATH"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn port_allocation_skips_phl_and_os_conflicts() {
        let processes = Processes::default();
        processes
            .0
            .lock()
            .unwrap()
            .insert("other".into(), ProcessEntry { pid: 1, port: 3081 });

        // Fixed port colliding with a PHL instance → next free port under auto…
        assert_eq!(allocate_port(&processes, "me", 3081, true).unwrap(), 3082);
        // …but refused as-is when auto is off.
        let err = allocate_port(&processes, "me", 3081, false).unwrap_err();
        assert!(err.contains("已被实例 other 占用"), "{err}");

        // An OS-level listener is skipped by the bind test too.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied = listener.local_addr().unwrap().port();
        let picked = allocate_port(&processes, "me", occupied, true).unwrap();
        assert_ne!(picked, occupied);
        assert!(port_free(picked));

        // The instance's own recorded port (restart-in-place) is not a conflict,
        // provided the OS also considers it free.
        let own = allocate_port(&processes, "me", 20_000, true).unwrap();
        processes
            .0
            .lock()
            .unwrap()
            .insert("me".into(), ProcessEntry { pid: 2, port: own });
        assert_eq!(allocate_port(&processes, "me", own, true).unwrap(), own);

        drop(listener);
    }

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let dir = temp_dir("tail");
        let path = dir.join("launch.log");
        std::fs::write(&path, "l1\nl2\nl3\n").unwrap();
        let tail = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(log_tail(&path, 2));
        assert_eq!(tail, "l2\nl3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_webui_token_never_leaves_the_backend() {
        // `dsh web` prints the authenticated URL at the top of the log, and a
        // short log puts that line inside the tail PHL shows the user.
        let line = "dsh web: http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w\nready";
        let redacted = redact_web_token(line);
        assert!(!redacted.contains("6qeWvc"), "{redacted}");
        assert!(redacted.contains("token=<redacted>"));
        assert!(redacted.contains("ready"), "the rest of the log survives");

        // A token that ends at a query separator keeps the rest of the query.
        assert_eq!(
            redact_web_token("http://h/?token=abc&x=1"),
            "http://h/?token=<redacted>&x=1"
        );
        assert_eq!(redact_web_token("nothing to hide"), "nothing to hide");

        // And the tail path itself redacts, not just the helper.
        let dir = temp_dir("tail-token");
        let path = dir.join("launch.log");
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let tail = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(log_tail(&path, 10));
        assert!(!tail.contains("6qeWvc"), "{tail}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn log_tail_of_a_huge_file_reads_only_the_window() {
        // O-12: asking for 3 lines out of a log that has megabytes of
        // noise must not materialise the whole file. Write well past the
        // tail window and check only the last three lines come back.
        let dir = temp_dir("tail-big");
        let path = dir.join("launch.log");
        let mut raw = vec![b'x'; process::LOG_TAIL_WINDOW as usize + 4096];
        raw.extend_from_slice(b"\nline-a\nline-b\nline-c\n");
        std::fs::write(&path, &raw).unwrap();
        let tail = log_tail(&path, 3).await;
        assert_eq!(tail, "line-a\nline-b\nline-c");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn pruning_keeps_the_newest_logs_and_drops_the_rest() {
        let dir = temp_dir("prune");
        for i in 0..5 {
            std::fs::write(dir.join(format!("launch-2026-09-06T0{i}-00-00.log")), b"x").unwrap();
        }
        std::fs::write(dir.join("other.log"), b"x").unwrap();
        process::prune_launch_logs(&dir, 2).await;
        assert!(dir.join("launch-2026-09-06T04-00-00.log").exists());
        assert!(dir.join("launch-2026-09-06T03-00-00.log").exists());
        assert!(!dir.join("launch-2026-09-06T00-00-00.log").exists());
        assert!(dir.join("other.log").exists(), "only launch-*.log is swept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_pid_never_removes_the_fresh_process_entry() {
        // The relaunch race, pinned: stop + instant re-launch puts a new pid
        // in the map; the old watcher's delayed removal must not forget it
        // (that would orphan the live process from PHL's bookkeeping).
        let processes = Processes::default();
        processes.set(
            "inst",
            ProcessEntry {
                pid: 111,
                port: 3080,
            },
        );
        processes.remove_if_pid("inst", 999); // unrelated stale pid
        assert_eq!(
            processes.entry_of("inst").unwrap().pid,
            111,
            "stale removal refused"
        );
        processes.remove_if_pid("inst", 111);
        assert!(
            processes.entry_of("inst").is_none(),
            "the owning pid still removes"
        );
        // Removal after disappearance is inert, not a panic.
        processes.remove_if_pid("inst", 111);
    }

    #[tokio::test]
    async fn termination_is_forgotten_only_once_exit_is_confirmed() {
        // R3's gate, exercised against real process states: a pid the kernel
        // reports gone passes; a live process whose kill_tree succeeds passes;
        // either way the in-memory map and the persisted row move together.
        let processes = Processes::default();
        let registry = Registry::default();
        let mut gone = if cfg!(windows) {
            tokio::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .unwrap()
        } else {
            tokio::process::Command::new("true").spawn().unwrap()
        };
        let pid = gone.id().unwrap();
        gone.wait().await.unwrap();
        processes.set("inst", ProcessEntry { pid, port: 3099 });
        registry.remember(PersistedProcess {
            instance_id: "inst".into(),
            pid,
            port: 3099,
            started_at_ms: 0,
            exe_path: "x".into(),
        });
        terminate_and_forget(
            &processes,
            &registry,
            "inst",
            pid,
            3099,
            "启动已取消",
            &mut gone,
        )
        .await
        .unwrap();
        assert!(processes.entry_of("inst").is_none());
        assert!(
            registry.record_of("inst").is_none(),
            "confirmed exit: forgotten"
        );

        // A live child that *can* be terminated also clears both rows — the
        // proof is the probe saying dead within the confirmation window.
        let mut sleeper = if cfg!(windows) {
            tokio::process::Command::new("cmd")
                .args(["/C", "ping -n 60 127.0.0.1 > nul"])
                .spawn()
                .unwrap()
        } else {
            tokio::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap()
        };
        let spid = sleeper.id().unwrap();
        processes.set(
            "alive",
            ProcessEntry {
                pid: spid,
                port: 3100,
            },
        );
        registry.remember(PersistedProcess {
            instance_id: "alive".into(),
            pid: spid,
            port: 3100,
            started_at_ms: 0,
            exe_path: "x".into(),
        });
        terminate_and_forget(
            &processes,
            &registry,
            "alive",
            spid,
            3100,
            "等待 120 秒仍未就绪",
            &mut sleeper,
        )
        .await
        .unwrap();
        assert!(
            registry.record_of("alive").is_none(),
            "kill confirmed dead: registration cleared"
        );
        assert!(processes.entry_of("alive").is_none());
        let _ = sleeper.wait().await;
    }

    #[tokio::test]
    async fn a_stop_of_a_process_that_died_after_the_keep_clears_the_rows() {
        // The kept-alive instance's retry-stop path (R3): `terminate_or_gone`
        // must treat a kill failure against an already-dead pid as the desired
        // outcome and clear both rows. Whether taskkill itself succeeds or
        // errors on a dead pid varies by state, so the assertion is the END
        // — rows gone, command Ok — whichever branch the OS took.
        let processes = Processes::default();
        let registry = Registry::default();
        let mut gone = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = gone.id();
        gone.wait().unwrap();
        processes.set("inst", ProcessEntry { pid, port: 3099 });
        registry.remember(PersistedProcess {
            instance_id: "inst".into(),
            pid,
            port: 3099,
            started_at_ms: 0,
            exe_path: "x".into(),
        });
        // `stop_permission`'s decide() gate accepts a dead pid, and the stop
        // core clears the rows against it.
        assert!(stop_permission(&registry, "inst", pid).is_ok());
        terminate_or_gone(&processes, &registry, "inst", pid)
            .await
            .expect("a dead pid must never block the stop retry");
        assert!(processes.entry_of("inst").is_none());
        assert!(registry.record_of("inst").is_none());
    }

    #[test]
    fn stop_refuses_a_pid_the_registry_does_not_own() {
        let registry = Registry::default();
        // No record: a process PHL launched in this session is ours.
        assert!(stop_permission(&registry, "inst", 1234).is_ok());

        // A record whose creation stamp cannot belong to this pid: the number
        // has been reused, so PHL must refuse to kill it.
        registry.remember(PersistedProcess {
            instance_id: "inst".into(),
            pid: 1234,
            port: 3080,
            started_at_ms: 0,
            exe_path: "C:\\node.exe".into(),
        });
        let err = stop_permission(&registry, "inst", std::process::id()).unwrap_err();
        assert!(err.contains("身份无法确认"), "{err}");

        // A pid that is already gone is not a kill target, but must not block
        // the cleanup either.
        assert!(stop_permission(&registry, "inst", 1234).is_ok());
    }

    #[test]
    fn a_watcher_only_closes_the_window_of_its_own_process() {
        // The other half of the relaunch race: the old watcher must not close
        // the WebUI window that belongs to the new process, while a plain stop
        // (which removes the entry itself) still has to close it.
        let processes = Processes::default();
        processes.set(
            "inst",
            ProcessEntry {
                pid: 111,
                port: 3080,
            },
        );
        assert!(processes.is_current_or_empty("inst", 111), "its own pid");

        // A relaunch registered the new pid before the old watcher woke up.
        processes.set(
            "inst",
            ProcessEntry {
                pid: 222,
                port: 3081,
            },
        );
        assert!(
            !processes.is_current_or_empty("inst", 111),
            "the stale watcher must leave the new window alone"
        );
        assert!(processes.is_current_or_empty("inst", 222));

        // Stop removes the entry itself; "nothing registered" is not "newer".
        processes.remove_if_pid("inst", 222);
        assert!(processes.is_current_or_empty("inst", 222));
        assert!(processes.is_current_or_empty("never-seen", 1));
    }
}
