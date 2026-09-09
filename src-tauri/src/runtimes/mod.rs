//! Node Runtime Manager: the catalog comes from the official nodejs.org dist
//! index (npmmirror's copy for CN users), installs download the platform
//! archive, verify it against `SHASUMS256.txt`, and unpack into
//! `<root>/runtimes/node-<major>/`.
//!
//! Instances reference runtimes by id only — the tree is shared, never copied
//! per instance (see the note in `instances.rs`).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::paths::{ensure_under_root, PhlState};
use crate::versions::now_millis;
use crate::versions::{
    cancelled, download, extract, http_client, now_iso, parse_semver, safe_join, sanitize_version,
    strip_first, Downloaded, ProgressEvent, Transfers,
};
use catalog::DistEntry;

pub(crate) mod catalog;
pub(crate) mod install;

pub(crate) use catalog::group_catalog;
#[cfg(test)]
use install::{check_shasums, dist_platform, extract_zip, parse_shasums};
pub(crate) use install::{run_runtime_install, RuntimeMarker};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::Arc;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The dist index covers every release since 2010 and is a few MB.
const INDEX_TIMEOUT: Duration = Duration::from_secs(30);

// The dist base arrives from the frontend (official `https://nodejs.org/dist`
// or the npmmirror copy, per the download-source setting). Both hosts serve
// the same layout: `/index.json`, `/v<ver>/…`, `/v<ver>/SHASUMS256.txt`.

/// Only show lines a user today would actually install: every LTS line down
/// to 16, plus the two newest majors (the current release line needs an entry
/// before it has an LTS codename).
pub(crate) const MIN_MAJOR: u64 = 16;

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

/// The full semver recorded by an installed runtime, or None when the
/// directory carries no readable marker. Shared with the environment
/// verifier, which compares it against what the binary actually reports.
pub(crate) fn runtime_version(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("phl-runtime.json")).ok()?;
    let marker: RuntimeMarker = serde_json::from_str(&raw).ok()?;
    Some(marker.version)
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
        let mut command = std::process::Command::new("node");
        command.arg("--version");
        // A console program opens a console window unless told not to; this
        // probe runs on every refresh, so it must stay silent.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::launch::CREATE_NO_WINDOW);
        }
        let output = command.output().ok()?;
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
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    dist_base: String,
    version_name: String,
    version: String,
    keep_archive: bool,
    on_progress: Channel<ProgressEvent>,
) -> Result<(), String> {
    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "runtime-install",
        format!("安装 Runtime {version_name}"),
        vec![crate::resources::Resource::Runtime(version_name.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
            task.set_phase("downloading");
            let r = run_runtime_install(
                &flag,
                &dist_base,
                &version_name,
                &version,
                &phl.root(),
                keep_archive,
                &on_progress,
            )
            .await;
            if crate::versions::cancelled(&flag) {
                return Err("cancelled".into());
            }
            r
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

#[tauri::command]
pub async fn remove_runtime_dir(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    runtime_name: String,
) -> Result<(), String> {
    crate::resources::guarded(
        crate::resources::next_task_id("runtime-remove"),
        "runtime-remove",
        format!("删除 Runtime {runtime_name}"),
        vec![crate::resources::Resource::Runtime(runtime_name.clone())],
        None,
        &locks,
        &tasks,
        move |_| async move { remove_runtime_dir_body(&phl, &runtime_name).await },
    )
    .await
}

async fn remove_runtime_dir_body(phl: &PhlState, runtime_name: &str) -> Result<(), String> {
    let safe = sanitize_version(runtime_name)?;
    let root = phl.root();
    let dir = root.join("runtimes").join(&safe);
    ensure_under_root(&root.join("runtimes"), &dir)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Bytes on disk per installed runtime, for the Runtimes overview. The dist
/// index does not publish sizes, so this is the only honest number there is.
#[tauri::command]
pub async fn runtimes_disk_usage(phl: State<'_, PhlState>) -> Result<HashMap<String, u64>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn entry(version: &str, lts: Option<&str>) -> DistEntry {
        DistEntry {
            version: version.into(),
            lts: lts.map(str::to_string),
        }
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
        assert_eq!(
            map.get("node-v22.11.0-win-x64.zip").map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            map.get("node-v22.11.0-darwin-x64.tar.gz")
                .map(String::as_str),
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
        zip.start_file("node-v0.0.0-win-x64/node.exe", opts)
            .unwrap();
        use std::io::Write;
        zip.write_all(b"bin").unwrap();
        zip.start_file("node-v0.0.0-win-x64/lib/a.js", opts)
            .unwrap();
        zip.write_all(b"// a").unwrap();
        zip.finish().unwrap();

        let dest = dir.join("out");
        extract_zip(&zip_path, &dest, &|_| {}, &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        assert!(
            dest.join("node.exe").exists(),
            "top-level node-v…/ prefix stripped"
        );
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
            zip.start_file(format!("node-v0.0.0-win-x64/f{i}.js"), opts)
                .unwrap();
            use std::io::Write;
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();

        let err = extract_zip(
            &zip_path,
            &dir.join("out"),
            &|_| {},
            &Arc::new(AtomicBool::new(true)),
        )
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
