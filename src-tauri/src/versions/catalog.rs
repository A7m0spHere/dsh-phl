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
            // maintenance line — treat it as legacy. Anchored on the newest
            // *published* version rather than on the badge's winner: a release
            // npm has not taken yet must not mark every installable version
            // legacy, which is what the wizard's default pick reads.
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
                latest: false, // flagged once every source has joined (mark_latest)
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
    // join the list as read-only rows so "GitHub is ahead" is visible. The
    // legacy flag stays false — there is no older line to retire until the
    // release is installable — but the *badge* is not withheld: a release the
    // team has cut is the newest version, and `pending_publish` is what says
    // it cannot be installed yet. Deciding the badge here instead of at npm
    // publish time is the 2026-09-23 fix; see `mark_latest`.
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
            latest: false, // mark_latest flags the winner across all sources
            legacy: false,
            pending_publish: true,
            source: None,
        });
    }

    // One badge decision, over every source. It has to run here, after the
    // GitHub-ahead rows have joined: see `mark_latest`.
    mark_latest(&mut out);

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
    // Shape of the packument; the badge intentionally does NOT follow
    // dist-tags.latest (see `mark_latest`), so nothing consumes it now.
    #[allow(dead_code)]
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
        out.push((
            ver,
            NpmEntry {
                channel,
                published_at,
                size,
                requires_node,
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
    let newest = newest_semver(&out);
    Ok((out, newest))
}

/// The newest version the registry holds — the anchor the legacy-line check
/// compares against. It used to hand the「最新」badge back as well; that job
/// moved to `mark_latest` over the merged catalog, and this is what is left of
/// the npm-side rule.
pub(crate) fn newest_semver(out: &[(String, NpmEntry)]) -> Option<semver::Version> {
    out.iter()
        .filter_map(|(_, entry)| entry.semver.clone())
        .max()
}

/// The「最新」badge marks the single newest release in the merged catalog —
/// npm rows and GitHub-only rows alike, by semver, never npm's `latest`
/// dist-tag. Names that do not parse never take the badge.
///
/// Two reports shaped this rule. 2026-09-11: the dist-tag kept the badge on
/// 0.1.5-rc.1 after rc.2 shipped under `next`, so the badge has followed the
/// highest semver since. 2026-09-23: it *still* lagged, because the rule ran
/// over the npm list alone — upstream had cut 0.1.7-alpha.2 on GitHub while
/// npm was still at 0.1.7-alpha.1, so the newest row rendered as「npm 未收录」
/// with the badge one line below it. What the badge states is *which release
/// is newest*; whether that release is installable yet is `pending_publish`'s
/// job, and the list shows both side by side.
fn mark_latest(out: &mut [DshVersionMeta]) {
    let newest = out
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| parse_semver(&entry.name).map(|sem| (i, sem)))
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(i, _)| i);
    for (i, entry) in out.iter_mut().enumerate() {
        entry.latest = Some(i) == newest;
    }
}

#[cfg(test)]
mod latest_badge_tests {
    use super::*;

    /// A merged-catalog row. `latest` is pre-set exactly as the list builders
    /// leave it (false everywhere) so a test can only pass if `mark_latest`
    /// itself flags the winner.
    fn row(name: &str, pending: bool) -> DshVersionMeta {
        DshVersionMeta {
            id: format!("dsh-{name}"),
            name: name.to_string(),
            channel: "rc".into(),
            released_at: "2026-09-22T00:00:00.000Z".into(),
            size: if pending { 0 } else { 1 },
            requires_node: Vec::new(),
            notes: Vec::new(),
            latest: false,
            legacy: false,
            pending_publish: pending,
            source: None,
        }
    }

    fn flagged(rows: &[DshVersionMeta]) -> Vec<String> {
        rows.iter()
            .filter(|r| r.latest)
            .map(|r| r.name.clone())
            .collect()
    }

    fn nentry(v: &str) -> (String, NpmEntry) {
        (
            v.to_string(),
            NpmEntry {
                channel: "rc".into(),
                published_at: "2026-09-10T00:00:00.000Z".into(),
                size: 1,
                requires_node: vec![],
                tarball: format!("https://registry/{v}.tgz"),
                integrity: None,
                semver: parse_semver(v),
            },
        )
    }

    #[test]
    fn badge_follows_highest_version_not_the_dist_tag() {
        // The 2026-09-11 report: upstream dist-tags.latest stayed on rc.1
        // while rc.2 published under `next`. The badge must move to rc.2 —
        // what the sorted list shows first.
        let mut out = vec![
            row("0.1.5-rc.1", false),
            row("0.1.5-rc.2", false),
            row("0.1.3-alpha.2", false),
        ];
        mark_latest(&mut out);
        assert_eq!(flagged(&out), vec!["0.1.5-rc.2"]);
    }

    #[test]
    fn a_release_only_on_github_still_takes_the_badge() {
        // The 2026-09-23 report, with the day's real shape: npm's newest was
        // 0.1.7-alpha.1, GitHub had already cut 0.1.7-alpha.2, and the badge
        // was decided over the npm list alone — so the newest row rendered as
        //「npm 未收录」with the badge one line below it. The newest release
        // carries the badge whether or not npm has caught up; `pending_publish`
        // is the badge that speaks to installability.
        let mut out = vec![
            row("0.1.7-alpha.2", true),
            row("0.1.7-alpha.1", false),
            row("0.1.6-alpha.2", false),
        ];
        mark_latest(&mut out);
        assert_eq!(flagged(&out), vec!["0.1.7-alpha.2"]);
        assert!(out[0].pending_publish, "the winner is still uninstallable");
        assert!(out[0].source.is_none());
    }

    #[test]
    fn stable_outranks_its_own_prereleases() {
        let mut out = vec![
            row("0.1.5-rc.2", false),
            row("0.1.4", false),
            row("0.1.5", false),
        ];
        mark_latest(&mut out);
        assert_eq!(flagged(&out), vec!["0.1.5"]);
    }

    #[test]
    fn unparsable_and_empty_catalogs_yield_no_badge() {
        let mut out = vec![row("not-a-version", false), row("0.1.0-rc.3", false)];
        mark_latest(&mut out);
        assert_eq!(flagged(&out), vec!["0.1.0-rc.3"]);

        // A stale flag must not survive a catalog with nothing to flag.
        let mut stale = vec![row("not-a-version", false)];
        stale[0].latest = true;
        mark_latest(&mut stale);
        assert!(flagged(&stale).is_empty());

        let mut empty: Vec<DshVersionMeta> = Vec::new();
        mark_latest(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn newest_semver_anchors_the_legacy_line() {
        let out = vec![nentry("0.1.5-rc.1"), nentry("0.1.5-rc.2"), nentry("bad")];
        assert_eq!(
            newest_semver(&out).map(|s| s.to_string()).as_deref(),
            Some("0.1.5-rc.2")
        );
        assert!(newest_semver(&[]).is_none());
    }
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
