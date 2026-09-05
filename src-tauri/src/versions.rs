//! Real Version Manager backing: DSH version catalog from GitHub Releases +
//! npm registry, and download / verify / extract into PHL's versions layout.
//!
//! Source priority (per the product decision): GitHub provides the release
//! list and notes, npm (`@deepseek-ai/dsh`) provides the actual tarballs.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tauri::ipc::Channel;
use tauri::State;

use crate::paths::{ensure_under_root, PhlState};

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

/* ------------------------------ commands ------------------------------ */

#[tauri::command]
pub fn default_root() -> String {
    crate::paths::default_root().to_string_lossy().into_owned()
}

#[tauri::command]
pub async fn list_dsh_versions(registry_base: String) -> Result<Vec<DshVersionMeta>, String> {
    let client = http_client();
    let (npm, latest_semver) = npm_catalog(&client, &registry_base).await?;
    // CN registry choice implies the user also wants CN-friendly GitHub access.
    let prefer_mirror = registry_base.contains("npmmirror");
    let github = github_releases(&client, prefer_mirror)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[phl] github releases unavailable, npm-only catalog: {e}");
            HashMap::new()
        });

    let npm_names: std::collections::HashSet<String> =
        npm.iter().map(|(ver, _)| ver.clone()).collect();
    let mut out: Vec<DshVersionMeta> = npm
        .into_iter()
        .map(|(ver, entry)| {
            // A version line older than the latest minor is the previous
            // maintenance line — treat it as legacy.
            let legacy = entry
                .semver
                .as_ref()
                .zip(latest_semver.as_ref())
                .map(|(v, latest)| v.minor < latest.minor)
                .unwrap_or(false);
            let (released_at, notes) = match github.get(&ver) {
                Some(rel) => (rel.published_at.clone(), rel.notes.clone()),
                None => (entry.published_at.clone(), Vec::new()),
            };
            DshVersionMeta {
                id: format!("dsh-{ver}"),
                name: ver,
                channel: entry.channel,
                released_at,
                size: entry.size,
                requires_node: entry.requires_node,
                notes,
                latest: entry.latest,
                legacy,
                pending_publish: false,
                source: Some(VersionSourceMeta {
                    tarball: entry.tarball,
                    integrity: entry.integrity,
                }),
            }
        })
        .collect();

    // GitHub releases the team has cut but not published to npm yet. They
    // join the list as read-only rows so "GitHub is ahead" is visible; the
    // legacy/latest flags deliberately stay false — *installable* progress
    // is still what those badges mean.
    for (ver, rel) in github.iter().filter(|(ver, _)| !npm_names.contains(*ver)) {
        let Some(sem) = parse_semver(ver) else {
            continue;
        };
        // The 0.1.2-rc.1 line was published 15 minutes after each GitHub tag
        // historically; a day-old absence is an upstream decision, so say
        // *when* it was cut rather than implying it is imminent.
        let channel = match sem.pre.as_str() {
            "" => "stable",
            p if p.starts_with("rc") => "rc",
            _ => "alpha",
        }
        .to_string();
        out.push(DshVersionMeta {
            id: format!("dsh-{ver}"),
            name: ver.clone(),
            channel,
            released_at: rel.published_at.clone(),
            size: 0,
            requires_node: Vec::new(),
            notes: rel.notes.clone(),
            latest: false,
            legacy: false,
            pending_publish: true,
            source: None,
        });
    }

    out.sort_by(|a, b| {
        parse_semver(&b.name)
            .unwrap_or_else(|| semver::Version::new(0, 0, 0))
            .cmp(&parse_semver(&a.name).unwrap_or_else(|| semver::Version::new(0, 0, 0)))
    });
    Ok(out)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn download_dsh_version(
    transfers: State<'_, Transfers>,
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
    let result = run_install(
        &flag,
        &tarball_url,
        integrity.as_deref(),
        &version_name,
        &phl.root(),
        &registry_base,
        keep_archive,
        total_bytes,
        &on_progress,
    )
    .await;
    transfers.release(&transfer_id);
    result
}

#[tauri::command]
pub fn cancel_transfer(transfers: State<'_, Transfers>, transfer_id: String) {
    transfers.cancel(&transfer_id);
}

#[tauri::command]
pub async fn list_installed_versions(
    phl: State<'_, PhlState>,
) -> Result<Vec<InstalledVersionInfo>, String> {
    let dir = phl.root().join("versions");
    let mut out = Vec::new();
    // A missing directory just means nothing has been installed yet — the
    // very first launch always lands here.
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
        let installed_at = match read_marker(&path).await {
            Some(marker) => marker.installed_at,
            None => continue, // no marker → not a completed install
        };
        let (install_health, skipped_dependencies) = read_deps_marker(&path);
        out.push(InstalledVersionInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            installed_at,
            install_health,
            skipped_dependencies,
        });
    }
    out.sort_by(|a, b| a.installed_at.cmp(&b.installed_at));
    Ok(out)
}

#[tauri::command]
pub async fn remove_version_dir(
    phl: State<'_, PhlState>,
    version_name: String,
) -> Result<(), String> {
    let safe = sanitize_version(&version_name)?;
    let root = phl.root();
    let dir = root.join("versions").join(&safe);
    // Canonical containment: a junction planted at the version path must not
    // redirect `remove_dir_all` outside the data root.
    ensure_under_root(&root.join("versions"), &dir)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/* --------------------------- catalog sources --------------------------- */

struct NpmEntry {
    channel: String,
    published_at: String,
    size: u64,
    /// Node majors from the package's `engines.node`; empty means the
    /// registry did not declare one, which is "unknown", not "none".
    requires_node: Vec<u32>,
    latest: bool,
    tarball: String,
    integrity: Option<String>,
    semver: Option<semver::Version>,
}

pub(crate) fn parse_semver(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v).ok()
}

