//! Plugin supply-chain trust (T-107/T-108): trust levels derived only from
//! install-time facts, and GitHub HEAD → commit SHA pinning.

use super::resolve::ResolvedSource;
use super::{http_client, PluginSourceWire};

/// Trust levels (T-107), derived only from install-time facts:
///
/// - `verified` — npm exact version whose registry `dist.integrity` matched
///   the downloaded bytes.
/// - `pinned` — the content is fixed to something immutable: a tarball with
///   a declared checksum, or a GitHub source resolved to a commit SHA.
/// - `unverified` — whatever the remote serves right now (GitHub HEAD,
///   checksum-less tarball). Installs carry a warning and updates re-warn.
pub(crate) fn compute_trust(source: &PluginSourceWire, resolved: &ResolvedSource) -> &'static str {
    match source {
        PluginSourceWire::Npm { .. } => {
            if resolved.integrity.is_some() {
                "verified"
            } else {
                "unverified"
            }
        }
        PluginSourceWire::Tarball { .. } => {
            if resolved.integrity.is_some() {
                "pinned"
            } else {
                "unverified"
            }
        }
        PluginSourceWire::Github { .. } => {
            if resolved.commit.is_some() {
                "pinned"
            } else {
                "unverified"
            }
        }
    }
}

/// Resolves a GitHub repo's HEAD to its current commit SHA so the install
/// downloads an immutable tarball instead of a moving target. Best-effort:
/// every failure (offline, rate limit, proxy down) degrades to HEAD, which
/// the trust model then labels unverified rather than failing the install.
pub(crate) async fn resolve_commit(repo: &str) -> Option<String> {
    const MIRRORS: &[&str] = &["https://gh-proxy.com/"];
    let api = format!("https://api.github.com/repos/{repo}/commits/HEAD");
    let mut urls: Vec<String> = MIRRORS.iter().map(|m| format!("{m}{api}")).collect();
    urls.push(api);
    for url in urls {
        let response = http_client()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;
        let value: serde_json::Value = response.json().await.ok()?;
        if let Some(sha) = value.get("sha").and_then(|s| s.as_str()) {
            if let Some(sha) = normalize_commit_sha(sha) {
                return Some(sha);
            }
        }
    }
    None
}

/// The 40-hex commit SHA a `sha` field must hold before an install pins to
/// it. Both guards matter: the first candidate source is a third-party proxy
/// that can hand back any bytes, and slicing `raw[..40]` unchecked panics on
/// a multi-byte char boundary — remote data must never be able to crash the
/// install path (2026-09-29 review).
pub(crate) fn normalize_commit_sha(raw: &str) -> Option<String> {
    let taken: String = raw.chars().take(40).collect();
    (taken.len() == 40 && taken.bytes().all(|b| b.is_ascii_hexdigit())).then_some(taken)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sha_field_that_cannot_be_a_commit_is_refused_not_sliced() {
        let hex40 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            normalize_commit_sha(hex40).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        // Longer output keeps the first 40, like the old slice did — safely.
        assert_eq!(
            normalize_commit_sha(&format!("{hex40}ffff")).as_deref(),
            Some(hex40)
        );
        // Short, non-hex, and multi-byte shapes are all just "no SHA".
        assert_eq!(normalize_commit_sha("0123456789"), None);
        assert_eq!(normalize_commit_sha(&"g".repeat(40)), None);
        // 39 ASCII bytes + one multi-byte char: the old `raw[..40]` panicked
        // on exactly this shape.
        let poisoned = format!("{}中", "a".repeat(39));
        assert_eq!(normalize_commit_sha(&poisoned), None);
    }

    fn resolved(integrity: Option<&str>, commit: Option<String>) -> ResolvedSource {
        ResolvedSource {
            url: String::new(),
            integrity: integrity.map(Into::into),
            version: String::new(),
            commit,
        }
    }

    #[test]
    fn trust_is_derived_from_source_facts_not_optimism() {
        let npm = resolved(Some("sha512-abc"), None);
        let npm_bare = resolved(None, None);
        let tarball = resolved(Some("sha512-abc"), None);
        let tarball_bare = resolved(None, None);
        let gh_head = resolved(None, None);
        let gh_pinned = resolved(None, Some("a".repeat(40)));

        assert_eq!(
            compute_trust(&PluginSourceWire::Npm { pkg: "x".into() }, &npm),
            "verified"
        );
        assert_eq!(
            compute_trust(&PluginSourceWire::Npm { pkg: "x".into() }, &npm_bare),
            "unverified",
            "npm without integrity cannot be called verified"
        );
        assert_eq!(
            compute_trust(
                &PluginSourceWire::Tarball {
                    url: String::new(),
                    integrity: None
                },
                &tarball
            ),
            "pinned"
        );
        assert_eq!(
            compute_trust(
                &PluginSourceWire::Tarball {
                    url: String::new(),
                    integrity: None
                },
                &tarball_bare
            ),
            "unverified"
        );
        assert_eq!(
            compute_trust(&PluginSourceWire::Github { repo: "a/b".into() }, &gh_pinned),
            "pinned"
        );
        assert_eq!(
            compute_trust(&PluginSourceWire::Github { repo: "a/b".into() }, &gh_head),
            "unverified"
        );
    }
}
