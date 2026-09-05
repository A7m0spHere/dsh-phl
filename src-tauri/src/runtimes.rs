//! Node Runtime Manager: the catalog comes from the official nodejs.org dist
//! index (npmmirror's copy for CN users), installs download the platform
//! archive, verify it against `SHASUMS256.txt`, and unpack into
//! `<root>/runtimes/node-<major>/`.
//!
//! Instances reference runtimes by id only — the tree is shared, never copied
//! per instance (see the note in `instances.rs`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::launch::node_binary;
use crate::paths::{ensure_under_root, PhlState};
use crate::versions::now_millis;
use crate::versions::{
    cancelled, download, extract, http_client, now_iso, parse_semver, safe_join, sanitize_version,
    strip_first, Downloaded, ProgressEvent, Transfers,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The dist index covers every release since 2010 and is a few MB.
const INDEX_TIMEOUT: Duration = Duration::from_secs(30);

// The dist base arrives from the frontend (official `https://nodejs.org/dist`
// or the npmmirror copy, per the download-source setting). Both hosts serve
// the same layout: `/index.json`, `/v<ver>/…`, `/v<ver>/SHASUMS256.txt`.

/// Only show lines a user today would actually install: every LTS line down
/// to 16, plus the two newest majors (the current release line needs an entry
/// before it has an LTS codename).
const MIN_MAJOR: u64 = 16;

/* ----------------------------- wire types ----------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRuntimeMeta {
    /// `node-22` — same id the mock used, so instances keep referencing majors.
    pub id: String,
    pub major: u64,
    /// Full version of the latest release in the line, e.g. `22.12.0`.
    pub version: String,
    pub codename: Option<String>,
    pub lts: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRuntimeInfo {
    /// Directory name under `<root>/runtimes/`, e.g. `node-22`.
    pub name: String,
    pub installed_at: String,
    pub version: String,
}

/* ------------------------------ commands ------------------------------ */

#[tauri::command]
pub async fn list_node_runtimes(dist_base: String) -> Result<Vec<NodeRuntimeMeta>, String> {
    let client = http_client();
    let url = format!("{}/index.json", dist_base.trim_end_matches('/'));
    let entries: Vec<DistEntry> = client
        .get(&url)
        .timeout(INDEX_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Node dist 索引请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Node dist 索引返回错误: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Node dist 索引解析失败: {e}"))?;
    Ok(group_catalog(entries))
}

#[tauri::command]
pub async fn list_installed_runtimes(
    phl: State<'_, PhlState>,
) -> Result<Vec<InstalledRuntimeInfo>, String> {
    let dir = phl.root().join("runtimes");
    let mut out = Vec::new();
    // First launch: nothing installed yet.
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.to_string()),
    };
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // `.phl-new-*` staging dirs and OS junk are not runtimes.
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(raw) = tokio::fs::read_to_string(path.join("phl-runtime.json")).await else {
            continue; // no marker → not a completed install
        };
        let Ok(marker) = serde_json::from_str::<RuntimeMarker>(&raw) else {
            continue;
        };
        out.push(InstalledRuntimeInfo {
            name,
            installed_at: marker.installed_at,
            version: marker.version,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The `node` found on PATH, if any — the "系统 Node" entry runs this binary,
/// so the version shown must be what would actually execute. Spawned on a
/// blocking thread: `node --version` is a process launch, not a syscall.
#[tauri::command]
pub async fn system_node_version() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let output = std::process::Command::new("node").arg("--version").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let version = text.strip_prefix('v')?;
        // Accept only `MAJOR.MINOR.PATCH`; shims and wrappers print other things.
        let ok = version.split('.').count() == 3
            && version.chars().all(|c| c.is_ascii_digit() || c == '.');
        ok.then(|| version.to_string())
    })
    .await
    .ok()
    .flatten()
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn download_node_runtime(
    transfers: State<'_, Transfers>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    dist_base: String,
    version_name: String,
    version: String,
    keep_archive: bool,
    on_progress: Channel<ProgressEvent>,
) -> Result<(), String> {
    let flag = transfers.take(&transfer_id);
    let result = run_runtime_install(
        &flag,
        &dist_base,
        &version_name,
        &version,
        &phl.root(),
        keep_archive,
        &on_progress,
    )
    .await;
    transfers.release(&transfer_id);
    result
}

#[tauri::command]
pub async fn remove_runtime_dir(
    phl: State<'_, PhlState>,
    runtime_name: String,
) -> Result<(), String> {
    let safe = sanitize_version(&runtime_name)?;
    let root = phl.root();
    let dir = root.join("runtimes").join(&safe);
    ensure_under_root(&root.join("runtimes"), &dir)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Bytes on disk per installed runtime, for the Runtimes overview. The dist
/// index does not publish sizes, so this is the only honest number there is.
#[tauri::command]
pub async fn runtimes_disk_usage(
    phl: State<'_, PhlState>,
) -> Result<HashMap<String, u64>, String> {
    let dir = phl.root().join("runtimes");
    let mut out = HashMap::new();
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.to_string()),
    };
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        // Staging dirs are not installed runtimes; don't count their bytes.
        if path.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            out.insert(
                entry.file_name().to_string_lossy().into_owned(),
                crate::instances::dir_size(&path),
            );
        }
    }
    Ok(out)
}

