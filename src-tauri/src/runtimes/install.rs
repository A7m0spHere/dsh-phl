//! The runtime installer: dist archive download into a transaction-scoped
//! staging tree, SHASUMS256 verification, health gate (runnable node
//! reporting the requested version) and the backup-swap commit.

use std::path::Path;

use serde::Deserialize;
use tauri::ipc::Channel;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{
    cancelled, download, extract, http_client, now_iso, now_millis, safe_join, sanitize_version,
    strip_first, Downloaded, ProgressEvent, HTTP_TIMEOUT,
};
use crate::launch::node_binary;

/* ------------------------------- install ------------------------------- */

#[derive(Deserialize)]
// Same on-disk shape as the install marker: camelCase keys.
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeMarker {
    pub(crate) installed_at: String,
    pub(crate) version: String,
}

// The install pipeline threads flag + task + channel through every stage;
// bundling them would obscure which installer owns which cancel/registry.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_runtime_install(
    flag: &Arc<AtomicBool>,
    dist_base: &str,
    version_name: &str,
    version: &str,
    root: &Path,
    keep_archive: bool,
    task: &crate::resources::Task,
    on_progress: &Channel<ProgressEvent>,
) -> Result<(), String> {
    // `version_name` is `node-<major>` (the install directory, and what the
    // frontend passes back for removal); `version` is the full semver.
    let version_name = sanitize_version(version_name)?;
    let version = sanitize_version(version)?;
    if cancelled(flag) {
        return Err("cancelled".into());
    }

    let platform = dist_platform()?;
    let filename = format!("node-v{version}-{platform}");
    let base = dist_base.trim_end_matches('/');
    let url = format!("{base}/v{version}/{filename}");

    let runtimes_dir = root.join("runtimes");
    let cache_dir = root.join("cache");
    let dest = runtimes_dir.join(&version_name);
    // Transaction-scoped staging/backup names, same vocabulary as the
    // version installer: a failed attempt can only leave a `.phl-txn`
    // child behind, never something that reads as an installed runtime.
    let token = now_millis();
    let staging = crate::versions::txn_dir(&runtimes_dir, &version_name, "staging", token);
    let backup = crate::versions::txn_dir(&runtimes_dir, &version_name, "backup", token);
    let part_path = cache_dir.join(format!("{filename}.part"));
    tokio::fs::create_dir_all(&runtimes_dir)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| e.to_string())?;

    // Everything below lands in the staging dir; the installed tree for this
    // major is only touched once a complete replacement exists. Deleting the
    // old tree up front would leave instances pointing at a runtime that a
    // failed download just destroyed.
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let downloaded: Downloaded = download(
        flag,
        &url,
        &part_path,
        None,
        &|progress, bytes_done, bytes_per_sec| {
            let ev = ProgressEvent::Downloading {
                progress,
                bytes_done,
                bytes_per_sec,
            };
            // One event, two consumers: the page's bar and the task center.
            crate::versions::sync_task_progress(task, &ev);
            let _ = on_progress.send(ev);
        },
    )
    .await?;

    let ev = ProgressEvent::Verifying;
    crate::versions::sync_task_progress(task, &ev);
    on_progress.send(ev).map_err(|e| e.to_string())?;
    let client = http_client();
    let shasums = fetch_shasums(&client, base, &version).await?;
    check_shasums(&shasums, &filename, &downloaded.sha256)?;

    let ev = ProgressEvent::Extracting { progress: 0.0 };
    crate::versions::sync_task_progress(task, &ev);
    on_progress.send(ev).map_err(|e| e.to_string())?;
    if platform.ends_with(".zip") {
        extract_zip(
            &part_path,
            &staging,
            &|progress| {
                let ev = ProgressEvent::Extracting { progress };
                crate::versions::sync_task_progress(task, &ev);
                let _ = on_progress.send(ev);
            },
            flag,
        )
        .await?;
    } else {
        // The .tar.gz nests everything under node-v…-<platform>/;
        // versions::extract strips the first component, same as npm's package/.
        extract(
            &part_path,
            &staging,
            &|progress| {
                let ev = ProgressEvent::Extracting { progress };
                crate::versions::sync_task_progress(task, &ev);
                let _ = on_progress.send(ev);
            },
            flag,
        )
        .await?;
    }

    let marker = serde_json::json!({
        "installedAt": now_iso(),
        "version": version,
        "bytes": downloaded.bytes,
        "sha256": hex::encode(&downloaded.sha256),
    });
    tokio::fs::write(staging.join("phl-runtime.json"), marker.to_string())
        .await
        .map_err(|e| e.to_string())?;

    // Health gate before the swap: the extracted tree must carry a runnable
    // node that reports exactly the requested version. A runtime that cannot
    // start its own binary must never replace a working one.
    task.set_phase("checking");
    task.set_progress(None);
    if let Err(e) = check_runtime_health(&staging, &version).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("Runtime 校验未通过，安装中止: {e}"));
    }

    if keep_archive {
        tokio::fs::rename(&part_path, cache_dir.join(&filename))
            .await
            .map_err(|e| e.to_string())?;
    } else {
        let _ = tokio::fs::remove_file(&part_path).await;
    }

    // Swap in through the shared promote path: backup, rename, final verify
    // (marker version must match), rollback of the old tree on any failure.
    task.set_phase("committing");
    crate::versions::promote_staged(&staging, &dest, &backup, &|installed: &Path| {
        let marker = std::fs::read_to_string(installed.join("phl-runtime.json"))
            .map_err(|e| format!("Runtime 标记读取失败: {e}"))?;
        let parsed: RuntimeMarker =
            serde_json::from_str(&marker).map_err(|e| format!("Runtime 标记解析失败: {e}"))?;
        if parsed.version != version {
            return Err(format!(
                "Runtime 标记版本 {} 与目标 {version} 不一致",
                parsed.version
            ));
        }
        // The zip layout puts node.exe at the top level, the tar layout
        // in bin/ — same distinction `runtime_bin_dir` encodes.
        let bin = if cfg!(windows) {
            installed.to_path_buf()
        } else {
            installed.join("bin")
        };
        if !bin.join(node_binary()).exists() {
            return Err("node 可执行文件缺失".into());
        }
        Ok(())
    })
    .await?;
    Ok(())
}

