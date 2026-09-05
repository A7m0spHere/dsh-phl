//! Catalog sources: the npm packument for `@deepseek-ai/dsh` and the GitHub
//! Releases listing, merged into one version list (T-105 Batch F, split from
//! the former monolithic versions.rs).

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use super::{
    http_client, DshVersionMeta, VersionSourceMeta, DSH_PACKAGE, DSH_PACKAGE_RAW, GITHUB_RELEASES,
    HTTP_TIMEOUT,
};

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

/* --------------------------- catalog sources --------------------------- */

pub(crate) struct NpmEntry {
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
pub(crate) fn majors_from_engines(range: &str) -> Vec<u32> {
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

pub(crate) async fn npm_catalog(
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

pub(crate) struct GhInfo {
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

pub(crate) async fn github_releases(
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
