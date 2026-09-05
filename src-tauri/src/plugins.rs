//! Plugin Market backing: the DSH community catalog (awesome-dsh-plugin
//! `plugins.json`) plus a real download / verify / install pipeline.
//!
//! Source priority follows the ecosystem convention: a repo-verified npm
//! package beats an author-built tarball, which beats pulling the GitHub
//! repo source. Installation lands in the instance profile's `node_modules`
//! and registers the plugin in `cordis.patch.yml` — the same files DSH's own
//! `dsh plugin add` manages — so PHL never invents a private layout.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::paths::PhlState;
use crate::versions::{download, extract, http_client, now_iso, verify_integrity, Transfers};

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// The aggregated `plugins.json` is ~2.5 MB and the main site can take 40 s+
/// on a slow link — the default 15 s aborts mid-transfer, which is exactly
/// the "插件市场加载失败" report. Catalog fetches get their own ceiling.
const REGISTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Catalog sources, tried in order. `dsh-ai.org` serves a byte-compatible
/// copy of the same `plugins.json` (same schema, CI-refreshed) and is the
/// community's stand-in when the main domain is slow or unreachable. The
/// jsDelivr/raw entries were dropped on purpose: the repo only carries the
/// per-plugin YAML sources, never the aggregated JSON.
const REGISTRY_FALLBACKS: &[&str] = &["https://awesome-dsh-plugin.com", "https://dsh-ai.org"];

/* ----------------------------- wire types ----------------------------- */

/// Tagged exactly like the frontend `PluginSource` union.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PluginSourceWire {
    Npm {
        pkg: String,
    },
    Tarball {
        url: String,
        integrity: Option<String>,
    },
    Github {
        repo: String,
    },
}

/// Mirrors the frontend `Plugin`. Live registry entries carry no releases —
/// the concrete version resolves against npm at install time.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub author: String,
    pub category: String,
    pub summary: String,
    pub summary_en: Option<String>,
    pub repo_url: Option<String>,
    pub screenshots: Vec<String>,
    pub source: PluginSourceWire,
    pub official: bool,
    pub downloads: u64,
    pub stars: Option<u64>,
    pub added_at: Option<String>,
    pub releases: Vec<PluginReleaseMeta>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginReleaseMeta {
    pub version: String,
    pub published_at: String,
    pub source: Option<PluginSourceWire>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum PluginProgressEvent {
    Preparing,
    #[serde(rename_all = "camelCase")]
    Downloading {
        progress: f64,
        bytes_done: u64,
        bytes_per_sec: u64,
    },
    Verifying,
    #[serde(rename_all = "camelCase")]
    Installing {
        progress: f64,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallOutcome {
    pub version: String,
    /// The id written into `cordis.patch.yml` (npm package name or repo name).
    pub registry_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginVersionInfo {
    pub version: Option<String>,
}

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

/* --------------------------- npm resolution --------------------------- */

#[derive(Deserialize)]
struct Packument {
    #[serde(rename = "dist-tags", default)]
    dist_tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    versions: std::collections::HashMap<String, PackumentVersion>,
}

#[derive(Deserialize)]
struct PackumentVersion {
    #[serde(default)]
    dist: NpmDist,
}

#[derive(Deserialize, Default)]
struct NpmDist {
    #[serde(default)]
    tarball: String,
    #[serde(default)]
    integrity: Option<String>,
}

struct ResolvedSource {
    url: String,
    integrity: Option<String>,
    version: String,
}

async fn resolve_source(
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
            })
        }
        PluginSourceWire::Tarball { url, integrity } => Ok(ResolvedSource {
            url: url.clone(),
            integrity: integrity.clone(),
            version: requested_version.unwrap_or("latest").to_string(),
        }),
        PluginSourceWire::Github { repo } => {
            let repo = repo.trim_matches('/');
            if repo.split('/').count() != 2 {
                return Err(format!("非法的 GitHub 仓库: {repo}"));
            }
            Ok(ResolvedSource {
                url: format!("https://codeload.github.com/{repo}/tar.gz/HEAD"),
                integrity: None,
                version: requested_version.unwrap_or("HEAD").to_string(),
            })
        }
    }
}