/* ------------------------------- install ------------------------------- */

#[derive(Deserialize)]
// Same on-disk shape as the install marker: camelCase keys.
#[serde(rename_all = "camelCase")]
struct RuntimeMarker {
    installed_at: String,
    version: String,
}

async fn run_runtime_install(
    flag: &Arc<AtomicBool>,
    dist_base: &str,
    version_name: &str,
    version: &str,
    root: &Path,
    keep_archive: bool,
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
    tokio::fs::create_dir_all(&runtimes_dir).await.map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| e.to_string())?;

    // Everything below lands in the staging dir; the installed tree for this
    // major is only touched once a complete replacement exists. Deleting the
    // old tree up front would leave instances pointing at a runtime that a
    // failed download just destroyed.
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let downloaded: Downloaded = download(flag, &url, &part_path, None, &|progress, bytes_done, bytes_per_sec| {
        let _ = on_progress.send(ProgressEvent::Downloading { progress, bytes_done, bytes_per_sec });
    })
    .await?;

    on_progress
        .send(ProgressEvent::Verifying)
        .map_err(|e| e.to_string())?;
    let client = http_client();
    let shasums = fetch_shasums(&client, base, &version).await?;
    check_shasums(&shasums, &filename, &downloaded.sha256)?;

    on_progress
        .send(ProgressEvent::Extracting { progress: 0.0 })
        .map_err(|e| e.to_string())?;
    if platform.ends_with(".zip") {
        extract_zip(&part_path, &staging, &|progress| {
            let _ = on_progress.send(ProgressEvent::Extracting { progress });
        }, flag)
        .await?;
    } else {
        // The .tar.gz nests everything under node-v…-<platform>/;
        // versions::extract strips the first component, same as npm's package/.
        extract(&part_path, &staging, &|progress| {
            let _ = on_progress.send(ProgressEvent::Extracting { progress });
        }, flag)
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
    crate::versions::promote_staged(
        &staging,
        &dest,
        &backup,
        &|installed: &Path| {
            let marker = std::fs::read_to_string(installed.join("phl-runtime.json"))
                .map_err(|e| format!("Runtime 标记读取失败: {e}"))?;
            let parsed: RuntimeMarker = serde_json::from_str(&marker)
                .map_err(|e| format!("Runtime 标记解析失败: {e}"))?;
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
        },
    )
    .await?;
    Ok(())
}

/// A staged runtime must contain its node binary, and that binary must run
/// and report the requested version. Spawned on a blocking thread — this is
/// a process launch, not a syscall.
async fn check_runtime_health(staging: &Path, version: &str) -> Result<(), String> {
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
fn dist_platform() -> Result<&'static str, String> {
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

/* ------------------------------- catalog ------------------------------- */

#[derive(Deserialize)]
struct DistEntry {
    version: String,
    /// The index spells non-LTS as the JSON literal `false`, which
    /// `Option<String>` alone would reject.
    #[serde(default, deserialize_with = "lts_codename")]
    lts: Option<String>,
}

fn lts_codename<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value: serde_json::Value = Deserialize::deserialize(d)?;
    Ok(value.as_str().map(str::to_string))
}

