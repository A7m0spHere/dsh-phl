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
            if sha.len() >= 40 {
                return Some(sha[..40].to_string());
            }
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;

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
