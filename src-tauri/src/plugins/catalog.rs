//! The community registry: awesome-dsh-plugin `plugins.json`, its fallback
//! sources, the offline cache, and the npm dist-tag probe for updates.

use std::path::Path;

use serde::Deserialize;

use super::resolve::Packument;
use super::{
    http_client, PluginMeta, PluginSourceWire, PluginVersionInfo, HTTP_TIMEOUT, REGISTRY_FALLBACKS,
    REGISTRY_TIMEOUT,
};

/* ------------------------------ registry ------------------------------ */

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default)]
    plugins: Vec<RegistryEntry>,
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

#[tauri::command]
pub async fn list_dsh_plugins(
    registry_base: String,
    cache_dir: Option<String>,
) -> Result<Vec<PluginMeta>, String> {
    let mut bases: Vec<String> = Vec::new();
    let base = registry_base.trim().trim_end_matches('/').to_string();
    if !base.is_empty() {
        bases.push(base.clone());
    }
    for fallback in REGISTRY_FALLBACKS {
        if *fallback != base {
            bases.push((*fallback).to_string());
        }
    }

    let mut last_err = String::from("没有可用的注册表源");
    for base in &bases {
        match fetch_registry_text(base).await {
            Ok(text) => match parse_registry(&text) {
                Ok(file) => {
                    // Stash the raw payload so the next completely-offline
                    // launch still has a real catalog to show.
                    if let Some(dir) = &cache_dir {
                        let path = Path::new(dir).join("plugin-catalog.json");
                        let _ = tokio::fs::create_dir_all(dir).await;
                        let _ = tokio::fs::write(&path, &text).await;
                    }
                    return Ok(map_registry_file(file));
                }
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }
    }

    // Every online source failed — serve the last successful fetch, if any.
    if let Some(dir) = &cache_dir {
        let path = Path::new(dir).join("plugin-catalog.json");
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            if let Ok(file) = parse_registry(&text) {
                eprintln!("[phl] registry unreachable ({last_err}); serving cached catalog");
                return Ok(map_registry_file(file));
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