struct MajorLine {
    major: u64,
    version: String,
    codename: Option<String>,
    semver: semver::Version,
}

/// Collapses the per-release index into one entry per major: the newest
/// release in the line, its LTS status, and its codename.
fn group_catalog(entries: Vec<DistEntry>) -> Vec<NodeRuntimeMeta> {
    let mut lines: Vec<MajorLine> = Vec::new();
    for entry in entries {
        let version = entry.version.trim_start_matches('v').to_string();
        let Some(semver) = parse_semver(&version) else {
            continue;
        };
        let line = MajorLine { major: semver.major, version, codename: entry.lts, semver };
        let major = line.major;
        match lines.iter_mut().find(|l| l.major == major) {
            Some(existing) if existing.semver < line.semver => *existing = line,
            Some(_) => {}
            None => lines.push(line),
        }
    }
    lines.retain(|l| l.major >= MIN_MAJOR);
    lines.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| b.semver.cmp(&a.semver)));
    lines
        .into_iter()
        .enumerate()
        .filter(|(rank, l)| l.codename.is_some() || *rank < 2)
        .map(|(_, l)| NodeRuntimeMeta {
            id: format!("node-{}", l.major),
            major: l.major,
            version: l.version,
            lts: l.codename.is_some(),
            codename: l.codename,
        })
        .collect()
}

/* ------------------------------- verify ------------------------------- */

async fn fetch_shasums(
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
fn parse_shasums(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(filename)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.insert(filename.trim_start_matches('*').to_string(), hash.to_string());
    }
    out
}

