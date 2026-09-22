//! Process-level utilities for launching DSH: node/runtime resolution, the
//! child command builder, port allocation, log reading and process-tree
//! termination.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::probe::ProcessState;
use super::Processes;

/// One bounded head read: `dsh web:` prints the token line in the first
/// kilobytes, and a wedged plugin could otherwise spam the log until reading
/// it back costs the same memory as the log itself.
pub(crate) const LOG_HEAD_WINDOW: u64 = 256 * 1024;

/// The cap for a bounded tail read (O-12): startup diagnostics ask for the
/// last 30 lines *after a failure* — the log can have grown to megabytes of
/// plugin noise by then, and slurping it to slice off 30 lines is the whole
/// problem, not the solution.
pub(crate) const LOG_TAIL_WINDOW: u64 = 256 * 1024;

/// Read up to `window` bytes from the end of `path` without loading more.
/// The flag says whether the beginning of the file was cut away.
pub(crate) async fn read_tail(path: &Path, window: u64) -> (Vec<u8>, bool) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return (Vec::new(), false);
    };
    let Ok(len) = file.metadata().await.map(|m| m.len()) else {
        return (Vec::new(), false);
    };
    let start = len.saturating_sub(window);
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return (Vec::new(), false);
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.take(window).read_to_end(&mut buf).await.is_err() {
        return (Vec::new(), false);
    }
    (buf, start > 0)
}

/// One read of a log's head: `None` until `dsh web` has printed its line.
/// Used both by the launch (which waits for this boot's line) and by restart
/// adoption, where the line is long past any buffering.
pub(crate) async fn read_web_url_once(log_path: &Path) -> Option<String> {
    let raw = read_head(log_path).await?;
    parse_web_url(&raw)
}

/// The token line sits at the top of the log; a bounded head read gets it
/// without materialising whatever the instance has printed since.
async fn read_head(log_path: &Path) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(log_path).await.ok()?;
    let mut buf = Vec::new();
    file.take(LOG_HEAD_WINDOW)
        .read_to_end(&mut buf)
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Open this launch's log file, truncated.
///
/// The name carries second precision, so a relaunch inside the same second
/// lands on the same path. Appending to it left the PREVIOUS boot's `dsh web:`
/// line at the head of the file — exactly the line the URL reader lifts out —
/// so the window opened with a token the new process never minted and DSH
/// answered 401. Truncating makes the head of the file this boot's own line by
/// construction. `write` + `truncate` rather than `append`: the child inherits
/// the handle and writes at its offset, so appending buys nothing here.
pub(crate) fn open_launch_log(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

/* --------------------------- launch readiness --------------------------- */

/// How long a launch keeps waiting for this boot's `dsh web:` line after the
/// port starts answering.
///
/// DSH prints the authenticated URL only *after* its listener is bound —
/// measured at 1.3 s on an idle machine and 2.1 s on a loaded one
/// (2026-09-15) — so "the socket accepts" does not mean "the URL is in the
/// log". The old fixed 2 s read lost that race by 70 ms on a real launch: the
/// frontend received a URL-less outcome, fell back to the bare `host:port`,
/// and DSH answered with its 401 "dsh web authentication required" page in a
/// window PHL reported as running. The grace is generous on purpose, because
/// the gap is the tail of DSH's profile boot and stretches on a cold first
/// run; it costs nothing when the line arrives on time, and only a DSH that
/// prints no line at all ever pays it.
pub(crate) const WEB_URL_GRACE: Duration = Duration::from_secs(15);

/// What the readiness wait observed — the loop's old exit reasons, kept one for
/// one so the caller's error wording is unchanged.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    /// The port answers, and this boot's authenticated URL is in the log when
    /// DSH printed one.
    Ready {
        web_url: Option<String>,
    },
    Cancelled,
    Exited {
        code: Option<i32>,
    },
    Timeout,
}