#[derive(Deserialize)]
struct Packument {
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    #[serde(default)]
    time: HashMap<String, String>,
    versions: HashMap<String, PackumentVersion>,
}

#[derive(Deserialize)]
struct PackumentVersion {
    #[serde(default)]
    dist: NpmDist,
    #[serde(default)]
    engines: Engines,
}

#[derive(Deserialize, Default)]
struct Engines {
    #[serde(default)]
    node: Option<String>,
}

#[derive(Deserialize, Default)]
struct NpmDist {
    #[serde(default)]
    tarball: String,
    #[serde(default)]
    integrity: Option<String>,
    /// npm spells this `unpackedSize`; without the rename it silently stayed
    /// `None` and every version reported a size of 0.
    #[serde(default, rename = "unpackedSize")]
    unpacked_size: Option<u64>,
}

/// `engines.node` is a semver *range* ("^20 || >=22"), but the UI and the
/// launch check want concrete majors. Probing candidate majors is more
/// honest than trying to invert the range: `>=18.17` has to keep counting 18
/// as supported, which a naive lower-bound read would get wrong.
///
/// The `||` split is not optional. node's range syntax allows alternatives,
/// the `semver` crate's `VersionReq` only understands `,`-separated
/// comparators, and a failed parse here reads as "unknown" — so without this
/// a perfectly ordinary `"^20 || >=22"` would silently declare no supported
/// runtime at all.
fn majors_from_engines(range: &str) -> Vec<u32> {
    let reqs: Vec<semver::VersionReq> = range
        .split("||")
        .filter_map(|part| semver::VersionReq::parse(part.trim()).ok())
        .collect();
    if reqs.is_empty() {
        return Vec::new();
    }
    (14u32..=34)
        .filter(|major| {
            [0u64, 12, 17, 99].iter().any(|&minor| {
                let candidate = semver::Version::new(u64::from(*major), minor, 0);
                reqs.iter().any(|req| req.matches(&candidate))
            })
        })
        .collect()
}

async fn npm_catalog(
    client: &reqwest::Client,
    registry_base: &str,
) -> Result<(Vec<(String, NpmEntry)>, Option<semver::Version>), String> {
    let url = format!("{registry_base}/{DSH_PACKAGE}");
    let packument: Packument = client
        .get(&url)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("npm registry 请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("npm registry 返回错误: {e}"))?
        .json()
        .await
        .map_err(|e| format!("npm registry 响应解析失败: {e}"))?;

    let latest = packument.dist_tags.get("latest").cloned();
    let latest_semver = latest.as_deref().and_then(parse_semver);

    let mut out = Vec::new();
    for (ver, pv) in packument.versions {
        let sem = parse_semver(&ver);
        let channel = match sem.as_ref().map(|s| s.pre.as_str()) {
            Some(pre) if pre.starts_with("rc") => "rc".to_string(),
            Some(pre) if pre.starts_with("alpha") || pre.starts_with("beta") => "alpha".to_string(),
            Some(_) => "alpha".to_string(),
            None => "stable".to_string(),
        };
        let published_at = packument
            .time
            .get(&ver)
            .cloned()
            .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".into());
        // The registry does not publish the tarball's compressed size, and
        // probing every version with a range request cost one HTTP round
        // trip per published version on every catalog load. `unpackedSize`
        // is the honest cheap answer; the exact byte count arrives from the
        // response's `content-length` when the version is actually installed.
        let size = pv.dist.unpacked_size.unwrap_or(0);
        let requires_node = pv
            .engines
            .node
            .as_deref()
            .map(majors_from_engines)
            .unwrap_or_default();
        let is_latest = latest.as_deref() == Some(ver.as_str());
        out.push((
            ver,
            NpmEntry {
                channel,
                published_at,
                size,
                requires_node,
                latest: is_latest,
                tarball: pv.dist.tarball,
                integrity: pv.dist.integrity,
                semver: sem,
            },
        ));
    }
    if out.is_empty() {
        return Err(format!(
            "npm registry 上没有找到 {DSH_PACKAGE_RAW} 的任何版本"
        ));
    }
    Ok((out, latest_semver))
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

struct GhInfo {
    published_at: String,
    notes: Vec<String>,
}

/// Release bodies are raw markdown aimed at GitHub rendering. Strip the
/// markup so the version list shows plain sentences: headings, rules, image
/// tags and language-switch links carry no information on their own.
fn clean_note_line(line: &str) -> String {
    let text = line.trim();
    if text.is_empty() || text.starts_with('#') || text == "---" {
        return String::new();
    }
    // `[中文](#anchor)`-style language switches are pure chrome.
    if text.starts_with('[') && text.ends_with(')') && text.contains("](") {
        return String::new();
    }
    // Drop HTML tags (`<h3 id="…">`, `</h3>`, `<img …>`).
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let cleaned = out
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim();
    cleaned.to_string()
}

async fn github_releases(
    client: &reqwest::Client,
    prefer_mirror: bool,
) -> Result<HashMap<String, GhInfo>, String> {
    // api.github.com is unreachable for many CN networks. Prefix proxies that
    // mirror the full URL work for the API too. The endpoints are raced in
    // parallel rather than tried in sequence: a sequential fallback pays the
    // full 8-second timeout of every dead source *before* the good one starts,
    // which is exactly what made each catalog sync feel slow. `prefer_mirror`
    // still decides the winner when both return the same data at the same time.
    const MIRRORS: &[&str] = &["https://gh-proxy.com/"];
    let official = GITHUB_RELEASES.to_string();
    let mut candidates: Vec<String> = MIRRORS.iter().map(|m| format!("{m}{official}")).collect();
    if !prefer_mirror {
        candidates.push(official);
    }

    let mut set = tokio::task::JoinSet::new();
    for url in &candidates {
        set.spawn(fetch_github_releases(client.clone(), url.clone()));
    }

    // Preferred-first order: the loop returns on the *first* success, so with
    // the mirror at the front of `candidates` a CN user is served by the
    // mirror as soon as it answers, and vice versa. JoinSet completion order
    // is arrival order, not candidate order, so instead of racing blindly we
    // keep every successful result and prefer the candidate we listed first.
    let mut results: HashMap<String, HashMap<String, GhInfo>> = HashMap::new();
    let mut last_err = String::from("没有可用的 GitHub 源");
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok((url, map))) => {
                results.insert(url, map);
            }
            Ok(Err(e)) => {
                eprintln!("[phl] github source failed: {e}");
                last_err = e;
            }
            Err(e) => eprintln!("[phl] github source task crashed: {e}"),
        }
    }
    for url in &candidates {
        if let Some(map) = results.remove(url) {
            return Ok(map);
        }
    }
    Err(last_err)
}