/// A staged runtime must contain its node binary, and that binary must run
/// and report the requested version. Spawned on a blocking thread — this is
/// a process launch, not a syscall.
pub(crate) async fn check_runtime_health(staging: &Path, version: &str) -> Result<(), String> {
    let node = staging.join(node_binary());
    if !node.exists() {
        return Err("node 可执行文件缺失".into());
    }
    let reported = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(&node);
        command.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::launch::CREATE_NO_WINDOW);
        }
        command
            .output()
            .map_err(|e| format!("无法运行 node --version: {e}"))
    })
    .await
    .map_err(|e| format!("校验线程异常退出: {e}"))??;
    if !reported.status.success() {
        return Err(format!(
            "node --version 退出码 {}",
            reported.status.code().unwrap_or(-1)
        ));
    }
    let stdout = String::from_utf8_lossy(&reported.stdout).trim().to_string();
    if stdout != format!("v{version}") {
        return Err(format!("node --version 返回 {stdout}，期望 v{version}"));
    }
    Ok(())
}

/// The dist archive layout for this machine. Node publishes `.zip` only for
/// Windows; every other platform is a `.tar.gz` the existing pipeline handles.
pub(crate) fn dist_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("win-x64.zip"),
        ("windows", "aarch64") => Ok("win-arm64.zip"),
        ("macos", "x86_64") => Ok("darwin-x64.tar.gz"),
        ("macos", "aarch64") => Ok("darwin-arm64.tar.gz"),
        ("linux", "x86_64") => Ok("linux-x64.tar.gz"),
        ("linux", "aarch64") => Ok("linux-arm64.tar.gz"),
        (os, arch) => Err(format!("暂不支持的平台: {os}-{arch}")),
    }
}

