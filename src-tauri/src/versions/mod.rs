//! Real Version Manager backing: DSH version catalog from GitHub Releases +
//! npm registry, and download / verify / extract into PHL's versions layout.
//!
//! Source priority (per the product decision): GitHub provides the release
//! list and notes, npm (`@deepseek-ai/dsh`) provides the actual tarballs.
//!
//! The domain is split along its seams: `catalog` (remote listing), `download`
//! (byte transport + integrity), `extract` (archive handling), `dependencies`
//! (the npm solve + 404 policy) and `install` (the transactional swap). This
//! module keeps the shared wire types, the cancel registry and time helpers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::paths::PhlState;

const DSH_PACKAGE: &str = "@deepseek-ai%2Fdsh";
const DSH_PACKAGE_RAW: &str = "@deepseek-ai/dsh";
const GITHUB_RELEASES: &str =
    "https://api.github.com/repos/deepseek-ai/deepseek-harness/releases?per_page=50";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("PHL/0.1 (dsh-phl)")
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .expect("reqwest client")
}

pub(crate) mod catalog;
pub(crate) mod dependencies;
pub(crate) mod download;
pub(crate) mod extract;
pub(crate) mod install;

pub(crate) use catalog::parse_semver;
pub(crate) use dependencies::{
    install_version_deps, package_requires_deps, pick_npm_capable_node, version_deps_missing,
};
pub(crate) use download::{download, sidecar_path_of, verify_integrity, Downloaded};
pub(crate) use extract::{extract, safe_join, strip_first};
#[cfg(test)]
pub(crate) use install::remove_version_dir_inner;
pub(crate) use install::{
    now_millis, promote_staged, read_marker, run_install, sanitize_version, txn_dir,
};
// Test-only surface: the unit tests live in this module and reach the rest
// of the domain through the glob above.
#[cfg(test)]
pub(crate) use catalog::list_dsh_versions;
#[cfg(test)]
pub(crate) use catalog::majors_from_engines;
#[cfg(test)]
pub(crate) use dependencies::{
    decide_not_found, parse_404_package, prune_allowed, remove_dep_from_manifest,
    strip_dev_dependencies, NotFoundAction,
};
#[cfg(test)]
pub(crate) use install::{check_version_health, InstallMarker};

/* ----------------------------- wire types ----------------------------- */

