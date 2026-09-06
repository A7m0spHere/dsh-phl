//! Native, optional metadata enrichment. Never reads credentials or writes an instance.
//! Model metadata enrichment inspired by dsh-model-info-fill (MIT); independently implemented.
use super::{FalseValue, MetadataSource, ModelInput, ModelRef, ReasoningEfforts};
use crate::paths::PhlState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::State;

const URL: &str = "https://models.dev/api.json";
const TTL: u64 = 7 * 24 * 60 * 60;
const RETRY: u64 = 5 * 60;
const LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone)]
struct Entry {
    provider: String,
    model: ModelRef,
}

fn parse(data: &Value) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Some(providers) = data.as_object() else {
        return entries;
    };
    for (key, provider) in providers {
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .trim()
            .to_lowercase();
        for raw in models.values() {
            let Some(id) = raw
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            else {
                continue;
            };
            let input = raw
                .pointer("/modalities/input")
                .and_then(Value::as_array)
                .and_then(|items| {
                    let mut out = Vec::new();
                    if items.iter().any(|v| v == "text") {
                        out.push(ModelInput::Text);
                    }
                    if items.iter().any(|v| v == "image") {
                        out.push(ModelInput::Image);
                    }
                    (!out.is_empty()).then_some(out)
                });
            let reasoning_efforts = if raw.get("reasoning") == Some(&Value::Bool(false)) {
                Some(ReasoningEfforts::Disabled(FalseValue))
            } else {
                // A capability boolean says nothing about supported wire effort values.
                // Only explicit, DSH-supported levels may be advertised; never invent `off`.
                raw.get("reasoning_options")
                    .and_then(Value::as_array)
                    .and_then(|options| {
                        options
                            .iter()
                            .filter(|o| o.get("type").and_then(Value::as_str) == Some("effort"))
                            .find_map(|o| {
                                let values = o.get("values")?.as_array()?;
                                let levels: BTreeMap<String, Option<String>> = values
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .filter(|s| LEVELS.contains(s))
                                    .map(|s| (s.to_string(), (s != "off").then(|| s.to_string())))
                                    .collect();
                                levels
                                    .keys()
                                    .any(|s| s != "off")
                                    .then_some(ReasoningEfforts::Levels(levels))
                            })
                    })
            };
            entries.push(Entry {
                provider: provider_id.clone(),
                model: ModelRef {
                    id: id.trim().into(),
                    name: raw
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string),
                    context_window: raw
                        .pointer("/limit/context")
                        .and_then(Value::as_u64)
                        .filter(|n| *n > 0),
                    max_tokens: raw
                        .pointer("/limit/output")
                        .and_then(Value::as_u64)
                        .filter(|n| *n > 0),
                    input,
                    reasoning_efforts,
                    ..Default::default()
                },
            });
        }
    }
    entries
}

enum Match<'a> {
    Found(&'a Entry),
    Ambiguous,
    Missing,
}

fn choose<'a>(rows: Vec<&'a Entry>, provider: &str) -> Match<'a> {
    if rows.len() == 1 {
        return Match::Found(rows[0]);
    }
    let local: Vec<_> = rows
        .iter()
        .copied()
        .filter(|e| e.provider == provider)
        .collect();
    if local.len() == 1 {
        return Match::Found(local[0]);
    }
    if rows.is_empty() {
        Match::Missing
    } else {
        Match::Ambiguous
    }
}

fn lookup<'a>(entries: &'a [Entry], id: &str, provider: Option<&str>) -> Match<'a> {
    let id = id.trim().to_lowercase();
    if id.is_empty() {
        return Match::Missing;
    }
    let hint = provider.unwrap_or("").trim().to_lowercase();
    let exact: Vec<_> = entries
        .iter()
        .filter(|e| e.model.id.to_lowercase() == id)
        .collect();
    if !exact.is_empty() {
        return choose(exact, &hint);
    }
    let parts: Vec<_> = id.split(['/', ':']).filter(|s| !s.is_empty()).collect();
    // Provider-qualified paths, longest first: openrouter/openai/gpt ->
    // (openrouter, openai/gpt), then (openai, gpt).
    for split in 1..parts.len() {
        let suffix = parts[split..].join("/");
        let qualified: Vec<_> = entries
            .iter()
            .filter(|e| e.provider == parts[split - 1] && e.model.id.to_lowercase() == suffix)
            .collect();
        if !qualified.is_empty() {
            return choose(qualified, &hint);
        }
    }
    // A recognized explicit qualifier constrains stripped/tail lookup too.
    let explicit = parts
        .iter()
        .rev()
        .skip(1)
        .find(|p| entries.iter().any(|e| e.provider == **p))
        .copied();
    let allowed = |e: &&Entry| explicit.map_or(true, |p| e.provider == p);
    for split in 1..parts.len() {
        let suffix = parts[split..].join("/");
        let stripped: Vec<_> = entries
            .iter()
            .filter(allowed)
            .filter(|e| e.model.id.to_lowercase() == suffix)
            .collect();
        if !stripped.is_empty() {
            return choose(stripped, &hint);
        }
    }
    let tail = parts.last().copied().unwrap_or("");
    choose(
        entries
            .iter()
            .filter(allowed)
            .filter(|e| {
                e.model
                    .id
                    .rsplit(['/', ':'])
                    .next()
                    .unwrap_or("")
                    .eq_ignore_ascii_case(tail)
            })
            .collect(),
        &hint,
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub model: ModelRef,
    pub matched: bool,
    pub changed: bool,
    pub ambiguous: bool,
}