/* ------------------------------- verify ------------------------------- */

pub(crate) async fn fetch_shasums(
    client: &reqwest::Client,
    base: &str,
    version: &str,
) -> Result<HashMap<String, String>, String> {
    let url = format!("{base}/v{version}/SHASUMS256.txt");
    let text = client
        .get(&url)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("SHASUMS256 请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("SHASUMS256 返回错误: {e}"))?
        .text()
        .await
        .map_err(|e| format!("SHASUMS256 读取失败: {e}"))?;
    let map = parse_shasums(&text);
    if map.is_empty() {
        return Err("SHASUMS256.txt 为空或格式无法识别".into());
    }
    Ok(map)
}

/// Lines look like `<sha256>␣␣<filename>` or `<sha256>␣*<filename>` (binary
/// marker); anything else is skipped.
pub(crate) fn parse_shasums(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(filename)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.insert(
            filename.trim_start_matches('*').to_string(),
            hash.to_string(),
        );
    }
    out
}

pub(crate) fn check_shasums(
    shasums: &HashMap<String, String>,
    filename: &str,
    digest: &[u8],
) -> Result<(), String> {
    let Some(expected) = shasums.get(filename) else {
        return Err(format!("SHASUMS256 里没有 {filename} 的校验条目"));
    };
    if expected.eq_ignore_ascii_case(&hex::encode(digest)) {
        Ok(())
    } else {
        Err("校验失败：下载内容与官方 SHASUMS256 摘要不一致".into())
    }
}

/* ------------------------------- unpack ------------------------------- */

/// Mirrors `versions::extract` for zip archives (Node's Windows builds).
/// Progress counts entries from the central directory — the tar path counts
/// compressed bytes, but per-entry decompression wrappers buy little here.
pub(crate) async fn extract_zip<F: Fn(f64) + Send + Sync>(
    archive_path: &Path,
    dest: &Path,
    on_tick: &F,
    flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| e.to_string())?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<f64, String>>(16);
    let path = archive_path.to_path_buf();
    let dest = dest.to_path_buf();
    let flag_worker = Arc::clone(flag);
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|e| format!("无法读取 zip: {e}"))?;
            let total = archive.len().max(1);
            for index in 0..archive.len() {
                // Read here, not only in the receiver, or unpacking kept going
                // while the caller was already deleting the directory.
                if flag_worker.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                let mut entry = archive
                    .by_index(index)
                    .map_err(|e| format!("zip 条目读取失败: {e}"))?;
                let Some(relative) = strip_first(Path::new(entry.name())) else {
                    continue;
                };
                if relative.as_os_str().is_empty() {
                    continue; // the top-level directory entry itself
                }
                let target = safe_join(&dest, &relative)?;
                if entry.is_dir() {
                    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                } else {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut out)
                        .map_err(|e| format!("解压写入失败: {e}"))?;
                    // Node's unix archives are tarballs; only the zip path can
                    // lose the executable bit.
                    #[cfg(unix)]
                    if let Some(mode) = entry.unix_mode() {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &target,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                }
                let progress = (index + 1) as f64 / total as f64;
                // A closed channel means the caller gave up; stop rather than
                // keep writing into a directory it is cleaning up.
                if tx.blocking_send(Ok(progress)).is_err() {
                    return Err("cancelled".into());
                }
            }
            Ok(())
        })();
        let _ = tx.blocking_send(result.map(|()| 1.0));
    });

    while let Some(msg) = rx.recv().await {
        match msg {
            Ok(progress) => on_tick(progress),
            Err(e) => return Err(e),
        }
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
    }
    Ok(())
}