/// `@scope/name` for npm sources, bare repo name otherwise — this is the id
/// DSH uses in `cordis.patch.yml` and the path under `node_modules`.
fn registry_id_of(source: &PluginSourceWire) -> String {
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

fn sanitize_pkg_path(name: &str) -> Result<String, String> {
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

/* ------------------------------ install ------------------------------ */

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_plugin(
    transfers: State<'_, Transfers>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    plugin_id: String,
    source: PluginSourceWire,
    version: Option<String>,
    registry_base: String,
    instance_id: String,
    on_progress: Channel<PluginProgressEvent>,
) -> Result<PluginInstallOutcome, String> {
    // The profile directory is resolved backend-side from the instance id —
    // the manifest's profile is the authority, not a path from the WebView.
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
    let flag = transfers.take(&transfer_id);
    let result = run_plugin_install(
        &flag,
        &plugin_id,
        &source,
        version.as_deref(),
        &registry_base,
        &profile,
        &on_progress,
    )
    .await;
    transfers.release(&transfer_id);
    result
}

async fn run_plugin_install(
    flag: &Arc<AtomicBool>,
    plugin_id: &str,
    source: &PluginSourceWire,
    requested_version: Option<&str>,
    registry_base: &str,
    instance_root: &Path,
    on_progress: &Channel<PluginProgressEvent>,
) -> Result<PluginInstallOutcome, String> {
    if instance_root.as_os_str().is_empty() {
        return Err("实例目录未设置".into());
    }
    let registry_id = registry_id_of(source);
    let registry_id = sanitize_pkg_path(&registry_id)?;

    let resolved = resolve_source(source, requested_version, registry_base, on_progress).await?;
    if cancelled(flag) {
        return Err("cancelled".into());
    }

    // Stream the tarball into the shared cache dir, hashing as we go.
    let cache_dir = instance_root.join(".phl-cache");
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| format!("无法创建缓存目录: {e}"))?;
    let part_path = cache_dir.join(format!("{}.part", sanitize_cache_name(&registry_id)));
    let downloaded = download(
        flag,
        &resolved.url,
        &part_path,
        None,
        &|progress, bytes_done, bytes_per_sec| {
            let _ = on_progress.send(PluginProgressEvent::Downloading {
                progress,
                bytes_done,
                bytes_per_sec,
            });
        },
    )
    .await;

    let downloaded = match downloaded {
        Ok(d) => d,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(e);
        }
    };

    let _ = on_progress.send(PluginProgressEvent::Verifying);
    if let Err(e) = verify_integrity(&downloaded.sha512, resolved.integrity.as_deref()) {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(e);
    }
    if cancelled(flag) {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err("cancelled".into());
    }

    // Unpack into a staging dir, then move into node_modules/<registry_id>.
    let node_modules = instance_root.join("node_modules");
    let staging = node_modules.join(format!(".phl-tmp-{}", sanitize_cache_name(&registry_id)));
    let backup = node_modules.join(format!(".phl-old-{}", sanitize_cache_name(&registry_id)));
    let dest = node_modules.join(&registry_id);
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;

    let install_result = async {
        extract(
            &part_path,
            &staging,
            &|progress| {
                let _ = on_progress.send(PluginProgressEvent::Installing { progress });
            },
            flag,
        )
        .await?;
        if !staging.exists() {
            return Err("压缩包内没有预期的 package/ 前缀".into());
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("无法创建插件目录: {e}"))?;
        }
        // Only now is there something to put in place. An install over an
        // existing plugin is an *update*, and a failed or cancelled extract
        // must leave the previous copy alone — the instance record still
        // points at it, and cordis.patch.yml still references it.
        let had_previous = dest.exists() && tokio::fs::rename(&dest, &backup).await.is_ok();
        match tokio::fs::rename(&staging, &dest).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if had_previous {
                    let _ = tokio::fs::rename(&backup, &dest).await;
                }
                Err(format!("无法放置插件目录: {e}"))
            }
        }
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_dir_all(&backup).await;
    let _ = tokio::fs::remove_file(&part_path).await;
    install_result?;

    let marker = serde_json::json!({
        "installedAt": now_iso(),
        // The catalog id (`owner/repo`) is *not* the registry id (npm package
        // name), and the instance's plugin list is keyed by the former. Record
        // it so the install can be read back off disk without guessing.
        "pluginId": plugin_id,
        "version": resolved.version,
        "kind": source_kind(source),
        "registryId": registry_id,
        "source": source,
        "integrity": resolved.integrity.unwrap_or_default(),
        "bytes": downloaded.bytes,
    });
    tokio::fs::write(dest.join("phl-plugin.json"), marker.to_string())
        .await
        .map_err(|e| format!("无法写入安装记录: {e}"))?;

    register_cordis_patch(instance_root, &registry_id, None)
        .await
        .map_err(|e| format!("注册 cordis.patch.yml 失败: {e}"))?;

    // A reinstall over a plugin the user had disabled must clear the flag:
    // `register_cordis_patch` leaves an existing block untouched, so without
    // this the frontend would record the plugin as enabled while DSH kept
    // refusing to load it — and the next disk scan would flip the switch back.
    set_plugin_disabled(instance_root, &registry_id, false)
        .await
        .map_err(|e| format!("更新 cordis.patch.yml 失败: {e}"))?;

    Ok(PluginInstallOutcome {
        version: resolved.version,
        registry_id,
    })
}