async fn fetch_github_releases(
    client: reqwest::Client,
    url: String,
) -> Result<(String, HashMap<String, GhInfo>), String> {
    let releases: Vec<GhRelease> = client
        .get(&url)
        // GitHub is frequently unreachable behind CN proxies; fail fast and
        // let the next source (or npm-only) carry the catalog instead.
        .timeout(Duration::from_secs(8))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub 请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GitHub 返回错误: {e}"))?
        .json()
        .await
        .map_err(|e| format!("GitHub 响应解析失败: {e}"))?;

    let mut out = HashMap::new();
    for release in releases {
        // Tags look like `dsh-v0.1.2-alpha.3`.
        let Some(version) = release.tag_name.strip_prefix("dsh-v") else {
            continue;
        };
        let notes = release
            .body
            .unwrap_or_default()
            .lines()
            .map(clean_note_line)
            .filter(|l| !l.is_empty())
            .take(6)
            .collect();
        out.insert(
            version.to_string(),
            GhInfo {
                published_at: release.published_at.unwrap_or_default(),
                notes,
            },
        );
    }
    Ok((url, out))
}

/* --------------------------- dependency install -------------------------- */

/// An npm tarball contains the package *only* — dependencies never do. DSH's
/// `lib/bin.js` ESM-imports its own runtime deps (`@deepseek-ai/dsh-app-boot`
/// and friends), and bare-specifier resolution walks up from
/// `versions/<name>/`, so the only tree they can ever be found in is the
/// version's own `node_modules`. Extraction alone produces a version that
/// cannot boot.
pub(crate) fn package_requires_deps(version_dir: &Path) -> bool {
    #[derive(Deserialize)]
    struct PackageJson {
        #[serde(default)]
        dependencies: Option<HashMap<String, serde_json::Value>>,
        #[serde(default, rename = "peerDependencies")]
        peer_dependencies: Option<HashMap<String, serde_json::Value>>,
    }
    let Ok(raw) = std::fs::read_to_string(version_dir.join("package.json")) else {
        return false;
    };
    match serde_json::from_str::<PackageJson>(&raw) {
        Ok(pkg) => {
            !pkg.dependencies.unwrap_or_default().is_empty()
                || !pkg.peer_dependencies.unwrap_or_default().is_empty()
        }
        Err(_) => false,
    }
}

/// The launch-time repair probe for versions installed before the dependency
/// step existed (and for node_modules the user deleted).
pub(crate) fn version_deps_missing(version_dir: &Path) -> bool {
    package_requires_deps(version_dir) && !version_dir.join("node_modules").exists()
}