fn check_shasums(
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
async fn extract_zip<F: Fn(f64) + Send + Sync>(
    archive_path: &Path,
    dest: &Path,
    on_tick: &F,
    flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dest).await.map_err(|e| e.to_string())?;
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
                    std::io::copy(&mut entry, &mut out).map_err(|e| format!("解压写入失败: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn entry(version: &str, lts: Option<&str>) -> DistEntry {
        DistEntry { version: version.into(), lts: lts.map(str::to_string) }
    }

    #[test]
    fn runtime_marker_roundtrips_written_shape() {
        let raw = serde_json::json!({
            "installedAt": "2026-09-03T04:31:37Z",
            "version": "22.11.0",
            "bytes": 30_000_000u64,
            "sha256": "ab",
        })
        .to_string();
        let marker: RuntimeMarker = serde_json::from_str(&raw).expect("marker must parse");
        assert_eq!(marker.installed_at, "2026-09-03T04:31:37Z");
        assert_eq!(marker.version, "22.11.0");
    }

    #[test]
    fn catalog_groups_latest_per_major_and_filters() {
        let metas = group_catalog(vec![
            entry("v22.11.0", Some("Jod")),
            entry("v22.12.0", Some("Jod")),
            entry("v23.1.0", None),
            entry("v21.7.3", None),
            entry("v18.20.4", Some("Hydrogen")),
            entry("v14.21.3", Some("Fermium")),
        ]);
        let ids: Vec<&str> = metas.iter().map(|m| m.id.as_str()).collect();
        // 21: neither LTS nor among the two newest lines; 14: below the floor.
        assert!(!ids.contains(&"node-21"));
        assert!(!ids.contains(&"node-14"));
        // 22 keeps only the newest patch of the line.
        let node22 = metas.iter().find(|m| m.id == "node-22").unwrap();
        assert_eq!(node22.version, "22.12.0");
        assert!(node22.lts);
        // 23 is the newest major — shown even without an LTS codename.
        let node23 = metas.iter().find(|m| m.id == "node-23").unwrap();
        assert!(!node23.lts);
        // Newest line first.
        assert_eq!(metas.first().unwrap().major, 23);
    }

    #[test]
    fn lts_field_is_boolean_or_codename() {
        let e: DistEntry =
            serde_json::from_str(r#"{"version":"v23.1.0","date":"2024-10-16","lts":false}"#)
                .unwrap();
        assert!(e.lts.is_none());
        let e: DistEntry =
            serde_json::from_str(r#"{"version":"v22.11.0","date":"2024-10-29","lts":"Jod"}"#)
                .unwrap();
        assert_eq!(e.lts.as_deref(), Some("Jod"));
    }

    #[test]
    fn platform_matches_host_shape() {
        let platform = dist_platform().unwrap();
        match std::env::consts::OS {
            "windows" => assert!(platform.ends_with(".zip")),
            _ => assert!(platform.ends_with(".tar.gz")),
        }
    }

    #[test]
    fn shasums_are_parsed_and_enforced() {
        let map = parse_shasums(
            "abc123  node-v22.11.0-win-x64.zip\n0123 *node-v22.11.0-darwin-x64.tar.gz\n\n",
        );
        assert_eq!(map.get("node-v22.11.0-win-x64.zip").map(String::as_str), Some("abc123"));
        assert_eq!(
            map.get("node-v22.11.0-darwin-x64.tar.gz").map(String::as_str),
            Some("0123")
        );

        let digest = Sha256::digest(b"payload");
        // Right name, wrong digest → refused.
        assert!(check_shasums(&map, "node-v22.11.0-win-x64.zip", digest.as_slice()).is_err());
        // Digest not in the list at all → refused, not skipped.
        assert!(check_shasums(&map, "node-v22.11.0-missing.zip", digest.as_slice()).is_err());
        let mut map = map;
        map.insert("node-v22.11.0-win-x64.zip".into(), hex::encode(digest));
        check_shasums(&map, "node-v22.11.0-win-x64.zip", digest.as_slice()).unwrap();
    }

    #[test]
    fn system_node_parse_rejects_shims() {
        // Mirrors the gate in system_node_version; kept as a unit test because
        // the real detection depends on the machine's PATH.
        let parse = |text: &str| {
            let version = text.trim().strip_prefix('v')?;
            let ok = version.split('.').count() == 3
                && version.chars().all(|c| c.is_ascii_digit() || c == '.');
            ok.then(|| version.to_string())
        };
        assert_eq!(parse("v22.9.0\n").as_deref(), Some("22.9.0"));
        assert_eq!(parse("node was not found"), None);
        assert_eq!(parse("v22"), None);
    }

    #[tokio::test]
    async fn zip_roundtrip_strips_top_dir() {
        let dir = std::env::temp_dir().join(format!("phl-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/node-v0.0.0-win-x64/lib")).unwrap();

        let zip_path = dir.join("test.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.add_directory("node-v0.0.0-win-x64/lib", opts).unwrap();
        zip.start_file("node-v0.0.0-win-x64/node.exe", opts).unwrap();
        use std::io::Write;
        zip.write_all(b"bin").unwrap();
        zip.start_file("node-v0.0.0-win-x64/lib/a.js", opts).unwrap();
        zip.write_all(b"// a").unwrap();
        zip.finish().unwrap();

        let dest = dir.join("out");
        extract_zip(&zip_path, &dest, &|_| {}, &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert!(dest.join("node.exe").exists(), "top-level node-v…/ prefix stripped");
        assert!(dest.join("lib/a.js").exists());
        assert!(!dest.join("node-v0.0.0-win-x64").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn zip_extract_stops_when_cancelled() {
        let dir = std::env::temp_dir().join(format!("phl-zip-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let zip_path = dir.join("test.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..50 {
            zip.start_file(format!("node-v0.0.0-win-x64/f{i}.js"), opts).unwrap();
            use std::io::Write;
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();

        let err = extract_zip(&zip_path, &dir.join("out"), &|_| {}, &Arc::new(AtomicBool::new(true)))
            .await
            .unwrap_err();
        assert_eq!(err, "cancelled");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_join_still_guards_zip_entries() {
        let dest = Path::new("phl").join("runtimes").join("node-22");
        assert!(safe_join(&dest, Path::new("node.exe")).is_ok());
        assert!(safe_join(&dest, Path::new("../evil")).is_err());
        assert!(safe_join(&dest, Path::new("C:\\evil")).is_err());
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;

    /// Real-network probe: run explicitly with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn catalog_probe() {
        match list_node_runtimes("https://nodejs.org/dist".into()).await {
            Ok(list) => {
                println!("runtime catalog OK: {} lines", list.len());
                for m in &list {
                    println!("  {} v{} lts={}", m.id, m.version, m.lts);
                }
                assert!(!list.is_empty());
            }
            Err(e) => panic!("runtime catalog failed: {e}"),
        }
    }
}