fn source_kind(source: &PluginSourceWire) -> &'static str {
    match source {
        PluginSourceWire::Npm { .. } => "npm",
        PluginSourceWire::Tarball { .. } => "tarball",
        PluginSourceWire::Github { .. } => "github",
    }
}

fn sanitize_cache_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

/// Latest published version for an npm-sourced plugin; `None` for anything
/// that has no npm dist-tags to consult (tarball / GitHub sources).
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

/* -------------------------- cordis.patch.yml -------------------------- */
//
// Line-level editing keeps the file byte-stable outside the one plugin block
// — comments and unrelated entries (including DSH's own) survive untouched,
// which a serde_yaml round-trip would not guarantee.

fn patch_path(instance_root: &Path) -> PathBuf {
    instance_root.join("cordis.patch.yml")
}

async fn read_patch_lines(instance_root: &Path) -> Vec<String> {
    match tokio::fs::read_to_string(patch_path(instance_root)).await {
        Ok(text) => text.lines().map(|l| l.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

async fn write_patch_lines(instance_root: &Path, lines: &[String]) -> Result<(), String> {
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    tokio::fs::create_dir_all(instance_root)
        .await
        .map_err(|e| format!("实例目录不存在: {e}"))?;
    tokio::fs::write(patch_path(instance_root), text)
        .await
        .map_err(|e| e.to_string())
}

/// Registry ids currently flagged `disabled: true` in the profile's patch
/// file. The Instance Manager reads the enabled state from here rather than
/// from any record of its own — the file DSH actually consults is the only
/// answer that cannot drift.
pub(crate) async fn disabled_plugin_ids(instance_root: &Path) -> std::collections::HashSet<String> {
    let lines = read_patch_lines(instance_root).await;
    let mut disabled = std::collections::HashSet::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("- id:") else {
            continue;
        };
        let id = rest
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();
        if id.is_empty() {
            continue;
        }
        let Some(range) = find_block(&lines, &id) else {
            continue;
        };
        if range.0 != i {
            continue; // a later duplicate block; the first one wins
        }
        let indent = child_indent(&lines, range);
        let flagged = (range.0 + 1..range.1).any(|j| {
            indent_of(&lines[j]) == indent
                && lines[j]
                    .trim_start()
                    .strip_prefix("disabled:")
                    .is_some_and(|v| v.trim() == "true")
        });
        if flagged {
            disabled.insert(id);
        }
    }
    disabled
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Line range `[start, end)` of the `- id: <id>` block in a top-level list.
///
/// The block ends at the next non-empty line indented at or above the item's
/// own level. Matching a trimmed `"- "` prefix instead would also stop at
/// *nested* sequence items (`      - a` under `config:`), cutting the block
/// in half and orphaning its tail at the document's top level.
fn find_block(lines: &[String], id: &str) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    let mut base = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        match start {
            None => {
                if let Some(rest) = trimmed.strip_prefix("- id:") {
                    if rest.trim().trim_matches(|c| c == '\'' || c == '"') == id {
                        start = Some(i);
                        base = indent_of(line);
                    }
                }
            }
            Some(s) => {
                if !trimmed.is_empty() && indent_of(line) <= base {
                    return Some((s, i));
                }
            }
        }
    }
    start.map(|s| (s, lines.len()))
}

/// Indentation of the block's direct children, so keys are read and written
/// at the right level instead of matching something nested deeper.
fn child_indent(lines: &[String], range: (usize, usize)) -> usize {
    (range.0 + 1..range.1)
        .find(|&i| !lines[i].trim().is_empty())
        .map(|i| indent_of(&lines[i]))
        .unwrap_or_else(|| indent_of(&lines[range.0]) + 2)
}

/// Writes `disabled: true/false` for the plugin block, inserting the block
/// when the plugin was never registered (e.g. an out-of-band install).
async fn set_plugin_disabled(
    instance_root: &Path,
    registry_id: &str,
    disabled: bool,
) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    match find_block(&lines, registry_id) {
        Some(range) => {
            // Scope the key to the block's own level: a `disabled:` sitting
            // inside a nested config mapping belongs to that sub-mapping,
            // not to the plugin.
            let indent = child_indent(&lines, range);
            let at = (range.0 + 1..range.1).find(|&i| {
                indent_of(&lines[i]) == indent && lines[i].trim_start().starts_with("disabled:")
            });
            match (disabled, at) {
                (false, Some(i)) => {
                    lines.remove(i);
                }
                (true, Some(i)) => {
                    lines[i] = format!("{}disabled: true", " ".repeat(indent));
                }
                (true, None) => {
                    lines.insert(range.0 + 1, format!("{}disabled: true", " ".repeat(indent)));
                }
                (false, None) => {}
            }
            write_patch_lines(instance_root, &lines).await
        }
        None if disabled => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("- id: {registry_id}"));
            lines.push(format!("  name: {registry_id}"));
            lines.push("  disabled: true".into());
            write_patch_lines(instance_root, &lines).await
        }
        None => Ok(()), // enabling something unregistered is a no-op
    }
}