/// Mirrors the frontend `DshVersion` minus the in-memory `state` field, which
/// is derived from what actually exists under `<root>/versions/`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshVersionMeta {
    pub id: String,
    pub name: String,
    pub channel: String,
    pub released_at: String,
    pub size: u64,
    pub requires_node: Vec<u32>,
    pub notes: Vec<String>,
    pub latest: bool,
    pub legacy: bool,
    /// GitHub has cut the release but the npm package is not published yet —
    /// surfaced so the list tracks GitHub's progress instead of looking
    /// frozen while the upstream publish lags. No install source.
    pub pending_publish: bool,
    pub source: Option<VersionSourceMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSourceMeta {
    pub tarball: String,
    pub integrity: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledVersionInfo {
    pub name: String,
    pub installed_at: String,
    /// `healthy` | `degraded` — degraded means dependency pruning happened
    /// during install (`phl-deps.json` records which packages were skipped).
    #[serde(default = "default_install_health")]
    pub install_health: String,
    #[serde(default)]
    pub skipped_dependencies: Vec<String>,
}

fn default_install_health() -> String {
    "healthy".into()
}

/// Best-effort read of the dependency-install record; an absent or unreadable
/// file simply means "installed before this field existed, healthy".
fn read_deps_marker(version_dir: &Path) -> (String, Vec<String>) {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct DepsMarker {
        #[serde(default)]
        install_health: String,
        #[serde(default)]
        skipped: Vec<String>,
    }
    match std::fs::read_to_string(version_dir.join("phl-deps.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<DepsMarker>(&raw).ok())
    {
        Some(m) if !m.install_health.is_empty() => (m.install_health, m.skipped),
        _ => (default_install_health(), Vec::new()),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum ProgressEvent {
    #[serde(rename_all = "camelCase")]
    Downloading {
        progress: f64,
        bytes_done: u64,
        bytes_per_sec: u64,
    },
    #[serde(rename_all = "camelCase")]
    Extracting { progress: f64 },
    #[serde(rename_all = "camelCase")]
    Verifying,
    /// npm is materialising the version's own dependencies — the longest
    /// stage of a cold install. Progress is a slow indeterminate ramp from
    /// the npm child's wall clock, not a byte count.
    #[serde(rename_all = "camelCase")]
    InstallingDeps { progress: f64 },
}

/// Mirror one progress event into the task-registry row (O-10), so the
/// title-bar task center shows the same phase the page shows instead of
/// freezing on "准备中". Called beside every `on_progress.send` in the
/// installers — the Channel's own callback only ever sees serialized bytes,
/// so the update has to happen where the event still exists as data.
/// Indeterminate phases must clear the ratio (`None`): a stale download
/// percentage under a "安装依赖" label is exactly the lie this wiring kills.
pub(crate) fn sync_task_progress(task: &crate::resources::Task, ev: &ProgressEvent) {
    match ev {
        ProgressEvent::Downloading { progress, .. } => {
            task.set_phase("downloading");
            task.set_progress(Some(*progress));
        }
        ProgressEvent::Extracting { progress } => {
            task.set_phase("extracting");
            task.set_progress(Some(*progress));
        }
        ProgressEvent::Verifying => {
            task.set_phase("verifying");
            task.set_progress(None);
        }
        ProgressEvent::InstallingDeps { .. } => {
            // npm has no machine-readable progress without a TTY — the ramp it
            // produced was a wall-clock guess, so the task row stays
            // indeterminate here on purpose.
            task.set_phase("installing-deps");
            task.set_progress(None);
        }
    }
}

/* --------------------------- cancel registry --------------------------- */

/// Transfer ids mapped to a shared cancel flag so the frontend
/// `AbortController` can abort an in-flight Rust download.
///
/// Ids are **unique per attempt** (the frontend appends a nonce), which is
/// what makes `cancel` safe to register a flag for an id it has not seen yet:
/// a cancel that loses the race and arrives after the transfer already
/// finished leaves an entry that no future `take` will ever look up. Were the
/// ids stable per plugin/version instead, that leftover `true` would abort the
/// next install of the same thing, permanently, until the app restarted.
#[derive(Default)]
pub struct Transfers(pub Mutex<HashMap<String, Arc<AtomicBool>>>);

impl Transfers {
    pub(crate) fn take(&self, id: &str) -> Arc<AtomicBool> {
        self.0
            .lock()
            .expect("transfers lock")
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    fn cancel(&self, id: &str) {
        // The abort can beat the transfer command's own `take`: the frontend
        // fires it as soon as the user clicks, while the flag is only
        // registered after the IPC round-trip. Create the entry here so the
        // racing `take` observes the cancel instead of dropping it — safe
        // because ids are never reused (see the type's own doc comment).
        self.0
            .lock()
            .expect("transfers lock")
            .entry(id.to_string())
            .or_default()
            .store(true, Ordering::SeqCst);
    }

    pub(crate) fn release(&self, id: &str) {
        self.0.lock().expect("transfers lock").remove(id);
    }
}

pub(crate) fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

#[tauri::command]
pub fn default_root() -> String {
    crate::paths::default_root().to_string_lossy().into_owned()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn download_dsh_version(
    transfers: State<'_, Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    tarball_url: String,
    integrity: Option<String>,
    version_name: String,
    registry_base: String,
    keep_archive: bool,
    total_bytes: Option<u64>,
    on_progress: Channel<ProgressEvent>,
) -> Result<(), String> {
    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "version-install",
        format!("安装版本 {version_name}"),
        vec![crate::resources::Resource::Version(version_name.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| async move {
            let r = run_install(
                &flag,
                &tarball_url,
                integrity.as_deref(),
                &version_name,
                &phl.root(),
                &registry_base,
                keep_archive,
                total_bytes,
                &task,
                &on_progress,
            )
            .await;
            if cancelled(&flag) {
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
pub fn cancel_transfer(
    transfers: State<'_, Transfers>,
    tasks: State<'_, crate::resources::Tasks>,
    transfer_id: String,
) {
    transfers.cancel(&transfer_id);
    // The registry row (if the transfer ever registered one) learns about the
    // request too, so the task centre can show "cancelling" while the flag
    // takes effect at the next checkpoint.
    tasks.request_cancel(&transfer_id);
}

pub(crate) fn now_iso() -> String {
    // No chrono dependency: build an ISO timestamp from the unix epoch.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm (no external date crate).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha512};

    #[test]
    fn install_marker_roundtrips_written_shape() {
        // Keys must stay in lock-step with the json!() that run_install writes.
        let raw = serde_json::json!({
            "installedAt": "2026-09-03T04:31:37Z",
            "tarball": "https://registry/dsh.tgz",
            "integrity": "",
            "bytes": 15268u64,
        })
        .to_string();
        let marker: InstallMarker = serde_json::from_str(&raw).expect("marker must parse");
        assert_eq!(marker.installed_at, "2026-09-03T04:31:37Z");
    }

    #[test]
    fn orphan_404_names_are_parsed_from_npm_stderr() {
        let scoped = "npm error code E404\nnpm error 404 Not Found - GET https://registry.npmjs.org/@deepseek-ai%2Fdsh-experimental-agent-team - Not found\nnpm error 404\nnpm error 404  The requested resource '@deepseek-ai/dsh-experimental-agent-team@^0.1.2-alpha.5' could not be found or you do not have permission to access it.";
        assert_eq!(
            parse_404_package(scoped).as_deref(),
            Some("@deepseek-ai/dsh-experimental-agent-team")
        );
        let plain = "npm error 404  The requested resource 'some-pkg@1.2.3' could not be found";
        assert_eq!(parse_404_package(plain).as_deref(), Some("some-pkg"));
        assert_eq!(parse_404_package("npm error code EACCES"), None);
    }

    #[test]
    fn dev_dependencies_are_stripped_from_scratch_manifest() {
        let dir = std::env::temp_dir().join(format!("phl-strip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"dsh","dependencies":{"commander":"^15.0.0"},"devDependencies":{"@deepseek-ai/dsh-experimental-agent-team":"^0.1.0","vitest":"1.0.0"}}"#,
        )
        .unwrap();

        assert!(strip_dev_dependencies(&dir));
        let after = std::fs::read_to_string(dir.join("package.json")).unwrap();
        assert!(!after.contains("experimental"));
        assert!(!after.contains("vitest"));
        assert!(after.contains("commander"));
        // Nothing left to strip is a no-op.
        assert!(!strip_dev_dependencies(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_pruning_removes_and_leaves() {
        let dir = std::env::temp_dir().join(format!("phl-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"dsh","dependencies":{"@deepseek-ai/dsh-experimental-agent-team":"^0.1.0","commander":"^15.0.0"}}"#,
        )
        .unwrap();

        assert!(!remove_dep_from_manifest(&dir, "@deepseek-ai/nope"));
        assert!(remove_dep_from_manifest(
            &dir,
            "@deepseek-ai/dsh-experimental-agent-team"
        ));
        let after = std::fs::read_to_string(dir.join("package.json")).unwrap();
        assert!(!after.contains("experimental"));
        assert!(after.contains("commander"));
        // Second removal of the same name is a no-op.
        assert!(!remove_dep_from_manifest(
            &dir,
            "@deepseek-ai/dsh-experimental-agent-team"
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deps_gate_reads_package_json() {
        let dir = std::env::temp_dir().join(format!("phl-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No package.json → nothing to require.
        assert!(!package_requires_deps(&dir));
        // Self-contained package.
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        assert!(!package_requires_deps(&dir));
        // Dev deps do NOT block launching.
        std::fs::write(
            dir.join("package.json"),
            r#"{"devDependencies":{"vitest":"1.0.0"}}"#,
        )
        .unwrap();
        assert!(!package_requires_deps(&dir));
        // A real runtime dependency does.
        std::fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"@deepseek-ai/dsh-app-boot":"^0.1.0"}}"#,
        )
        .unwrap();
        assert!(package_requires_deps(&dir));
        // …and the launch-time probe only trips while node_modules is absent.
        assert!(version_deps_missing(&dir));
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        assert!(!version_deps_missing(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Build a minimal npm-style tarball (`package/…` prefix) in memory.
    fn make_test_tarball() -> Vec<u8> {
        // Per-call directory: tests run concurrently and this helper writes
        // its tarball through a file, so a shared name lets one call truncate
        // the mid-read bytes of another (the flaky "invalid gzip header").
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("phl-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("package/lib")).unwrap();
        std::fs::write(dir.join("package/package.json"), "{\"name\":\"dsh\"}").unwrap();
        std::fs::write(dir.join("package/lib/bin.js"), "// bin").unwrap();
        let tar_path = dir.join("test.tgz");
        let tar_gz = std::fs::File::create(&tar_path).unwrap();
        let enc = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all("package", dir.join("package")).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        std::fs::read(&tar_path).unwrap()
    }

    #[tokio::test]
    async fn integrity_and_extract_roundtrip() {
        let dir = std::env::temp_dir().join(format!("phl-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tgz = make_test_tarball();
        let part = dir.join("dsh-test.tgz.part");
        std::fs::write(&part, &tgz).unwrap();

        let digest = Sha512::digest(&tgz);
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(digest)
        );

        // Correct integrity passes.
        verify_integrity(digest.as_slice(), Some(&integrity)).unwrap();
        // A different payload's digest fails.
        assert!(
            verify_integrity(Sha512::digest(b"tampered").as_slice(), Some(&integrity)).is_err()
        );
        // An algorithm we cannot actually check must be refused, not skipped.
        assert!(verify_integrity(digest.as_slice(), Some("sha1-abc")).is_err());
        // No integrity published → nothing to check.
        verify_integrity(digest.as_slice(), None).unwrap();

        let dest = dir.join("versions").join("0.0.0-test");
        extract(&part, &dest, &|_| {}, &Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();

        assert!(
            dest.join("package.json").exists(),
            "package/ prefix stripped"
        );
        assert!(dest.join("lib/bin.js").exists());
        assert!(!dest.join("package").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// T-110 Version 行：坏压缩包（非 gzip 内容）→ 解压失败，不产生半成品树。
    #[tokio::test]
    async fn corrupt_archive_fails_extraction_without_a_partial_tree() {
        let dir = std::env::temp_dir().join(format!("phl-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("dsh-bad.tgz.part");
        std::fs::write(&part, b"this is not gzip at all").unwrap();

        let dest = dir.join("staging");
        let result = extract(&part, &dest, &|_| {}, &Arc::new(AtomicBool::new(false))).await;
        assert!(result.is_err(), "garbage bytes cannot extract");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn extract_stops_when_cancelled() {
        let dir = std::env::temp_dir().join(format!("phl-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("dsh-test.tgz.part");
        std::fs::write(&part, make_test_tarball()).unwrap();

        let flag = Arc::new(AtomicBool::new(true));
        let err = extract(&part, &dir.join("dest"), &|_| {}, &flag)
            .await
            .unwrap_err();
        assert_eq!(err, "cancelled");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_join_refuses_traversal() {
        let dest = Path::new("phl").join("versions").join("1.0.0");
        assert!(safe_join(&dest, Path::new("lib/bin.js")).is_ok());
        // Every one of these passes a `Path::starts_with` prefix check.
        assert!(safe_join(&dest, Path::new("../evil")).is_err());
        assert!(safe_join(&dest, Path::new("a/../../../evil")).is_err());
        assert!(safe_join(&dest, Path::new("/etc/passwd")).is_err());
    }

    #[tokio::test]
    async fn promote_replaces_the_old_install_and_cleans_the_backup() {
        let root = std::env::temp_dir().join(format!("phl-promote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let versions = root.join("versions");
        let dest = versions.join("0.1.0");
        std::fs::create_dir_all(dest.join("lib")).unwrap();
        std::fs::write(dest.join("lib").join("bin.js"), "// old").unwrap();

        let token = now_millis();
        let staging = txn_dir(&versions, "0.1.0", "staging", token);
        let backup = txn_dir(&versions, "0.1.0", "backup", token);
        std::fs::create_dir_all(staging.join("lib")).unwrap();
        std::fs::write(staging.join("lib").join("bin.js"), "// new").unwrap();

        promote_staged(&staging, &dest, &backup, &|_| Ok(()))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("lib").join("bin.js")).unwrap(),
            "// new"
        );
        assert!(!backup.exists(), "backup removed after a verified swap");
        assert!(!staging.exists(), "staging consumed by the rename");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn promote_rolls_the_previous_install_back_when_the_final_check_fails() {
        let root = std::env::temp_dir().join(format!("phl-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let versions = root.join("versions");
        let dest = versions.join("0.1.0");
        std::fs::create_dir_all(dest.join("lib")).unwrap();
        std::fs::write(dest.join("lib").join("bin.js"), "// old").unwrap();

        let token = now_millis();
        let staging = txn_dir(&versions, "0.1.0", "staging", token);
        let backup = txn_dir(&versions, "0.1.0", "backup", token);
        std::fs::create_dir_all(staging.join("lib")).unwrap();
        std::fs::write(staging.join("lib").join("bin.js"), "// broken").unwrap();

        let err = promote_staged(&staging, &dest, &backup, &|_| Err("坏树".into()))
            .await
            .unwrap_err();
        assert!(err.contains("已恢复原版本"), "{err}");

        assert_eq!(
            std::fs::read_to_string(dest.join("lib").join("bin.js")).unwrap(),
            "// old",
            "the previous install is back"
        );
        assert!(!staging.exists(), "the broken staging tree did not survive");
        assert!(!backup.exists(), "the backup was consumed by the rollback");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn version_health_gates_incomplete_trees() {
        let dir = std::env::temp_dir().join(format!("phl-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"name":"dsh"}"#).unwrap();

        // No marker at all → rejected before anything can read it as installed.
        assert!(check_version_health(&dir, "0.1.0").is_err());

        let marker = |v: &str| {
            std::fs::write(
                dir.join("phl-install.json"),
                serde_json::json!({ "installedAt": now_iso(), "version": v }).to_string(),
            )
            .unwrap();
        };

        // Marker naming a different version → rejected.
        marker("0.2.0");
        assert!(check_version_health(&dir, "0.1.0").is_err());

        // Matching marker but no entrypoint → rejected.
        marker("0.1.0");
        assert!(check_version_health(&dir, "0.1.0").is_err());

        // Complete tree passes.
        std::fs::write(dir.join("lib").join("bin.js"), "// bin").unwrap();
        check_version_health(&dir, "0.1.0").unwrap();

        // A manifest that requires deps without node_modules → rejected.
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"dsh","dependencies":{"commander":"^15.0.0"}}"#,
        )
        .unwrap();
        assert!(check_version_health(&dir, "0.1.0").is_err());
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        check_version_health(&dir, "0.1.0").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_denied_for_packages_outside_the_allowlist() {
        // Allowlisted experimental workspace package: prunable.
        assert!(prune_allowed("@deepseek-ai/dsh-experimental-agent-team"));
        assert!(prune_allowed("@deepseek-ai/dsh-experimental-"));
        // Everything else — core runtime deps especially — must never prune.
        assert!(!prune_allowed("commander"));
        assert!(!prune_allowed("@deepseek-ai/dsh"));
        assert!(!prune_allowed("@deepseek-ai/dsh-app-boot"));
        // A lookalike that skips the required suffix is still core.
        assert!(!prune_allowed("@deepseek-ai/dsh-experimentaloffice"));
    }

    #[test]
    fn mirror_404s_fall_back_to_official_before_any_prune_decision() {
        use NotFoundAction::*;
        let exp = "@deepseek-ai/dsh-experimental-agent-team";
        let core = "commander";

        // Mirror 404 → retry official first, whatever the package is.
        assert!(matches!(decide_not_found(exp, true, false), RetryOfficial));
        assert!(matches!(decide_not_found(core, true, false), RetryOfficial));

        // Official 404 → the allowlist decides.
        assert!(matches!(decide_not_found(exp, false, true), Prune));
        assert!(matches!(decide_not_found(core, false, true), Fatal));

        // Already on the official registry: a pending fallback flag must not
        // loop forever — the allowlist decides immediately.
        assert!(matches!(decide_not_found(exp, true, true), Prune));
        assert!(matches!(decide_not_found(core, true, true), Fatal));
    }

    #[test]
    fn engines_ranges_map_to_majors() {
        assert_eq!(
            majors_from_engines(">=20"),
            (20u32..=34).collect::<Vec<_>>()
        );
        // `||` alternatives — `VersionReq` cannot parse these in one piece.
        let alt = majors_from_engines("^18.17.0 || >=20.5.0");
        assert!(alt.contains(&18) && alt.contains(&20) && alt.contains(&22));
        assert!(!alt.contains(&19), "19 satisfies neither alternative");
        // Unparseable → unknown. Callers must not read that as "nothing works".
        assert!(majors_from_engines("garbage").is_empty());
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;

    /// Throwaway task handle for the removal tests in this module: phase
    /// writes land on a registry row nobody lists.
    fn test_task() -> crate::resources::Task {
        crate::resources::Tasks::default()
            .begin(
                crate::resources::TaskInfo::new(
                    "ver-test".into(),
                    "version-remove",
                    "test".into(),
                    &[],
                ),
                None,
            )
            .unwrap()
    }

    /// Real-network probe: run explicitly with `cargo test -- --ignored`
    /// to see what the catalog pipeline actually returns on this machine.
    #[tokio::test]
    #[ignore]
    async fn catalog_probe() {
        match list_dsh_versions("https://registry.npmjs.org".into()).await {
            Ok(list) => {
                println!("catalog OK: {} versions", list.len());
                for v in list.iter().take(4) {
                    println!(
                        "  {} channel={} size={} latest={} pending={} notes={}",
                        v.name,
                        v.channel,
                        v.size,
                        v.latest,
                        v.pending_publish,
                        v.notes.len()
                    );
                }
                assert!(!list.is_empty());
            }
            Err(e) => {
                println!("catalog FAILED: {e}");
                panic!("catalog failed: {e}");
            }
        }
    }
    #[tokio::test]
    async fn deleting_a_version_in_use_by_a_running_instance_is_refused_by_name() {
        use crate::instances::{create_instance_inner, InstanceManifest};
        use crate::launch::{ProcessEntry, Processes};
        use std::collections::HashMap;

        let root = std::env::temp_dir().join(format!("phl-rmver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let manifest = InstanceManifest {
            schema_version: 0,
            id: "pinver-01".into(),
            name: "占用版本的实例".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            port: 34490,
            auto_port: true,
            profile: "web".into(),
            created_at: now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: HashMap::new(),
            args: Vec::new(),
            api: None,
            management_mode: Default::default(),
            source: Default::default(),
            external_home: None,
            adopted_from: None,
        };
        create_instance_inner(&root, manifest).await.unwrap();

        // Simulate the completed install on disk.
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("package.json"), r#"{"name":"dsh"}"#).unwrap();
        std::fs::write(version.join("lib").join("bin.js"), "// bin").unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();

        // The instance is running.
        let processes = Processes::default();
        processes
            .0
            .lock()
            .unwrap()
            .insert("pinver-01".into(), ProcessEntry { pid: 1, port: 1 });

        let err = remove_version_dir_inner(&root, &processes, "0.1.0", &test_task())
            .await
            .unwrap_err();
        assert!(err.contains("占用版本的实例"), "{err}");
        assert!(err.contains("停止"), "{err}");
        assert!(version.exists(), "the version directory was not touched");

        // After the instance stops, the delete goes through.
        processes.0.lock().unwrap().remove("pinver-01");
        remove_version_dir_inner(&root, &processes, "0.1.0", &test_task())
            .await
            .unwrap();
        assert!(!version.exists(), "version removed once nothing pins it");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_legacy_bare_binding_still_blocks_removing_the_version_it_runs() {
        // Manifests written before the binding fix carry `0.1.0` where the
        // guard compares against `dsh-0.1.0`. An un-healed legacy instance
        // that is RUNNING must not be able to have its version deleted out
        // from under it — the comparison is legacy-tolerant.
        use crate::launch::{ProcessEntry, Processes};
        let root = std::env::temp_dir().join(format!("phl-rmver-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("instances").join("leg-01");
        std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
        // Written raw on purpose (write_manifest would canonicalize).
        std::fs::write(
            dir.join("instance.json"),
            r#"{"schemaVersion":2,"id":"leg-01","name":"旧版接入","kind":"sandbox","hue":0,
            "versionId":"0.1.0","runtimeId":"node-22","port":3080,"autoPort":true,
            "profile":"web","createdAt":"now","env":{},"args":[],"managementMode":"managed-copy",
            "source":"adopted"}"#,
        )
        .unwrap();
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();

        let processes = Processes::default();
        processes
            .0
            .lock()
            .unwrap()
            .insert("leg-01".into(), ProcessEntry { pid: 1, port: 1 });
        let err = remove_version_dir_inner(&root, &processes, "0.1.0", &test_task())
            .await
            .unwrap_err();
        assert!(err.contains("旧版接入"), "guard must name it: {err}");
        assert!(version.exists(), "the running version is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }
}
