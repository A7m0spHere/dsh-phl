//! Process-level utilities for launching DSH: node/runtime resolution, the
//! child command builder, port allocation, log reading and process-tree
//! termination.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Processes;

pub(crate) async fn read_web_url(log_path: &Path) -> Option<String> {
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
pub(crate) fn allocate_port(
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

pub(crate) fn port_free(port: u16) -> bool {
    // Port 0 is never a real target: `bind` treats it as "assign an ephemeral
    // port" and would always report it free.
    port != 0 && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub(crate) async fn count_plugins(profile_dir: &Path) -> usize {
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

pub(crate) async fn log_tail(path: &Path, max_lines: usize) -> String {
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
