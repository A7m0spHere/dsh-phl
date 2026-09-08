//! Source resolution: a catalog source (npm / tarball / GitHub) becomes a
//! concrete tarball URL + expected integrity + resolved version. GitHub
//! sources pin to a commit SHA when the API answers (see `security`).

use serde::Deserialize;
use tauri::ipc::Channel;

use super::security::resolve_commit;
use super::{http_client, PluginProgressEvent, PluginSourceWire, HTTP_TIMEOUT};

/* --------------------------- npm resolution --------------------------- */

#[derive(Deserialize)]
pub(crate) struct Packument {
    #[serde(rename = "dist-tags", default)]
    pub(crate) dist_tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    versions: std::collections::HashMap<String, PackumentVersion>,
}

#[derive(Deserialize)]
pub(crate) struct PackumentVersion {
    #[serde(default)]
    dist: NpmDist,
}

#[derive(Deserialize, Default)]
pub(crate) struct NpmDist {
    #[serde(default)]
    tarball: String,
    #[serde(default)]
    integrity: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ResolvedSource {
    pub(crate) url: String,
    pub(crate) integrity: Option<String>,
    pub(crate) version: String,
    /// The commit SHA a GitHub source was pinned to, when resolution
    /// succeeded. `None` means the content of this install is whatever the
    /// remote served at fetch time — the defining property of an unverified
    /// source.
    pub(crate) commit: Option<String>,
}

pub(crate) async fn resolve_source(
    source: &PluginSourceWire,
    requested_version: Option<&str>,
    registry_base: &str,
    on_progress: &Channel<PluginProgressEvent>,
) -> Result<ResolvedSource, String> {
    let _ = on_progress.send(PluginProgressEvent::Preparing);
    match source {
        PluginSourceWire::Npm { pkg } => {
            let encoded = pkg.replace('/', "%2F");
            let url = format!("{}/{}", registry_base.trim_end_matches('/'), encoded);
            let packument: Packument = http_client()
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

            // An explicit version is a pin, not a hint. Falling through to
            // `latest` when the registry did not have it installed something
            // the user never chose and then recorded it as if they had —
            // silently, and most likely on a mirror that had simply not
            // synced yet.
            let version = match requested_version {
                Some(want) => {
                    if !packument.versions.contains_key(want) {
                        return Err(format!("npm 上没有找到 {pkg}@{want}（该源可能尚未同步）"));
                    }
                    want.to_string()
                }
                None => packument
                    .dist_tags
                    .get("latest")
                    .cloned()
                    .ok_or_else(|| format!("npm 上没有找到 {pkg} 的可用版本"))?,
            };
            let pv = packument
                .versions
                .get(&version)
                .ok_or_else(|| format!("npm 上没有找到 {pkg}@{version}"))?;
            if pv.dist.tarball.is_empty() {
                return Err(format!("{pkg}@{version} 没有 tarball 下载地址"));
            }
            Ok(ResolvedSource {
                url: pv.dist.tarball.clone(),
                integrity: pv.dist.integrity.clone(),
                version,
                commit: None,
            })
        }
        PluginSourceWire::Tarball { url, integrity } => Ok(ResolvedSource {
            url: url.clone(),
            integrity: integrity.clone(),
            version: requested_version.unwrap_or("latest").to_string(),
            commit: None,
        }),
        PluginSourceWire::Github { repo } => {
            let repo = repo.trim_matches('/');
            if repo.split('/').count() != 2 {
                return Err(format!("非法的 GitHub 仓库: {repo}"));
            }
            // Pin to the commit whenever it can be resolved: a HEAD tarball
            // is a moving target, and an install that cannot say what bytes
            // it shipped is exactly what the trust model exists to expose.
            match resolve_commit(repo).await {
                Some(sha) => {
                    let short = &sha[..7];
                    Ok(ResolvedSource {
                        url: format!("https://codeload.github.com/{repo}/tar.gz/{sha}"),
                        integrity: None,
                        version: requested_version
                            .filter(|v| *v != "HEAD")
                            .unwrap_or(short)
                            .to_string(),
                        commit: Some(sha),
                    })
                }
                None => Ok(ResolvedSource {
                    url: format!("https://codeload.github.com/{repo}/tar.gz/HEAD"),
                    integrity: None,
                    version: requested_version.unwrap_or("HEAD").to_string(),
                    commit: None,
                }),
            }
        }
    }
}

/// `@scope/name` for npm sources, bare repo name otherwise — this is the id
/// DSH uses in `cordis.patch.yml` and the path under `node_modules`.
pub(crate) fn registry_id_of(source: &PluginSourceWire) -> String {
    match source {
        PluginSourceWire::Npm { pkg } => pkg.clone(),
        PluginSourceWire::Tarball { url, .. } => url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("plugin")
            .trim_end_matches(".tgz")
            .to_string(),
        PluginSourceWire::Github { repo } => {
            repo.rsplit('/').next().unwrap_or("plugin").to_string()
        }
    }
}

pub(crate) fn sanitize_pkg_path(name: &str) -> Result<String, String> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '-' | '_'))
        // `.` matters as much as `..`: `node_modules/.` resolves to
        // `node_modules` itself, so a registry id of "." would have made
        // uninstall wipe every plugin in the instance.
        && !name
            .split('/')
            .any(|seg| seg == ".." || seg == "." || seg.is_empty());
    if ok {
        Ok(name.to_string())
    } else {
        Err(format!("非法的插件标识: {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_pkg_path;

    #[test]
    fn registry_ids_are_whitelisted_not_normalized() {
        // The ids reach `node_modules/<id>` paths directly; anything that is
        // not a plain (optionally scoped) package name is refused outright.
        assert_eq!(sanitize_pkg_path("plain-plugin").unwrap(), "plain-plugin");
        assert_eq!(
            sanitize_pkg_path("@scope/pkg-name").unwrap(),
            "@scope/pkg-name"
        );
        assert_eq!(sanitize_pkg_path("a.b-c_d").unwrap(), "a.b-c_d");
        // Documented boundary of the current gate: a bare "@scope" is not a
        // valid npm name, but it is path-harmless inside node_modules, so
        // the whitelist (which defends *paths*) accepts it on purpose.
        assert!(sanitize_pkg_path("@scope").is_ok());

        for evil in [
            "",
            "..",
            "a/..",
            "a/../b",
            "./pkg",
            "pkg/",
            "/pkg",
            "a//b",
            "C:\\evil",
            "a\\b",
            "pkg;rm -rf",
            "x".repeat(129).as_str(),
        ] {
            assert!(sanitize_pkg_path(evil).is_err(), "must refuse {evil:?}");
        }
    }
}