fn enrich(model: ModelRef, entries: &[Entry], provider: Option<&str>) -> Resolution {
    if model.id.trim().is_empty() {
        return Resolution {
            model,
            matched: false,
            changed: false,
            ambiguous: false,
        };
    }
    let found = lookup(entries, &model.id, provider);
    let hit = match found {
        Match::Found(e) => Some(&e.model),
        _ => None,
    };
    let mut next = model.clone();
    // Missing means absent, including for empty input/maps and explicit false.
    // Per-field provenance records mixtures without labelling defaults as facts.
    macro_rules! fill {
        ($field:ident, $key:literal, $fallback:expr) => {
            if next.$field.is_none() {
                let from_catalog = hit.and_then(|m| m.$field.clone());
                let source = if from_catalog.is_some() {
                    MetadataSource::ModelsDev
                } else {
                    MetadataSource::Fallback
                };
                if let Some(value) = from_catalog.or($fallback) {
                    next.$field = Some(value);
                    next.metadata_sources.insert($key.into(), source);
                }
            }
        };
    }
    fill!(name, "name", None);
    fill!(context_window, "contextWindow", Some(262144));
    fill!(max_tokens, "maxTokens", Some(32768));
    fill!(input, "input", Some(vec![ModelInput::Text]));
    fill!(reasoning_efforts, "reasoningEfforts", None);
    Resolution {
        changed: next != model,
        model: next,
        matched: hit.is_some(),
        ambiguous: matches!(found, Match::Ambiguous),
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    version: u32,
    fetched_at: u64,
    data: Value,
}

#[derive(Default)]
struct CatalogCache {
    loaded: bool,
    fetched_at: Option<u64>,
    retry_at: u64,
    entries: Vec<Entry>,
}

impl CatalogCache {
    async fn load(&mut self, root: &Path, url: &str, now: u64) {
        let path = root.join("cache/models-dev.json");
        if !self.loaded {
            self.loaded = true;
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(file) = serde_json::from_slice::<CacheFile>(&bytes) {
                    let entries = parse(&file.data);
                    if file.version == 1 && !entries.is_empty() {
                        self.fetched_at = Some(file.fetched_at);
                        self.entries = entries;
                    }
                }
            }
        }
        let fresh = self
            .fetched_at
            .is_some_and(|stamp| stamp <= now && now - stamp < TTL);
        if fresh || now < self.retry_at {
            return;
        }
        self.retry_at = now.saturating_add(RETRY);
        let fetched = async {
            let response = crate::versions::http_client()
                .get(url)
                .timeout(Duration::from_secs(12))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            // Bound an optional enrichment payload independently of the provider API.
            if response
                .content_length()
                .is_some_and(|n| n > 32 * 1024 * 1024)
            {
                return None;
            }
            use futures_util::StreamExt;
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.ok()?;
                if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let data: Value = serde_json::from_slice(&bytes).ok()?;
            let entries = parse(&data);
            if entries.is_empty() {
                return None;
            }
            Some((
                CacheFile {
                    version: 1,
                    fetched_at: now,
                    data,
                },
                entries,
            ))
        }
        .await;
        if let Some((file, entries)) = fetched {
            self.fetched_at = Some(now);
            self.entries = entries;
            // A failed cache write must not discard the valid in-memory catalog.
            let tmp = path.with_extension("json.tmp");
            let persist = async {
                tokio::fs::create_dir_all(path.parent().unwrap()).await?;
                let bytes = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
                tokio::fs::write(&tmp, bytes).await?;
                tokio::fs::rename(&tmp, &path).await
            }
            .await;
            if persist.is_err() {
                let _ = tokio::fs::remove_file(&tmp).await;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichmentBatch {
    pub results: Vec<Resolution>,
    /// fresh / stale / unavailable (or mock in browser preview).
    pub catalog_status: String,
}

#[tauri::command]
pub async fn enrich_model_metadata(
    phl: State<'_, PhlState>,
    models: Vec<ModelRef>,
    provider: Option<String>,
) -> Result<EnrichmentBatch, String> {
    // Root-keyed and serialized across callers: one fetch, no concurrent cache writes.
    static CACHES: OnceLock<tokio::sync::Mutex<HashMap<PathBuf, CatalogCache>>> = OnceLock::new();
    let root = phl.root();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut caches = CACHES.get_or_init(Default::default).lock().await;
    let cache = caches.entry(root.clone()).or_default();
    cache.load(&root, URL, now).await;
    let catalog_status = match cache.fetched_at {
        Some(stamp) if stamp <= now && now - stamp < TTL => "fresh",
        Some(_) => "stale",
        None => "unavailable",
    }
    .into();
    Ok(EnrichmentBatch {
        results: models
            .into_iter()
            .map(|m| enrich(m, &cache.entries, provider.as_deref()))
            .collect(),
        catalog_status,
    })
}

#[cfg(test)]
mod tests;