/// Wait for one launch to become ready: the port accepting connections *and*
/// this boot's own `dsh web:` line, when DSH prints one.
///
/// Both halves matter. A socket alone can belong to a stranger — a port
/// collision hands the probe another instance's DSH, which is why the
/// check-then-use allocation came up in review — while the token line is the
/// one thing only *our* child can have written into this launch's freshly
/// truncated file. And the URL has to be in hand before the caller reports
/// success: the frontend opens the embedded window straight from the outcome,
/// and the bare address it falls back to is a 401 page.
pub(crate) async fn await_readiness(
    child: &mut tokio::process::Child,
    port: u16,
    log_path: &Path,
    timeout: Duration,
    url_grace: Duration,
    cancelled: &AtomicBool,
    on_wait: &mut impl FnMut(Duration),
) -> Readiness {
    let started = Instant::now();
    // When the socket first answered — the clock the URL grace runs on.
    let mut port_ready_at: Option<Instant> = None;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Readiness::Cancelled;
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Readiness::Exited {
                code: status.code(),
            };
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            let since = *port_ready_at.get_or_insert_with(Instant::now);
            if let Some(web_url) = read_web_url_once(log_path).await {
                return Readiness::Ready {
                    web_url: Some(web_url),
                };
            }
            if since.elapsed() >= url_grace {
                // A DSH that serves without printing a URL is not broken, it
                // is just not this DSH: succeed, with nothing to hand over.
                return Readiness::Ready { web_url: None };
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Readiness::Timeout;
        }
        on_wait(elapsed);
        // Once the socket is up the clock that matters is the URL grace, which
        // is finer than the boot wait — a shorter tick keeps the wait off the
        // launch timeline.
        let tick = if port_ready_at.is_some() { 200 } else { 500 };
        tokio::time::sleep(Duration::from_millis(tick)).await;
    }
}