/// The npm CLI bundled with a Node distribution — not a globally installed
/// `npm.cmd`, whose shebang could resolve to a *different* node and silently
/// install against the wrong version. Candidates cover the Windows layout
/// (node.exe beside `node_modules/`) and the dist layout (`bin/` + `lib/`).
fn find_npm_cli(node_program: &Path) -> Option<PathBuf> {
    let exec = if node_program.as_os_str() == "node" {
        // System node: `node` on PATH. Ask it where it actually lives —
        // once, on a blocking thread via the caller? this fn is sync and the
        // call is cheap next to an npm install.
        let out = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if text.is_empty() {
            return None;
        }
        PathBuf::from(text)
    } else {
        node_program.to_path_buf()
    };
    let dir = exec.parent()?;
    let candidates = [
        dir.join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js"),
        dir.parent()?
            .join("lib")
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Pick a Node that can *run npm* at install time: the newest installed
/// runtime that ships its bundled npm-cli, falling back to system node.
/// The choice is irrelevant to what the instance executes — this node only
/// materialises `versions/<name>/node_modules`.
pub(crate) fn pick_npm_capable_node(root: &Path) -> Option<PathBuf> {
    let dir = root.join("runtimes");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    for name in names {
        let base = dir.join(&name);
        let node = if cfg!(windows) {
            base.join(crate::launch::node_binary())
        } else {
            base.join("bin").join(crate::launch::node_binary())
        };
        if node.exists() && find_npm_cli(&node).is_some() {
            return Some(node);
        }
    }
    None
}

/// Strip the version's declared devDependencies before running npm.
///
/// dsh is a monorepo package: its dev deps reference unpublished workspace
/// members (`@deepseek-ai/dsh-experimental-*`), and npm resolves the *full*
/// tree — dev deps included — even with `--omit=dev`, so one phantom package
/// 404s the entire runtime install. Dev deps can never be needed at launch,
/// so they come out of a scratch manifest first; the original bytes are
/// restored before `install_version_deps` returns.
fn strip_dev_dependencies(version_dir: &Path) -> bool {
    let path = version_dir.join("package.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut pkg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(map) = pkg.as_object_mut() else {
        return false;
    };
    if map
        .get("devDependencies")
        .and_then(|v| v.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(true)
    {
        return false;
    }
    map.remove("devDependencies");
    let Ok(modified) = serde_json::to_string_pretty(&pkg) else {
        return false;
    };
    std::fs::write(&path, modified).is_ok()
}

/// Remove a named dependency from the version's package.json; true if the
/// manifest changed. The caller holds the original bytes and restores them
/// once node_modules exists — the pruning is solver input, never the shipped
/// manifest.
fn remove_dep_from_manifest(version_dir: &Path, name: &str) -> bool {
    let path = version_dir.join("package.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut pkg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(map) = pkg.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in ["dependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(deps) = map.get_mut(key).and_then(|d| d.as_object_mut()) {
            changed |= deps.remove(name).is_some();
        }
    }
    if !changed {
        return false;
    }
    let Ok(modified) = serde_json::to_string_pretty(&pkg) else {
        return false;
    };
    std::fs::write(&path, modified).is_ok()
}

/// The official npm registry. Mirrors can lag or drop packages; a 404 from a
/// mirror is retried here once before any prune decision is made.
pub(crate) const OFFICIAL_NPM_REGISTRY: &str = "https://registry.npmjs.org";

/// The only dependencies a 404 may ever drop from the scratch manifest.
///
/// DSH ships a workspace manifest: its experimental sub-packages are declared
/// as (dev-)dependencies but are not always published, so npm's full-tree
/// solve 404s on them even though nothing imports them at runtime. Anything
/// outside this prefix 404ing is a real problem — silently pruning a core
/// dependency must never be able to look like a successful install.
pub(crate) const PRUNE_ALLOWLIST: &[&str] = &["@deepseek-ai/dsh-experimental-"];

pub(crate) fn prune_allowed(pkg: &str) -> bool {
    PRUNE_ALLOWLIST.iter().any(|prefix| pkg.starts_with(prefix))
}

/// What the retry loop does with a 404 on `pkg`.
pub(crate) enum NotFoundAction {
    /// The current registry is a mirror that may simply lag: retry the whole
    /// solve against the official registry before deciding anything.
    RetryOfficial,
    /// Confirmed missing on the official registry, and allowlisted: drop it
    /// from the scratch manifest and retry. The install is marked degraded.
    Prune,
    /// Confirmed missing and not allowlisted: a required dependency is gone,
    /// and the install must fail rather than ship without it.
    Fatal,
}

pub(crate) fn decide_not_found(
    pkg: &str,
    fallback_pending: bool,
    using_official: bool,
) -> NotFoundAction {
    if !using_official && fallback_pending {
        return NotFoundAction::RetryOfficial;
    }
    if prune_allowed(pkg) {
        NotFoundAction::Prune
    } else {
        NotFoundAction::Fatal
    }
}

/// Pull the package name out of npm's not-found line:
/// `npm error 404  The requested resource '@scope/name@^1.0.0' could not be found`
fn parse_404_package(stderr_text: &str) -> Option<String> {
    for line in stderr_text.lines() {
        const MARKER: &str = "The requested resource '";
        let Some(idx) = line.find(MARKER) else {
            continue;
        };
        let rest = &line[idx + MARKER.len()..];
        let Some(end) = rest.find('\'') else { continue };
        let spec = &rest[..end]; // `@scope/name@range` or `name@range`
        let at = spec.rfind('@')?;
        if at == 0 {
            continue; // unscoped `name` with no range should not happen; skip anyway
        }
        return Some(spec[..at].to_string());
    }
    None
}
/// Run `npm install` for a version's own dependencies. Dev deps are stripped
/// from the manifest first (see `strip_dev_dependencies`), scripts are never
/// run (this is a package manager, not a build system), and the user's
/// configured registry mirror is honoured. A dependency that still 404s is
/// pruned from the scratch manifest and the solve is retried — refusing to
/// start the whole instance over one phantom package is worse than starting
/// it without an unused experimental plugin. Returns the skipped list.
/// Progress is a slow indeterminate ramp per attempt.
pub(crate) async fn install_version_deps(
    node_program: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: impl Fn(f64) + Send + Sync,
) -> Result<Vec<String>, String> {
    let npm_cli = find_npm_cli(node_program).ok_or(
        "未找到 Node 自带的 npm-cli.js，无法安装 DSH 版本依赖。请更换实例绑定的 Runtime 为 PHL 安装的 Node，或重新安装 Node。",
    )?;
    // The manifest edits below are solver input only; the shipped bytes come
    // back before this function returns, success or not.
    let original_manifest =
        std::fs::read_to_string(version_dir.join("package.json")).unwrap_or_default();
    let mut skipped: Vec<String> = Vec::new();
    strip_dev_dependencies(version_dir);

    let outcome = install_deps_attempts(
        node_program,
        &npm_cli,
        version_dir,
        registry_base,
        flag,
        &on_progress,
        &mut skipped,
    )
    .await;

    if !original_manifest.is_empty() {
        let _ = std::fs::write(version_dir.join("package.json"), &original_manifest);
    }
    let skipped = outcome?;
    // The install succeeded, but a pruned dependency means the environment is
    // a known-degraded derivative of the manifest — recorded where both the
    // verifier and the UI can find it.
    let marker = serde_json::json!({
        "installedAt": now_iso(),
        "installHealth": if skipped.is_empty() { "healthy" } else { "degraded" },
        "skipped": skipped,
    });
    let _ = std::fs::write(version_dir.join("phl-deps.json"), marker.to_string());
    on_progress(1.0);
    Ok(skipped)
}

/// One npm attempt per loop pass; a resolvable 404 prunes and retries.
#[allow(clippy::too_many_arguments)]
async fn install_deps_attempts(
    node_program: &Path,
    npm_cli: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    skipped: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let mut effective_registry = registry_base.to_string();
    // A mirror 404 gets exactly one retry against the official registry
    // before any prune decision — a lagging mirror must not read as "the
    // package is gone".
    let mut fallback_pending = registry_base.trim_end_matches('/') != OFFICIAL_NPM_REGISTRY;

    for attempt in 0..12_u8 {
        match run_npm_install(
            node_program,
            npm_cli,
            version_dir,
            &effective_registry,
            flag,
            on_progress,
        )
        .await
        {
            Ok(()) => return Ok(std::mem::take(skipped)),
            Err(e) if e == "cancelled" => return Err("cancelled".into()),
            Err(stderr) => {
                let Some(pkg) = parse_404_package(&stderr) else {
                    let tail = stderr
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .rev()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" | ");
                    // A failed solve must not leave a tree that reads as
                    // installed (the launch probe only checks node_modules).
                    let _ = tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                    return Err(if tail.is_empty() {
                        format!("npm install 失败: {stderr}")
                    } else {
                        format!("npm install 失败: {tail}")
                    });
                };
                let using_official = effective_registry == OFFICIAL_NPM_REGISTRY;
                match decide_not_found(&pkg, fallback_pending, using_official) {
                    NotFoundAction::RetryOfficial => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 在 {effective_registry} 上 404，改用官方 registry 重试"
                        );
                        effective_registry = OFFICIAL_NPM_REGISTRY.to_string();
                        fallback_pending = false;
                        continue;
                    }
                    NotFoundAction::Prune => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 官方 registry 也 404，属于可跳过的实验性依赖，从清单中移除后重试"
                        );
                        if !remove_dep_from_manifest(version_dir, &pkg) {
                            let _ =
                                tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                            return Err(format!(
                                "依赖 {pkg} 在 registry 上不存在，且无法从清单中移除它"
                            ));
                        }
                        skipped.push(pkg);
                        if attempt == 11 {
                            let _ =
                                tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                            return Err("跳过的依赖过多，安装中止".into());
                        }
                    }
                    NotFoundAction::Fatal => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 官方 registry 也 404，且不属于可跳过的实验性依赖"
                        );
                        let _ = tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                        return Err(format!(
                            "依赖 {pkg} 在 registry 上不存在，且不属于可跳过的实验性依赖，安装中止"
                        ));
                    }
                }
            }
        }
    }
    unreachable!("the loop always returns")
}

