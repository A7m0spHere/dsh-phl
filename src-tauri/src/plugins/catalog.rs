//! The community registry: awesome-dsh-plugin `plugins.json`, its fallback
//! sources, the offline cache, and the npm dist-tag probe for updates.

use serde::Deserialize;
use tauri::State;

use super::resolve::Packument;
use super::{
    http_client, PluginCatalogWire, PluginMeta, PluginSourceWire, PluginVersionInfo, HTTP_TIMEOUT,
    REGISTRY_FALLBACKS, REGISTRY_TIMEOUT,
};
use crate::paths::PhlState;

/* ------------------------------ registry ------------------------------ */

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default)]
    plugins: Vec<RegistryEntry>,
    /// The registry's own freshness stamp. Carried to the UI: a mirror that
    /// stopped syncing still serves a *parseable* file, so `updated` is the
    /// only signal that the catalog is stale.
    #[serde(default)]
    updated: Option<String>,
}

#[derive(Deserialize)]
struct RegistryEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    description: RegistryDescription,
    #[serde(default)]
    npm: Option<String>,
    #[serde(default)]
    tarball: Option<String>,
    #[serde(default)]
    stars: Option<u64>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    added: Option<String>,
    #[serde(default)]
    screenshots: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RegistryDescription {
    #[serde(default)]
    en: Option<String>,
    #[serde(default)]
    zh: Option<String>,
}

fn map_registry_entry(entry: RegistryEntry) -> Option<PluginMeta> {
    let repo = if entry.url.is_empty() {
        if entry.owner.is_empty() || entry.name.is_empty() {
            return None;
        }
        format!("{}/{}", entry.owner, entry.name)
    } else {
        entry
            .url
            .trim_start_matches("https://github.com/")
            .trim_start_matches("http://github.com/")
            .trim_matches('/')
            .to_string()
    };

    let source = match (&entry.npm, &entry.tarball) {
        (Some(pkg), _) if !pkg.is_empty() => PluginSourceWire::Npm { pkg: pkg.clone() },
        (None, Some(url)) if !url.is_empty() => PluginSourceWire::Tarball {
            url: url.clone(),
            integrity: None,
        },
        _ => PluginSourceWire::Github { repo: repo.clone() },
    };

    let summary = entry
        .description
        .zh
        .clone()
        .or_else(|| entry.description.en.clone())
        .unwrap_or_default();
    let official = entry.owner.eq_ignore_ascii_case("deepseek-ai")
        || entry
            .npm
            .as_deref()
            .unwrap_or("")
            .starts_with("@deepseek-ai/");
    let name = if entry.name.is_empty() {
        repo.rsplit('/').next().unwrap_or(&repo).to_string()
    } else {
        entry.name
    };

    Some(PluginMeta {
        id: repo.clone(),
        name,
        author: if entry.owner.is_empty() {
            repo.split('/').next().unwrap_or("unknown").to_string()
        } else {
            entry.owner
        },
        // Unknown categories degrade to `dev` instead of being dropped.
        category: if entry.category.is_empty() {
            "dev".into()
        } else {
            entry.category
        },
        summary,
        summary_en: entry.description.en,
        repo_url: Some(format!("https://github.com/{repo}")),
        screenshots: entry.screenshots,
        source,
        official,
        downloads: entry.downloads.unwrap_or(0),
        stars: entry.stars,
        added_at: entry.added,
        releases: Vec::new(),
    })
}

async fn fetch_registry_text(base: &str) -> Result<String, String> {
    let url = format!("{}/plugins.json", base.trim_end_matches('/'));
    let response = http_client()
        .get(&url)
        .timeout(REGISTRY_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("注册表请求失败 ({url}): {e}"))?
        .error_for_status()
        .map_err(|e| format!("注册表返回错误 ({url}): {e}"))?;
    response
        .text()
        .await
        .map_err(|e| format!("注册表下载中断 ({url}): {e}"))
}

fn parse_registry(text: &str) -> Result<RegistryFile, String> {
    serde_json::from_str(text).map_err(|e| format!("注册表 JSON 解析失败: {e}"))
}

fn map_registry_file(mut file: RegistryFile) -> Vec<PluginMeta> {
    let mut out: Vec<PluginMeta> = file
        .plugins
        .drain(..)
        .filter_map(map_registry_entry)
        .collect();
    out.sort_by(|a, b| b.downloads.cmp(&a.downloads).then(a.id.cmp(&b.id)));
    out
}