/// Replace the per-boot WebUI token in a log excerpt.
///
/// `dsh web` prints the authenticated URL as the first line of its log, and
/// PHL puts log tails into toasts and diagnostics reports that users paste into
/// issues. The token is per-boot and the log is local, but while the instance
/// runs it is a live credential — it must not cross the IPC boundary.
pub(crate) fn redact_web_token(text: &str) -> String {
    const KEY: &str = "token=";
    const PLACEHOLDER: &str = "token=<redacted>";
    if !text.contains(KEY) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(KEY) {
        out.push_str(&rest[..idx]);
        out.push_str(PLACEHOLDER);
        let tail = &rest[idx + KEY.len()..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ')' | ','))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

pub(crate) fn parse_web_url(log_text: &str) -> Option<String> {
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
pub(crate) fn build_command(
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

/// The executable a launch will run: an installed runtime's own `node`, or the
/// system Node resolved to a *verified absolute path*.
///
/// The system case used to hand back a bare `node` and let the OS resolve it
/// at spawn time. That is the one resolution a GUI launch cannot perform: an
/// app started from Finder inherits launchd's PATH, which knows nothing of
/// fnm, nvm or Homebrew, so the instance failed to start while the identical
/// command worked in a terminal. Resolving here makes the version the runtime
/// page reports, the binary the log line shows and the process actually
/// spawned one value (`discovery::inspect::resolve_system_node`).
pub(crate) fn resolve_node(root: &Path, runtime_name: &str) -> Result<PathBuf, String> {
    if runtime_name == "node-system" {
        return crate::discovery::inspect::resolve_system_node()
            .map(|(node, _)| node)
            .ok_or_else(|| {
                "系统 Node 不可用：PATH 与常见安装位置都没有找到可执行的 node。\
                 请在「运行时」页安装一个 Node，或把 Node 加入 PATH。"
                    .to_string()
            });
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

/// The smallest port PHL will hand to DSH. Chromium refuses to *navigate* to
/// the reserved/system service ports (its `ERR_UNSAFE_PORT` blocklist covers
/// most of 1–1023, port 1 included), even though binding them succeeds on
/// Windows — so an instance allocated one is a dead WebUI window that PHL
/// itself reports as running. The adoption flow used to persist `port: 0`,
/// the auto-scan advanced to 1, and that is exactly what happened (2026-09-11).
pub(crate) const MIN_WEB_PORT: u16 = 1024;

/// Picks the port to listen on. PHL's own running instances are named in the
/// error; anything else on the machine is caught by the bind test. Pure and
/// synchronous — the async caller turns `Err` into a launch failure.
pub(crate) fn allocate_port(
    processes: &Processes,
    instance_id: &str,
    wanted: u16,
    auto: bool,
) -> Result<u16, String> {
    let map = processes.0.lock().expect("processes lock");
    if !auto {
        if wanted < MIN_WEB_PORT {
            // The same blocklist as the auto branch, but without a scan to
            // rescue the choice: say why the configured port can never open.
            return Err(crate::errors::coded(
                crate::errors::ErrCode::PortConflict,
                format!(
                    "端口 {wanted} 是系统保留端口，浏览器会拒绝打开它的页面；\
                     请改用 {MIN_WEB_PORT} 以上的端口，或开启自动分配"
                ),
            ));
        }
        if let Some((other, _)) = map
            .iter()
            .find(|(id, e)| e.port == wanted && id.as_str() != instance_id)
        {
            return Err(crate::errors::coded(
                crate::errors::ErrCode::PortConflict,
                format!("端口 {wanted} 已被实例 {other} 占用"),
            ));
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
    // A stored port below the floor (the old adoption's `port: 0`, or a port
    // allocated before this guard existed) must not seed the scan: advancing
    // 0 → 1 used to "succeed" on Windows and strand the WebUI behind
    // `ERR_UNSAFE_PORT`. The scan always resumes inside the openable range.
    let mut port = wanted.max(MIN_WEB_PORT);
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

pub(crate) fn port_free(port: u16) -> bool {
    // Port 0 is never a real target: `bind` treats it as "assign an ephemeral
    // port" and would always report it free.
    port != 0 && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Per-instance retention for `launch-*.log` (O-12). A long-running instance
/// writes one file per launch, so without a cap an old instance's `logs/`
/// grows unbounded. Keep the newest `keep` files by name (the ISO timestamp
/// in the name sorts chronologically) and delete the rest. Best-effort: a
/// retention sweep must never fail a launch.
pub(crate) async fn prune_launch_logs(logs_dir: &Path, keep: usize) {
    let Ok(mut dir) = tokio::fs::read_dir(logs_dir).await else {
        return;
    };
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("launch-") && name.ends_with(".log") {
            names.push((name, entry.path()));
        }
    }
    if names.len() <= keep {
        return;
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path) in names.iter().take(names.len() - keep) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

pub(crate) async fn log_tail(path: &Path, max_lines: usize) -> String {
    // Bounded (O-12): read the last LOG_TAIL_WINDOW bytes, never the whole
    // file. When the window cut the first line mid-way, that fragment is not
    // a line and is dropped.
    let (bytes, cut) = read_tail(path, LOG_TAIL_WINDOW).await;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if cut && !lines.is_empty() {
        lines.remove(0);
    }
    let start = lines.len().saturating_sub(max_lines);
    // Redacted here rather than at each call site: this string ends up in
    // toasts and diagnostics reports, and a short log has the token line in it.
    redact_web_token(&lines[start..].join(
        "
",
    ))
}

/// A hung kill must not hang the stop command forever. The blocking worker
/// cannot be cancelled, but the caller gets an answer and the process entry
/// stays tracked, so the user can retry instead of watching a spinner.
const KILL_TIMEOUT: Duration = Duration::from_secs(10);

/// The spawned node is only the tree root — DSH shells out to plugins and
/// tools, so the whole tree has to go with it. Windows' `taskkill /T /F` walks
/// that tree itself; Unix reaches it through the process group, which is
/// `terminate_tree`'s job, so this is the direct-child fallback used when no
/// group can be vouched for (and the only path Windows needs).
#[cfg(windows)]
pub(crate) async fn kill_tree(pid: u32) -> Result<(), String> {
    let output = tokio::time::timeout(
        KILL_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        }),
    )
    .await
    .map_err(|_| {
        format!("终止进程 {pid} 超时（taskkill 超过 10 秒无响应），可在任务管理器中手动结束")
    })?
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    check_kill_output(pid, output)
}

#[cfg(not(windows))]
pub(crate) async fn kill_tree(pid: u32) -> Result<(), String> {
    // Direct child only, hard. Reached when `terminate_tree` cannot vouch for
    // a process group: DSH's own children are then expected to exit with it,
    // because there is no group handle to take them out deliberately.
    let output = tokio::time::timeout(
        KILL_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .output()
        }),
    )
    .await
    .map_err(|_| format!("终止进程 {pid} 超时（kill 超过 10 秒无响应）"))?
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    check_kill_output(pid, output)
}

pub(crate) fn check_kill_output(pid: u32, output: std::process::Output) -> Result<(), String> {
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

/// How long a politely-terminated process tree gets before it is killed
/// outright: enough for DSH to close the session files it opened (the reason a
/// stop is not one hard kill) and for its children to follow it out, short
/// enough that 「停止」 still feels immediate.
pub(crate) const TERM_GRACE: Duration = Duration::from_secs(3);

/// Poll `probe` until it reports the process gone, or `window` runs out.
///
/// The one implementation of "an exit we have observed" — the stop path's
/// confirm window and the grace inside `terminate_tree` both use it, because
/// a request that a signal was *delivered* is not an observed exit.
pub(crate) async fn confirm_exit(
    mut probe: impl FnMut() -> ProcessState,
    window: Duration,
) -> bool {
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

/// Terminate a launched DSH process *and the tree it started*.
///
/// The spawned node is only the tree root: DSH shells out to plugins and tools,
/// so a stop has to take the group with it. SIGTERM first so DSH closes what it
/// opened and its children follow, then SIGKILL for whatever refused — and the
/// hard signal is sent to the group whether or not the root has already exited,
/// because a child that ignores SIGTERM outlives its parent. The single
/// `kill -9 <pid>` this replaces neither gave DSH that chance nor reached
/// anything it started (2026-09-22 review).
///
/// The group is only signalled when the kernel shows this pid *leading* one
/// (`probe::owned_group_of`); everything else takes the direct-child path.
/// Unknown pid-sharing groups are never signalled: `kill -TERM -<pgid>` reaches
/// every member, and one of those members could be PHL itself.
pub(crate) async fn terminate_tree(pid: u32, grace: Duration) -> Result<(), String> {
    // Windows' `taskkill /T /F` already walks the tree; there is no group id.
    #[cfg(windows)]
    {
        let _ = grace;
        kill_tree(pid).await
    }
    #[cfg(not(windows))]
    {
        let Some(group) = super::probe::owned_group_of(pid) else {
            return kill_tree(pid).await;
        };
        // A TERM that fails is still no reason to skip the KILL below: the
        // pid may have exited between the probe and the signal.
        let _ = signal_group(group, "TERM").await;
        let root_gone = confirm_exit(|| super::probe::probe_process(pid).state, grace).await;
        let hard = signal_group(group, "KILL").await;
        if root_gone {
            // A group with nothing left in it cannot be signalled, and that is
            // the outcome we were after — worth a line, not an error.
            if let Err(e) = hard {
                eprintln!("[phl] {e}");
            }
            return Ok(());
        }
        hard
    }
}

/// One signal to a process group. `--` ends the option list: the group form is
/// a negative id, which `kill` would otherwise read as an option.
#[cfg(not(windows))]
async fn signal_group(group: u32, signal: &'static str) -> Result<(), String> {
    let output = tokio::time::timeout(
        KILL_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("kill")
                .args(["-s", signal, "--", &format!("-{group}")])
                .output()
        }),
    )
    .await
    .map_err(|_| format!("向进程组 {group} 发送 {signal} 超时（kill 超过 10 秒无响应）"))?
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "无法向进程组 {group} 发送 {signal}（退出码 {:?}）: {}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that stays alive for the whole test; the caller kills it.
    fn long_lived_child() -> tokio::process::Child {
        if cfg!(windows) {
            tokio::process::Command::new("cmd")
                .args(["/C", "ping -n 60 127.0.0.1 > nul"])
                .spawn()
                .unwrap()
        } else {
            tokio::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap()
        }
    }

    fn temp_log(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("phl-ready-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launch.log");
        (dir, path)
    }

    /// A shell that starts a child of its own — the shape of DSH shelling out
    /// to a plugin — and records that child's pid where the test can read it.
    #[cfg(unix)]
    fn tree_fixture(tag: &str) -> (PathBuf, tokio::process::Child, u32) {
        let dir = std::env::temp_dir().join(format!("phl-tree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("child.pid");
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "sleep 60 & echo $! > {} ; wait",
                pid_file.display()
            ))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Exactly what launch does, so the fixture has the group the stop path
        // is allowed to signal.
        command.process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        (pid_file, child, pid)
    }

    /// The review's finding: `kill -9 <pid>` left whatever DSH had started
    /// running. A stop must reach the tree.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_terminated_tree_takes_the_children_with_it() {
        let (pid_file, mut child, pid) = tree_fixture("tree");
        let grandchild = wait_for_pid_file(&pid_file).await;
        assert_eq!(
            super::super::probe::probe_process(grandchild).state,
            ProcessState::Alive,
            "the fixture must actually have a child running"
        );
        assert_eq!(super::super::probe::owned_group_of(pid), Some(pid));

        terminate_tree(pid, TERM_GRACE).await.unwrap();

        assert_eq!(
            super::super::probe::probe_process(pid).state,
            ProcessState::Exited
        );
        assert_eq!(
            super::super::probe::probe_process(grandchild).state,
            ProcessState::Exited,
            "a child of the instance outlived the stop"
        );
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(pid_file.parent().unwrap());
    }

    /// A process that ignores SIGTERM is killed anyway — but only after the
    /// grace, which is the whole point of asking nicely first.
    ///
    /// The fixture announces itself before the clock starts: signalling a shell
    /// that has not reached `trap` yet would kill it outright, and the test
    /// would then be asserting that a process which *did* die honours the grace
    /// (measured: ~1 run in 5 lost that race under a full parallel suite).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_sigterm_refusing_group_is_killed_after_the_grace() {
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("trap '' TERM; echo ready; while :; do sleep 1; done")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        command.process_group(0);
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        let grace = Duration::from_millis(400);
        wait_for_ready(&mut child).await;

        let started = Instant::now();
        terminate_tree(pid, grace).await.unwrap();

        assert!(
            started.elapsed() >= grace,
            "the hard kill must wait out the grace, took {:?}",
            started.elapsed()
        );
        assert_eq!(
            super::super::probe::probe_process(pid).state,
            ProcessState::Exited
        );
        let _ = child.wait().await;
    }

    /// Block until the fixture prints its readiness line.
    #[cfg(unix)]
    async fn wait_for_ready(child: &mut tokio::process::Child) {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdout = child.stdout.take().expect("fixture stdout is piped");
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await;
        assert!(
            matches!(ready, Ok(Ok(Some(ref line))) if line.trim() == "ready"),
            "fixture never announced readiness: {ready:?}"
        );
    }

    /// A pid that shares PHL's own group is never signalled as a group: the
    /// negative-pid form would reach this test runner too. The parse half is
    /// pinned on every host, Windows included.
    #[test]
    fn only_a_group_the_pid_leads_may_be_signalled() {
        assert_eq!(
            super::super::probe::parse_ps_pgid("4242\n", 4242),
            Some(4242)
        );
        assert_eq!(
            super::super::probe::parse_ps_pgid("  4242  ", 4242),
            Some(4242)
        );
        assert_eq!(super::super::probe::parse_ps_pgid("1\n", 4242), None);
        assert_eq!(super::super::probe::parse_ps_pgid("", 4242), None);
        assert_eq!(super::super::probe::parse_ps_pgid("pgid\n", 4242), None);
    }

    #[cfg(unix)]
    async fn wait_for_pid_file(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "fixture never wrote {path:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Reserve a port and keep the listener: a connect to it succeeds, exactly
    /// like DSH's listener the moment it binds.
    fn answering_port() -> (TcpListener, u16) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn a_port_that_answers_before_the_url_is_printed_still_yields_the_url() {
        // The regression (2026-09-15): DSH binds its listener first and prints
        // the authenticated URL 1.3–2.1 s later. Readiness that stops at the
        // socket, or a fixed short poll, loses the URL — and the window then
        // opens the bare host:port, which DSH answers with its 401 page.
        let (_listener, port) = answering_port();
        let (dir, log) = temp_log("url-late");
        std::fs::write(&log, "").unwrap();
        let late_log = log.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            std::fs::write(
                &late_log,
                "dsh web: http://127.0.0.1:4099/?token=later-on\n",
            )
            .unwrap();
        });

        let mut child = long_lived_child();
        let readiness = await_readiness(
            &mut child,
            port,
            &log,
            Duration::from_secs(30),
            WEB_URL_GRACE,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .await;
        let _ = child.kill().await;
        assert_eq!(
            readiness,
            Readiness::Ready {
                web_url: Some("http://127.0.0.1:4099/?token=later-on".into())
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_dsh_that_never_prints_a_url_is_ready_without_one() {
        // Not every server mints a token; the grace expiring is a success with
        // nothing to hand over, never a failed launch.
        let (_listener, port) = answering_port();
        let (dir, log) = temp_log("url-never");
        std::fs::write(&log, "booting: no url line here\n").unwrap();

        let mut child = long_lived_child();
        let readiness = await_readiness(
            &mut child,
            port,
            &log,
            Duration::from_secs(30),
            Duration::from_millis(200),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .await;
        let _ = child.kill().await;
        assert_eq!(readiness, Readiness::Ready { web_url: None });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn waiting_for_the_url_keeps_watching_cancel_and_exit() {
        // The grace window must not become a blind spot: a cancel and a child
        // that dies are still the two things that end the wait immediately.
        let (_listener, port) = answering_port();
        let (dir, log) = temp_log("url-watch");
        std::fs::write(&log, "").unwrap();

        let mut child = long_lived_child();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let flipper = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            flipper.store(true, Ordering::SeqCst);
        });
        let mut noop = |_| {};
        let readiness = await_readiness(
            &mut child,
            port,
            &log,
            Duration::from_secs(30),
            WEB_URL_GRACE,
            &cancelled,
            &mut noop,
        )
        .await;
        let _ = child.kill().await;
        assert_eq!(readiness, Readiness::Cancelled);

        let mut exited = if cfg!(windows) {
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
        let readiness = await_readiness(
            &mut exited,
            port,
            &log,
            Duration::from_secs(30),
            WEB_URL_GRACE,
            &AtomicBool::new(false),
            &mut noop,
        )
        .await;
        assert_eq!(readiness, Readiness::Exited { code: Some(7) });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_port_that_never_answers_times_out() {
        // The whole-launch budget still applies: a socket that never opens is
        // a wedged boot, reported with the same exit reason as before.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (dir, log) = temp_log("url-timeout");
        std::fs::write(&log, "").unwrap();

        let mut child = long_lived_child();
        let readiness = await_readiness(
            &mut child,
            port,
            &log,
            Duration::from_millis(400),
            WEB_URL_GRACE,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .await;
        let _ = child.kill().await;
        assert_eq!(readiness, Readiness::Timeout);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_launch_log_is_truncated_on_open() {
        // Same-second relaunches share the path; the head must be this boot's.
        let (dir, log) = temp_log("truncate");
        std::fs::write(&log, "dsh web: http://127.0.0.1:3081/?token=stale\n").unwrap();
        drop(open_launch_log(&log).unwrap());
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