/// One `npm install` child. The returned Err is the raw stderr text when the
/// solve failed (the retry loop parses 404s out of it) or "cancelled".
///
/// stderr is *drained while waiting*: npm's error output for a 70-dependency
/// tree is far over the ~64 KB pipe buffer, and a piped-stderr child whose
/// pipe nobody reads blocks mid-write — the wait never returns.
async fn run_npm_install(
    node_program: &Path,
    npm_cli: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: &(dyn Fn(f64) + Send + Sync),
) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::new(node_program);
    command
        .arg(npm_cli)
        .args([
            "install",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
        ])
        .arg("--prefix")
        .arg(version_dir)
        .arg("--registry")
        .arg(registry_base.trim_end_matches('/'))
        .current_dir(version_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(crate::launch::CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|e| format!("无法运行 npm: {e}"))?;
    let mut stderr_pipe = child.stderr.take().ok_or("无法读取 npm 输出")?;
    let started = Instant::now();
    let mut err_text = String::new();
    let mut buf = [0u8; 8192];
    let mut child_status: Option<std::process::ExitStatus> = None;
    let mut cancelled = false;

    // A 250 ms tick drives both the cancel poll and the progress ramp; the
    // stderr and wait arms do the rest. (A `select!` guard would be evaluated
    // once per arm and never re-checked, so cancel lives in the tick arm.)
    loop {
        tokio::select! {
            chunk = stderr_pipe.read(&mut buf) => {
                match chunk {
                    Ok(0) => {}
                    Ok(n) => {
                        err_text.push_str(&String::from_utf8_lossy(&buf[..n]));
                        // npm logs can grow past the pipe into MBs; the tail
                        // is all the retry loop and the error toast ever need.
                        if err_text.len() > 64 * 1024 {
                            // Walk forward to a char boundary instead of
                            // slicing at the raw offset: `err_text[..cut]`
                            // panics outright when `cut` lands inside a
                            // multi-byte character, which any non-ASCII npm
                            // output (a CN mirror's messages, box drawing)
                            // makes likely once the tail trim kicks in.
                            let mut start = err_text.len() - 32 * 1024;
                            while start < err_text.len() && !err_text.is_char_boundary(start) {
                                start += 1;
                            }
                            err_text.replace_range(..start, "");
                        }
                    }
                    Err(_) => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancelled_flag(flag) {
                    if let Some(pid) = child.id() {
                        let _ = crate::launch::kill_tree(pid).await;
                    }
                    cancelled = true;
                    break;
                }
                let elapsed = started.elapsed().as_secs_f64();
                on_progress(0.95_f64.min(elapsed / (elapsed + 60.0) * 1.9));
            }
            status = child.wait() => {
                child_status = status.ok();
                break;
            }
        }
    }

    if cancelled {
        return Err("cancelled".into());
    }

    // The child is gone, so the pipe drains from buffers and hits EOF at
    // once — the last few stderr lines still belong to the failure message.
    while let Ok(n) = stderr_pipe.read(&mut buf).await {
        if n == 0 {
            break;
        }
        err_text.push_str(&String::from_utf8_lossy(&buf[..n]));
    }

    let status = match child_status {
        Some(s) => s,
        None => child
            .wait()
            .await
            .map_err(|e| format!("无法等待 npm 退出: {e}"))?,
    };
    if status.success() {
        Ok(())
    } else if err_text.trim().is_empty() {
        Err(format!(
            "npm install 失败 (code {})",
            status.code().unwrap_or(-1)
        ))
    } else {
        Err(err_text)
    }
}

fn cancelled_flag(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

/* ------------------------------ install ------------------------------ */

#[derive(Deserialize)]
// The marker on disk is written as `{"installedAt": …}` — without the rename,
// deserialising a *completed install* failed and every version looked
// uninstalled after a restart.
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallMarker {
    installed_at: String,
    /// Written since the transactional installer; `default` keeps markers
    /// from before that era readable.
    #[serde(default)]
    version: String,
}

pub(crate) async fn read_marker(version_dir: &Path) -> Option<InstallMarker> {
    let raw = tokio::fs::read_to_string(version_dir.join("phl-install.json"))
        .await
        .ok()?;
    serde_json::from_str::<InstallMarker>(&raw).ok()
}

pub(crate) fn sanitize_version(name: &str) -> Result<String, String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
        // `.` is an allowed character, so both of these otherwise pass the
        // whitelist — and `<root>/versions/..` resolves to the data root,
        // which `remove_version_dir` would then `remove_dir_all`.
        && name != "."
        && name != "..";
    if ok {
        Ok(name.to_string())
    } else {
        Err(format!("非法的版本号: {name}"))
    }
}

/// The transactional swap both installers share: the fully staged tree
/// replaces `dest`, the previous tree waits in `backup` until the final
/// check passes, and any failure puts the previous tree back. `staging` and
/// `dest` must be on the same drive — both installers stage under the
/// parent of `dest` to guarantee that.
///
/// This is the whole transaction vocabulary in one place: stage, backup,
/// commit, verify, rollback, cleanup — deliberately not a generic framework.
pub(crate) async fn promote_staged(
    staging: &Path,
    dest: &Path,
    backup: &Path,
    final_check: &(dyn Fn(&Path) -> Result<(), String> + Send + Sync),
) -> Result<(), String> {
    let had_previous = dest.exists();
    if had_previous {
        let _ = tokio::fs::remove_dir_all(backup).await;
        if let Err(e) = tokio::fs::rename(dest, backup).await {
            let _ = tokio::fs::remove_dir_all(staging).await;
            return Err(format!("无法备份现有安装，版本未更新: {e}"));
        }
    }
    if let Err(e) = tokio::fs::rename(staging, dest).await {
        if had_previous {
            let _ = tokio::fs::rename(backup, dest).await;
        }
        let _ = tokio::fs::remove_dir_all(staging).await;
        return Err(format!("无法放置新版本: {e}"));
    }
    if let Err(e) = final_check(dest) {
        // The new tree is in place but wrong; the backup is the only copy of
        // what worked before, so it goes back before anything else happens.
        let _ = tokio::fs::remove_dir_all(dest).await;
        if had_previous {
            let _ = tokio::fs::rename(backup, dest).await;
        }
        return Err(format!("最终校验失败，已恢复原版本: {e}"));
    }
    // Success: the backup is now redundant space, not a rollback point —
    // the staging dir is gone (renamed), so nothing references it.
    let _ = tokio::fs::remove_dir_all(backup).await;
    Ok(())
}

/// One transaction-scoped directory name: `<parent>/.phl-txn/<name>.<role>-<token>`.
/// A failed or cancelled attempt can only ever leave a `.phl-txn` child
/// behind, never something that reads as an installed version or runtime.
pub(crate) fn txn_dir(parent: &Path, name: &str, role: &str, token: u128) -> PathBuf {
    parent
        .join(".phl-txn")
        .join(format!("{name}.{role}-{token}"))
}

pub(crate) fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Minimal health check a staged version must pass before it may replace a
/// good install: manifest present, DSH entrypoint present, dependencies
/// materialised when the manifest requires them, and the marker naming this
/// exact version.
fn check_version_health(dir: &Path, version_name: &str) -> Result<(), String> {
    if !dir.join("package.json").exists() {
        return Err("package.json 缺失".into());
    }
    if !dir.join("lib").join("bin.js").exists() {
        return Err("DSH 入口 lib/bin.js 缺失".into());
    }
    if package_requires_deps(dir) && !dir.join("node_modules").exists() {
        return Err("依赖目录 node_modules 缺失".into());
    }
    let raw = std::fs::read_to_string(dir.join("phl-install.json"))
        .map_err(|e| format!("安装标记读取失败: {e}"))?;
    let marker: InstallMarker =
        serde_json::from_str(&raw).map_err(|e| format!("安装标记解析失败: {e}"))?;
    if marker.version != version_name {
        let found = if marker.version.is_empty() {
            "<空>".to_string()
        } else {
            marker.version
        };
        return Err(format!("安装标记版本 {found} 与目标 {version_name} 不一致"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_install(
    flag: &Arc<AtomicBool>,
    tarball_url: &str,
    integrity: Option<&str>,
    version_name: &str,
    root: &Path,
    registry_base: &str,
    keep_archive: bool,
    total_hint: Option<u64>,
    on_progress: &Channel<ProgressEvent>,
) -> Result<(), String> {
    let version_name = sanitize_version(version_name)?;
    if cancelled(flag) {
        return Err("cancelled".into());
    }

    let versions_dir = root.join("versions");
    let cache_dir = root.join("cache");
    let dest = versions_dir.join(&version_name);
    let archive_path = cache_dir.join(format!("dsh-{version_name}.tgz"));
    let part_path = cache_dir.join(format!("dsh-{version_name}.tgz.part"));
    let token = now_millis();
    let staging = txn_dir(&versions_dir, &version_name, "staging", token);
    let backup = txn_dir(&versions_dir, &version_name, "backup", token);
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(staging.parent().expect("txn parent"))
        .await
        .map_err(|e| e.to_string())?;

    // The existing install is never touched until a verified replacement is
    // fully staged: a download, integrity, extraction or dependency failure
    // must leave the old version exactly where it was.
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let downloaded = download(
        flag,
        tarball_url,
        &part_path,
        total_hint,
        &|progress, bytes_done, bytes_per_sec| {
            let _ = on_progress.send(ProgressEvent::Downloading {
                progress,
                bytes_done,
                bytes_per_sec,
            });
        },
    )
    .await?;

    on_progress
        .send(ProgressEvent::Verifying)
        .map_err(|e| e.to_string())?;
    if let Err(e) = verify_integrity(&downloaded.sha512, integrity) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    on_progress
        .send(ProgressEvent::Extracting { progress: 0.0 })
        .map_err(|e| e.to_string())?;
    if let Err(e) = extract(
        &part_path,
        &staging,
        &|progress| {
            let _ = on_progress.send(ProgressEvent::Extracting { progress });
        },
        flag,
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    // The tarball is only the package itself — pull its declared deps into
    // the staging tree's own node_modules now, while the user is watching an
    // install. A version that boots into a surprise 30-second npm run would
    // be the worse failure mode.
    if package_requires_deps(&staging) {
        on_progress
            .send(ProgressEvent::Verifying)
            .map_err(|e| e.to_string())?;
        let node = pick_npm_capable_node(root).unwrap_or_else(|| PathBuf::from("node"));
        if let Err(e) = install_version_deps(&node, &staging, registry_base, flag, |_| {}).await {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(if e == "cancelled" {
                e
            } else {
                format!("已下载但依赖安装失败，版本未安装: {e}")
            });
        }
    }

    // The marker is written last into staging: its presence is what makes a
    // directory read as a completed install anywhere on disk.
    let marker = serde_json::json!({
        "installedAt": now_iso(),
        "version": version_name,
        "tarball": tarball_url,
        "integrity": integrity.unwrap_or(""),
        "bytes": downloaded.bytes,
    });
    if let Err(e) = tokio::fs::write(staging.join("phl-install.json"), marker.to_string()).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e.to_string());
    }

    on_progress
        .send(ProgressEvent::Verifying)
        .map_err(|e| e.to_string())?;
    if let Err(e) = check_version_health(&staging, &version_name) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("安装校验未通过，版本未安装: {e}"));
    }

    promote_staged(&staging, &dest, &backup, &|installed: &Path| {
        check_version_health(installed, &version_name)
    })
    .await?;

    if keep_archive {
        tokio::fs::rename(&part_path, &archive_path)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        let _ = tokio::fs::remove_file(&part_path).await;
    }
    Ok(())
}

/// A completed download: the byte count for the install marker, plus the
/// Sha512 accumulated over the stream. Hashing on the way past means the
/// integrity check never has to re-read the archive.
pub(crate) struct Downloaded {
    pub bytes: u64,
    pub sha512: Vec<u8>,
    /// npm publishes sha512, node dist publishes sha256 (SHASUMS256.txt) —
    /// one streaming pass computes both so neither verifier re-reads bytes.
    pub sha256: Vec<u8>,
}

pub(crate) async fn download<F: Fn(f64, u64, u64) + Send + Sync>(
    flag: &AtomicBool,
    url: &str,
    part_path: &Path,
    total_hint: Option<u64>,
    on_tick: &F,
) -> Result<Downloaded, String> {
    if cancelled(flag) {
        return Err("cancelled".into());
    }
    let client = http_client();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载源返回错误: {e}"))?;
    // The CDN may stream without `content-length`; the catalog's probed size
    // serves as the fallback denominator for progress.
    let total = response.content_length().or(total_hint).unwrap_or(0);

    let mut file = tokio::fs::File::create(part_path)
        .await
        .map_err(|e| format!("无法创建缓存文件: {e}"))?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha512::new();
    let mut hasher256 = Sha256::new();
    let mut bytes_done: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_bytes: u64 = 0;
    let mut last_speed: u64 = 0;

    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        if cancelled(flag) {
            let _ = file.shutdown().await;
            let _ = tokio::fs::remove_file(part_path).await;
            return Err("cancelled".into());
        }
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        hasher.update(&chunk);
        hasher256.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入缓存失败: {e}"))?;
        bytes_done += chunk.len() as u64;

        let elapsed = last_report.elapsed();
        if elapsed >= Duration::from_millis(120) {
            let delta = bytes_done - last_bytes;
            last_speed = if delta > 0 {
                ((delta as f64) / elapsed.as_secs_f64()) as u64
            } else {
                last_speed
            };
            last_report = Instant::now();
            last_bytes = bytes_done;
            let progress = if total > 0 {
                bytes_done as f64 / total as f64
            } else {
                0.0
            };
            on_tick(progress, bytes_done, last_speed);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    on_tick(1.0, bytes_done, last_speed);
    Ok(Downloaded {
        bytes: bytes_done,
        sha512: hasher.finalize().to_vec(),
        sha256: hasher256.finalize().to_vec(),
    })
}

/// Compares npm's `dist.integrity` against the digest the download already
/// computed. Taking the digest as an argument keeps the archive from being
/// read a second time — plugin tarballs are arbitrary sizes and the old
/// version loaded the whole file into memory to re-hash it.
pub(crate) fn verify_integrity(digest: &[u8], integrity: Option<&str>) -> Result<(), String> {
    let Some(expected) = integrity else {
        return Ok(()); // registry gave no integrity → nothing to check
    };
    let ok = if let Some(b64) = expected.strip_prefix("sha512-") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("integrity 字段解析失败: {e}"))?;
        decoded == digest
    } else if let Some(expected_hex) = expected.strip_prefix("sha512:") {
        expected_hex.eq_ignore_ascii_case(&hex::encode(digest))
    } else {
        // Anything we cannot check must not be waved through: npm only ever
        // publishes sha512 here, so an unfamiliar prefix means the metadata
        // is not what we think it is.
        let algo = expected.split(['-', ':']).next().unwrap_or(expected);
        return Err(format!("无法校验：不支持的摘要算法 {algo}"));
    };
    if ok {
        Ok(())
    } else {
        Err("校验失败：下载内容与官方 sha512 摘要不一致".into())
    }
}

/// Wraps the archive file so extraction can report progress from the number
/// of *compressed* bytes consumed. The previous approach counted entries up
/// front, which meant decompressing the whole archive twice.
struct CountingReader<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Resolves a tar entry's path under `dest`, refusing anything that escapes.
///
/// `Path::starts_with` compares components without resolving `..`, so
/// `dest.join("../../x")` still "starts with" `dest` and passes a prefix
/// check — while the OS happily resolves it at `File::create` time. The
/// components have to be rejected themselves.
pub(crate) fn safe_join(dest: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut out = dest.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("压缩包包含越界路径，已中止".into())
            }
        }
    }
    Ok(out)
}