async fn remove_plugin_block(instance_root: &Path, registry_id: &str) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    if let Some((start, end)) = find_block(&lines, registry_id) {
        lines.drain(start..end);
        write_patch_lines(instance_root, &lines).await
    } else {
        Ok(())
    }
}

/// Adds the `- id: …` entry if missing. `disabled` seeds the block with the
/// flag when the caller already knows the plugin starts disabled.
async fn register_cordis_patch(
    instance_root: &Path,
    registry_id: &str,
    disabled: Option<bool>,
) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    if find_block(&lines, registry_id).is_some() {
        return Ok(());
    }
    if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push(format!("- id: {registry_id}"));
    lines.push(format!("  name: {registry_id}"));
    if disabled == Some(true) {
        lines.push("  disabled: true".into());
    }
    write_patch_lines(instance_root, &lines).await
}

#[tauri::command]
pub async fn set_plugin_enabled(
    phl: State<'_, PhlState>,
    instance_id: String,
    registry_id: String,
    enabled: bool,
) -> Result<(), String> {
    let registry_id = sanitize_pkg_path(&registry_id)?;
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
    set_plugin_disabled(&profile, &registry_id, !enabled).await
}

#[tauri::command]
pub async fn uninstall_plugin(
    phl: State<'_, PhlState>,
    instance_id: String,
    registry_id: String,
) -> Result<(), String> {
    let registry_id = sanitize_pkg_path(&registry_id)?;
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
    remove_plugin_block(&profile, &registry_id).await?;
    let dir = profile.join("node_modules").join(&registry_id);
    crate::paths::ensure_under_root(&profile.join("node_modules"), &dir)?;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(|l| l.to_string()).collect()
    }

    const NESTED: &str = "\
- id: foo
  name: foo
  config:
    items:
      - a
      - b
- id: bar
  name: bar
";

    #[test]
    fn block_ends_at_a_sibling_not_at_a_nested_item() {
        let doc = lines(NESTED);
        // `      - a` is a nested sequence item; ending the block there would
        // orphan the rest of foo's config at the document's top level.
        assert_eq!(find_block(&doc, "foo"), Some((0, 6)));
        assert_eq!(find_block(&doc, "bar"), Some((6, 8)));
        assert_eq!(find_block(&doc, "missing"), None);
    }

    #[test]
    fn removing_a_block_leaves_the_rest_intact() {
        let mut doc = lines(NESTED);
        let (start, end) = find_block(&doc, "foo").unwrap();
        doc.drain(start..end);
        assert_eq!(doc, lines("- id: bar\n  name: bar\n"));
    }

    #[test]
    fn disabled_key_is_scoped_to_the_block_level() {
        let doc = lines("- id: foo\n  config:\n    disabled: true\n  name: foo\n");
        let range = find_block(&doc, "foo").unwrap();
        let indent = child_indent(&doc, range);
        assert_eq!(indent, 2);
        // The `disabled: true` at indent 4 belongs to `config`, not to foo,
        // so the flag lookup must not find it.
        let found = (range.0 + 1..range.1).find(|&i| {
            indent_of(&doc[i]) == indent && doc[i].trim_start().starts_with("disabled:")
        });
        assert_eq!(found, None);
    }

    #[test]
    fn nested_list_style_documents_are_handled() {
        // DSH may write the list under a top-level key, indenting every item.
        let doc = lines("plugins:\n  - id: foo\n    name: foo\n  - id: bar\n");
        assert_eq!(find_block(&doc, "foo"), Some((1, 3)));
        assert_eq!(child_indent(&doc, (1, 3)), 4);
    }
}
