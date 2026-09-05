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
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::paths::PhlState;
use crate::versions::{now_iso, sanitize_version};

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

#[derive(Clone, Copy, Debug)]
pub struct ProcessEntry {
    pub pid: u32,
    pub port: u16,
}

/// instance id → live process. In memory on purpose: if PHL itself restarts,
/// the DSH children keep running unmanaged — same as any launcher. The close
/// guard lists what the UI knows is running, not what a previous session left.
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
    phl: State<'_, PhlState>,
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
    let cancel = launches.take(&transfer_id);
    let result = run_launch(
        &app,
        &processes,
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
        &on_progress,
    )
    .await;
    launches.release(&transfer_id);
    result
}

/// Not running in the map → nothing to do; the exit event (or its absence)
/// keeps the frontend state honest either way.
#[tauri::command]
pub async fn stop_instance(
    processes: State<'_, Processes>,
    instance_id: String,
) -> Result<(), String> {
    // Read, kill, and only then forget. Removing first meant a failed
    // `taskkill` (elevated child, access denied) left the process alive with
    // PHL no longer tracking it: its port looked free, the close guard stopped
    // listing it, and nothing in the UI could stop it any more.
    let entry = processes
        .0
        .lock()
        .expect("processes lock")
        .get(&instance_id)
        .cloned();
    if let Some(entry) = entry {
        kill_tree(entry.pid).await?;
        let mut map = processes.0.lock().expect("processes lock");
        if map.get(&instance_id).map(|e| e.pid) == Some(entry.pid) {
            map.remove(&instance_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_launch(launches: State<'_, Launches>, transfer_id: String) {
    launches.cancel(&transfer_id);
}

/* ------------------------------- launch ------------------------------- */

#[allow(clippy::too_many_arguments)]
async fn run_launch(
    app: &AppHandle,
    processes: &Processes,
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
        let _ = on_progress.send(LaunchEvent {
            stage: "install-deps".into(),
            progress: 0.1,
            detail: Some(version_name.clone()),
        });
        let node = resolve_node(root, &runtime_name)?;
        crate::versions::install_version_deps(&node, &version_dir, registry_base, cancel, |p| {
            let _ = on_progress.send(LaunchEvent {
                stage: "install-deps".into(),
                progress: p,
                detail: Some(version_name.clone()),
            });
        })
        .await
        .map_err(|e| format!("安装 DSH {version_name} 的依赖失败: {e}"))?;
    }

    let dsh_home = instance_dir.join("dsh-home");
    let workspace = instance_dir.join("workspace");
    for dir in [dsh_home.join("profiles").join(&profile), workspace.clone()] {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
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

    let plugin_count = count_plugins(&dsh_home.join("profiles").join(&profile)).await;
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
        if key != "DSH_HOME" {
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
            for (name, value) in crate::api_config::provider_launch_keys(&config, binding) {
                if env.contains_key(&name) || std::env::var(&name).is_ok() {
                    continue;
                }
                command.env(name, value);
            }
        }
    }
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
    processes
        .0
        .lock()
        .expect("processes lock")
        .insert(instance_id.clone(), ProcessEntry { pid, port });

    let started = Instant::now();
    loop {
        if Launches::is_cancelled(cancel) {
            // kill_tree, not child.kill(): node is only the root of the tree.
            let _ = kill_tree(pid).await;
            processes
                .0
                .lock()
                .expect("processes lock")
                .remove(&instance_id);
            return Err("cancelled".into());
        }
        if let Ok(Some(status)) = child.try_wait() {
            processes
                .0
                .lock()
                .expect("processes lock")
                .remove(&instance_id);
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
            let _ = kill_tree(pid).await;
            processes
                .0
                .lock()
                .expect("processes lock")
                .remove(&instance_id);
            let tail = log_tail(&log_path, 30).await;
            return Err(format!(
                "等待 120 秒仍未就绪，已终止进程。\n--- 日志末尾 ---\n{tail}"
            ));
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
        processes
            .0
            .lock()
            .expect("processes lock")
            .remove(&instance_id);
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

    // Single source of truth for "no longer running": the watcher removes the
    // map entry (by pid, so a relaunched instance's entry survives) and
    // notifies the frontend for both crashes and clean shutdowns.
    let watcher_app = app.clone();
    let watcher_id = instance_id.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        if let Some(processes) = watcher_app.try_state::<Processes>() {
            let mut map = processes.0.lock().expect("processes lock");
            if map.get(&watcher_id).map(|e| e.pid) == Some(pid) {
                map.remove(&watcher_id);
            }
        }
        let code = status.ok().and_then(|s| s.code());
        let _ = watcher_app.emit(
            INSTANCE_EXITED,
            serde_json::json!({ "instanceId": watcher_id, "pid": pid, "code": code }),
        );
    });

    // `dsh web` prints the authenticated URL (`dsh web: http://127.0.0.1:PORT/?token=…`)
    // before binding; the bare host:port answers "authentication required".
    // The line can lag the listening socket by a buffer flush or two, so a
    // short poll beats shipping the tokenless fallback on first read.
    let web_url = read_web_url(&log_path).await;

    Ok(LaunchOutcome { pid, port, web_url })
}

/// Scan a launch log for the `dsh web:` line and lift the URL out of it.
async fn read_web_url(log_path: &Path) -> Option<String> {
    for _ in 0..8 {
        if let Ok(raw) = tokio::fs::read_to_string(log_path).await {
            if let Some(url) = parse_web_url(&raw) {
                return Some(url);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

fn parse_web_url(log_text: &str) -> Option<String> {
    for line in log_text.lines() {
        const MARKER: &str = "dsh web:";
        let Some(idx) = line.find(MARKER) else {
            continue;
        };
        let rest = line[idx + MARKER.len()..].trim_start();
        let Some(start) = rest.find("http") else {
            continue;
        };
        let url: String = rest[start..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

/// (program, argv, base env pairs) — what one launch executes.
type CommandPlan = (PathBuf, Vec<String>, Vec<(String, String)>);

/// Resolves the program + argv + base environment for one launch. User args
/// come *before* our `--port` so a duplicate can never win, and `--no-open`
/// is documented by DSH for exactly this launcher case.
fn build_command(
    root: &Path,
    version_name: &str,
    runtime_name: &str,
    port: u16,
    args: &[String],
    dsh_home: &Path,
) -> Result<CommandPlan, String> {
    let node = resolve_node(root, runtime_name)?;
    let bin = root
        .join("versions")
        .join(version_name)
        .join("lib")
        .join("bin.js");
    if !bin.exists() {
        return Err(format!("DSH 版本 {version_name} 未安装（缺少 lib/bin.js）"));
    }

    let mut cmd_args = vec![bin.to_string_lossy().into_owned(), "web".into()];
    cmd_args.extend(args.iter().cloned());
    cmd_args.push("--port".into());
    cmd_args.push(port.to_string());
    cmd_args.push("--no-open".into());

    let mut env_pairs = vec![("DSH_HOME".into(), dsh_home.to_string_lossy().into_owned())];
    if runtime_name != "node-system" {
        // Put the runtime's bin dir first so DSH's own child processes
        // resolve this exact node, not whatever is on the system PATH.
        let sep = if cfg!(windows) { ";" } else { ":" };
        let bin_dir = runtime_bin_dir(root, runtime_name);
        let existing = std::env::var("PATH").unwrap_or_default();
        env_pairs.push((
            "PATH".into(),
            format!("{}{sep}{existing}", bin_dir.display()),
        ));
    }
    Ok((node, cmd_args, env_pairs))
}

/// The system runtime runs whatever `node` is on PATH; installed runtimes
/// must have their binary in place or the launch is refused up front.
pub(crate) fn resolve_node(root: &Path, runtime_name: &str) -> Result<PathBuf, String> {
    if runtime_name == "node-system" {
        return Ok(PathBuf::from("node"));
    }
    let node = runtime_bin_dir(root, runtime_name).join(node_binary());
    if !node.exists() {
        return Err(format!(
            "Runtime {runtime_name} 未安装（缺少 {}）",
            node.display()
        ));
    }
    Ok(node)
}

pub(crate) fn runtime_bin_dir(root: &Path, runtime_name: &str) -> PathBuf {
    let dir = root.join("runtimes").join(runtime_name);
    if cfg!(windows) {
        dir // the zip layout has node.exe at the top
    } else {
        dir.join("bin")
    }
}

pub(crate) fn node_binary() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Picks the port to listen on. PHL's own running instances are named in the
/// error; anything else on the machine is caught by the bind test. Pure and
/// synchronous — the async caller turns `Err` into a launch failure.
fn allocate_port(
    processes: &Processes,
    instance_id: &str,
    wanted: u16,
    auto: bool,
) -> Result<u16, String> {
    let map = processes.0.lock().expect("processes lock");
    if !auto {
        if let Some((other, _)) = map
            .iter()
            .find(|(id, e)| e.port == wanted && id.as_str() != instance_id)
        {
            return Err(format!("端口 {wanted} 已被实例 {other} 占用"));
        }
        // The bind test is not optional here either. Checking only PHL's own
        // map meant a port held by an unrelated program passed: DSH then failed
        // to bind, but the readiness probe connected to *that* program and the
        // launch was reported as ready, with 打开 WebUI pointing at a stranger.
        if !port_free(wanted) {
            return Err(format!("端口 {wanted} 已被本机其他程序占用"));
        }
        return Ok(wanted);
    }
    let mut port = wanted;
    for _ in 0..100 {
        let phl_taken = map
            .iter()
            .any(|(id, e)| e.port == port && id != instance_id);
        if !phl_taken && port_free(port) {
            return Ok(port);
        }
        // `wrapping_add` walked 65535 → 0, and binding port 0 always succeeds
        // (it means "give me any ephemeral port"), so `port_free(0)` was true
        // and DSH got `--port 0` — after which the readiness probe could never
        // connect and the launch hung for the whole timeout.
        match port.checked_add(1) {
            Some(next) => port = next,
            None => break,
        }
    }
    Err(format!("从 {wanted} 起找不到可用端口"))
}

fn port_free(port: u16) -> bool {
    // Port 0 is never a real target: `bind` treats it as "assign an ephemeral
    // port" and would always report it free.
    port != 0 && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

async fn count_plugins(profile_dir: &Path) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(profile_dir.join("node_modules")).await else {
        return 0;
    };
    let mut count = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        count += 1;
    }
    count
}

async fn log_tail(path: &Path, max_lines: usize) -> String {
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return String::new();
    };
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

/// The spawned node is only the tree root — DSH shells out to plugins and
/// tools, so the whole tree has to go with it.
#[cfg(windows)]
pub(crate) async fn kill_tree(pid: u32) -> Result<(), String> {
    let output = tokio::task::spawn_blocking(move || {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    check_kill_output(pid, output)
}

#[cfg(not(windows))]
pub(crate) async fn kill_tree(pid: u32) -> Result<(), String> {
    // No libc dependency: `kill` on the direct child. DSH's own children are
    // expected to exit with it; a tree-kill here would need a setsid pre-exec.
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    check_kill_output(pid, output)
}

fn check_kill_output(pid: u32, output: std::process::Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "无法终止进程 {pid}（退出码 {:?}）: {}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