pub(crate) async fn extract<F: Fn(f64) + Send + Sync>(
    archive_path: &Path,
    dest: &Path,
    on_tick: &F,
    flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| e.to_string())?;
    let compressed = tokio::fs::metadata(archive_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // The tar jobs are blocking; run them on a worker thread and report
    // progress through a channel the async loop can forward.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<f64, String>>(16);
    let path = archive_path.to_path_buf();
    let dest_clone = dest.to_path_buf();
    let flag_worker = Arc::clone(flag);
    std::thread::spawn(move || {
        let consumed = Arc::new(AtomicU64::new(0));
        let result = (|| -> Result<(), String> {
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let counted = CountingReader {
                inner: file,
                read: Arc::clone(&consumed),
            };
            let mut archive = tar::Archive::new(GzDecoder::new(counted));
            let entries = archive.entries().map_err(|e| e.to_string())?;
            for entry in entries {
                // The cancel flag has to be read *here*, not only by the
                // async receiver: without it the thread kept unpacking after
                // the caller had already started deleting the directory.
                if flag_worker.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                let mut entry = entry.map_err(|e| e.to_string())?;
                let relative = strip_first(&entry.path().map_err(|e| e.to_string())?)
                    .ok_or("压缩包内没有预期的 package/ 前缀")?;
                if relative.as_os_str().is_empty() {
                    continue; // the prefix directory entry itself
                }
                let target = safe_join(&dest_clone, &relative)?;
                if entry.header().entry_type().is_dir() {
                    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                } else {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
                }
                let progress = if compressed > 0 {
                    (consumed.load(Ordering::Relaxed) as f64 / compressed as f64).min(1.0)
                } else {
                    0.0
                };
                // A closed channel means the caller gave up; stop rather
                // than keep writing into a directory it is cleaning up.
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

/// npm tarballs nest everything under `package/`; drop that single prefix.
/// GitHub codeload tarballs have a `repo-<sha>/` prefix that strips the same
/// way, so the plugin installer reuses this too.
pub(crate) fn strip_first(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    components.next()?;
    Some(components.as_path().to_path_buf())
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
        let dir = std::env::temp_dir().join(format!("phl-test-{}", std::process::id()));
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
}