/// The fetch order for a user-configured base: their source first (its intent
/// is respected even when it is a lagging mirror — the reply still says which
/// base served), then every fallback that isn't already in position 1.
fn candidate_bases(preferred: &str) -> Vec<String> {
    let mut bases: Vec<String> = Vec::new();
    if !preferred.is_empty() {
        bases.push(preferred.to_string());
    }
    for fallback in REGISTRY_FALLBACKS {
        if *fallback != preferred {
            bases.push((*fallback).to_string());
        }
    }
    bases
}

#[tauri::command]
pub async fn list_dsh_plugins(
    registry_base: String,
    phl: State<'_, PhlState>,
) -> Result<PluginCatalogWire, String> {
    // The offline copy lives in the data-root cache, like every other
    // download the app keeps. Resolved here rather than passed in: the
    // frontend has no business knowing where PHL stores data.
    let cache_dir = phl.root().join("cache");
    let preferred = registry_base.trim().trim_end_matches('/').to_string();
    let bases = candidate_bases(&preferred);

    let mut last_err = String::from("没有可用的注册表源");
    for base in &bases {
        match fetch_registry_text(base).await {
            Ok(text) => match parse_registry(&text) {
                Ok(file) => {
                    let updated = file.updated.clone();
                    // Stash the raw payload so the next completely-offline
                    // launch still has a real catalog to show.
                    {
                        let path = cache_dir.join("plugin-catalog.json");
                        let _ = tokio::fs::create_dir_all(&cache_dir).await;
                        let _ = tokio::fs::write(&path, &text).await;
                    }
                    return Ok(PluginCatalogWire {
                        served_from: base.clone(),
                        used_fallback: !preferred.is_empty() && base != &preferred,
                        from_cache: false,
                        updated,
                        plugins: map_registry_file(file),
                    });
                }
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }
    }

    // Every online source failed — serve the last successful fetch, if any.
    {
        let path = cache_dir.join("plugin-catalog.json");
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            if let Ok(file) = parse_registry(&text) {
                let updated = file.updated.clone();
                eprintln!("[phl] registry unreachable ({last_err}); serving cached catalog");
                return Ok(PluginCatalogWire {
                    served_from: "cache".into(),
                    used_fallback: true,
                    from_cache: true,
                    updated,
                    plugins: map_registry_file(file),
                });
            }
        }
    }
    Err(last_err)
}

#[tauri::command]
pub async fn plugin_latest_version(
    source: PluginSourceWire,
    registry_base: String,
) -> Result<PluginVersionInfo, String> {
    let PluginSourceWire::Npm { pkg } = source else {
        return Ok(PluginVersionInfo { version: None });
    };
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
    Ok(PluginVersionInfo {
        version: packument.dist_tags.get("latest").cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_bases_respects_the_configured_source_and_dedupes() {
        // The configured source leads even when it lags (the reply carries
        // the provenance), and never appears twice from the fallback list.
        let official = "https://awesome-dsh-plugin.com";
        assert_eq!(
            candidate_bases(official),
            vec![
                official.to_string(),
                "https://awesome-dsh-plugin.github.io/awesome-dsh-plugin".to_string(),
                "https://dsh-ai.org".to_string(),
            ]
        );
        // A lagging mirror still leads when the user chose it — but the fresh
        // sources follow immediately behind it.
        let mirror = candidate_bases("https://dsh-ai.org");
        assert_eq!(mirror[0], "https://dsh-ai.org");
        assert_eq!(mirror.len(), 3);
        assert_eq!(mirror[1], official);
        // Empty preferred and custom bases.
        assert_eq!(candidate_bases("").len(), 3);
        let custom = candidate_bases("https://mirror.example/");
        assert_eq!(custom.len(), 4);
        assert_eq!(custom[0], "https://mirror.example/");
    }

    #[test]
    fn parse_registry_surfaces_the_updated_stamp() {
        let text = r#"{
            "updated": "2026-09-05",
            "plugins": [
                {"name": "p1", "owner": "o", "url": "https://github.com/o/p1", "npm": "@o/p1"},
                {"description": {"en": "nothing identifying"}}
            ]
        }"#;
        let file = parse_registry(text).expect("parses");
        assert_eq!(file.updated.as_deref(), Some("2026-09-05"));
        let mapped = map_registry_file(file);
        // The entry with no url and no owner+name is dropped, not invented.
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].id, "o/p1");
        assert!(matches!(&mapped[0].source, PluginSourceWire::Npm { .. }));
    }

    #[test]
    fn registry_without_an_updated_stamp_stays_none() {
        let file = parse_registry(r#"{"plugins": []}"#).expect("parses");
        assert_eq!(file.updated, None);
    }
}
