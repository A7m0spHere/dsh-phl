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
/// models.dev adds new releases within days; a week-old cache made "the model
/// came out last week" systematically unresolvable until a second enrich ran.
/// A full fetch is one bounded GET (≤32 MiB), so daily freshness is cheap.
const TTL: u64 = 24 * 60 * 60;
const RETRY: u64 = 5 * 60;
const LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Second-tier catalog, consulted only for OpenRouter-style ids or an
/// OpenRouter base URL (see `uses_openrouter`). Public, keyless, updates the
/// day a model ships — which is exactly the gap models.dev sometimes leaves.
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENROUTER_TTL: u64 = 24 * 60 * 60;

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

/// Base-URL host → models.dev provider ids. This replaces the old disambiguation
/// hint for cross-provider name collisions: the hint callers used to pass is the
/// user's own display name for the provider ("我的OpenRouter"), which can never
/// equal a catalog provider id. The endpoint a user configured, however, says
/// exactly who serves the model — the same inference every open-source launcher
/// of this shape makes. Each row lists the provider ids to try, in order.
const HOST_PROVIDERS: &[(&str, &[&str])] = &[
    ("api.openai.com", &["openai"]),
    ("openai.azure.com", &["azure"]),
    ("api.anthropic.com", &["anthropic"]),
    ("api.deepseek.com", &["deepseek"]),
    ("api.moonshot.cn", &["moonshot"]),
    ("open.bigmodel.cn", &["zhipu"]),
    ("api.minimax.chat", &["minimax"]),
    ("generativelanguage.googleapis.com", &["google"]),
    ("api.openrouter.ai", &["openrouter"]),
    ("openrouter.ai", &["openrouter"]),
    ("api.groq.com", &["groq"]),
    ("api.fireworks.ai", &["fireworks"]),
    ("api.together.xyz", &["together"]),
    ("dashscope.aliyuncs.com", &["alibaba"]),
    ("api.x.ai", &["xai"]),
    ("api.mistral.ai", &["mistral"]),
    ("api.siliconflow.cn", &["siliconflow"]),
    ("api.stepfun.com", &["stepfun"]),
    ("open.xiaomi.com", &["xiaomi"]),
    ("api.voyageai.com", &["voyage"]),
    ("api.perplexity.ai", &["perplexity"]),
    ("api.cohere.com", &["cohere"]),
];

/// The catalog provider ids implied by a configured base URL, if any.
fn host_providers(base_url: Option<&str>) -> Vec<String> {
    let Some(url) = base_url else {
        return Vec::new();
    };
    let rest = url
        .split_once("//")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host = rest
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    HOST_PROVIDERS
        .iter()
        .find(|(h, _)| h == &host)
        .map(|(_, ids)| ids.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

/// True when the batch should consult the OpenRouter catalog: any id carries
/// a `vendor/model` prefix (OpenRouter's namespace convention), or the base
/// URL is an OpenRouter endpoint.
fn uses_openrouter(models: &[ModelRef], base_url: Option<&str>) -> bool {
    models.iter().any(|m| m.id.contains('/'))
        || host_providers(base_url).iter().any(|p| p == "openrouter")
}

fn choose<'a>(rows: Vec<&'a Entry>, hints: &[String]) -> Match<'a> {
    if rows.len() == 1 {
        return Match::Found(rows[0]);
    }
    if rows.is_empty() {
        return Match::Missing;
    }
    // Hints are tried in order (base-URL inference first, the legacy display
    // name last); a hint that isolates exactly one row disambiguates.
    for hint in hints {
        let local: Vec<_> = rows
            .iter()
            .copied()
            .filter(|e| &e.provider == hint)
            .collect();
        if local.len() == 1 {
            return Match::Found(local[0]);
        }
    }
    Match::Ambiguous
}

/// Last-resort id spelling, used ONLY after every literal comparison failed
/// (the layers above keep their old exact semantics — this can never change
/// a match that already existed). Collapses separators (`gpt-4.1` ≡ `gpt-4-1`,
/// `model_name` ≡ `model-name`), the Amazon-style `:0` version tag, and the
/// snapshot / alias suffixes providers bolt onto otherwise-identical base
/// names: `-20240806`, `-240806`, `-v1`, `-latest`, `-preview`, `-exp`,
/// `-chat`. New releases ship under exactly those dated names far more often
/// than under fresh bases, and this is the same normalization ladder
/// LiteLLM/Cline apply when matching live ids against curated tables.
fn canonicalize(id: &str) -> String {
    let mut s = id.trim().to_lowercase().replace(['_', '.'], "-");
    if let Some(pos) = s.find(':') {
        let suffix = &s[pos + 1..];
        if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
            s.truncate(pos);
        }
    }
    while let Some((head, tail)) = s.rsplit_once('-') {
        let all_digits = !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit());
        let dated = all_digits && matches!(tail.len(), 6 | 8 | 10);
        let alias = matches!(tail, "latest" | "preview" | "exp" | "chat" | "beta");
        // `-v…` forms: only drop when the rest of the segment is digits
        // (`opus-4-v1`), never `vllm`/`vision`-style words.
        let vnum = match tail.strip_prefix('v') {
            Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
            None => false,
        };
        if head.is_empty() || !(dated || alias || vnum) {
            break;
        }
        s = head.to_string();
    }
    s
}

fn lookup<'a>(entries: &'a [Entry], id: &str, hints: &[String]) -> Match<'a> {
    let id = id.trim().to_lowercase();
    if id.is_empty() {
        return Match::Missing;
    }
    let exact: Vec<_> = entries
        .iter()
        .filter(|e| e.model.id.to_lowercase() == id)
        .collect();
    if !exact.is_empty() {
        return choose(exact, hints);
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
            return choose(qualified, hints);
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
            return choose(stripped, hints);
        }
    }
    let tail = parts.last().copied().unwrap_or("");
    let tailed = choose(
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
        hints,
    );
    if !matches!(tailed, Match::Missing) {
        return tailed;
    }
    // Final layer: canonical spelling. Ambiguity discipline is unchanged —
    // `choose` still refuses to guess (and here two catalog rows sharing a
    // canonical form is exactly the dated-snapshot pair the hint must split).
    // Canonicalize the WHOLE id (so `gpt-5:0` loses its version tag before
    // the tail ever matters) and match either the full or the tail form.
    let base = canonicalize(&id);
    let base_tail = base.rsplit(['/', ':']).next().unwrap_or("").to_string();
    if (base.is_empty() || base == id) && base_tail == tail {
        return Match::Missing;
    }
    let mut rows: Vec<_> = entries
        .iter()
        .filter(allowed)
        .filter(|e| {
            let eid = e.model.id.to_lowercase();
            canonicalize(&eid) == base
                || canonicalize(eid.rsplit(['/', ':']).next().unwrap_or(&eid)) == base_tail
        })
        .collect();
    if rows.is_empty() {
        // The hinted hosts (base-URL inference, then the display name) carry
        // models.dev rows under their own provider; try them when the plain
        // tail had nothing.
        rows = entries
            .iter()
            .filter(|e| hints.contains(&e.provider) && canonicalize(&e.model.id) == base)
            .collect();
    }
    choose(rows, hints)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub model: ModelRef,
    pub matched: bool,
    pub changed: bool,
    pub ambiguous: bool,
}

/// A row of the second-tier OpenRouter catalog (public, keyless
/// `/api/v1/models`). Only numbers the endpoint actually states are kept:
/// output-token ceilings and reasoning levels are not part of its model
/// metadata, so they never originate here.
#[derive(Clone)]
pub(crate) struct OpenRouterEntry {
    /// Full lowercased id, `vendor/model`.
    pub id: String,
    pub provider: String,
    /// Fields the endpoint does not state stay `None` — the fill macro is
    /// field-uniform, and absence must never be invented.
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub input: Option<Vec<ModelInput>>,
    pub reasoning_efforts: Option<super::ReasoningEfforts>,
}

fn parse_openrouter(data: &Value) -> Vec<OpenRouterEntry> {
    let Some(list) = data.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.trim().to_lowercase();
            if id.is_empty() {
                return None;
            }
            let provider = id.split('/').next().unwrap_or("").to_string();
            let context_window = m
                .get("context_length")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0);
            let input = m
                .pointer("/architecture/input_modalities")
                .and_then(Value::as_array)
                .map(|mods| {
                    let mut out = Vec::new();
                    if mods.iter().any(|v| v.as_str() == Some("text")) {
                        out.push(ModelInput::Text);
                    }
                    if mods.iter().any(|v| v.as_str() == Some("image")) {
                        out.push(ModelInput::Image);
                    }
                    out
                })
                .filter(|out: &Vec<ModelInput>| !out.is_empty());
            Some(OpenRouterEntry {
                id,
                provider,
                name: None,
                context_window,
                max_tokens: None,
                input,
                reasoning_efforts: None,
            })
        })
        .collect()
}

/// Same discipline as `lookup`, smaller vocabulary: exact id → bare-tail
/// (OpenRouter ids are `vendor/model`; a user route may speak either) →
/// canonical spelling. Multiple candidates only resolve when a hint names the
/// vendor; otherwise no answer beats a wrong one.
fn lookup_openrouter<'a>(
    rows: &'a [OpenRouterEntry],
    id: &str,
    hints: &[String],
) -> Option<&'a OpenRouterEntry> {
    let id = id.trim().to_lowercase();
    if id.is_empty() {
        return None;
    }
    let tail_of = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let mut exact: Vec<&OpenRouterEntry> = rows.iter().filter(|e| e.id == id).collect();
    if exact.is_empty() {
        let tail = tail_of(&id);
        exact = rows.iter().filter(|e| tail_of(&e.id) == tail).collect();
    }
    if exact.is_empty() {
        let base = canonicalize(&id);
        let base_tail = tail_of(&base);
        if !base.is_empty() && base != id {
            exact = rows
                .iter()
                .filter(|e| e.id == base || canonicalize(&tail_of(&e.id)) == base_tail)
                .collect();
        }
    }
    match exact.len() {
        0 => None,
        1 => Some(exact[0]),
        _ => hints.iter().find_map(|hint| {
            let local: Vec<_> = exact
                .iter()
                .copied()
                .filter(|e| &e.provider == hint)
                .collect();
            (local.len() == 1).then_some(local[0])
        }),
    }
}

fn enrich(
    model: ModelRef,
    entries: &[Entry],
    hints: &[String],
    openrouter: Option<&[OpenRouterEntry]>,
) -> Resolution {
    if model.id.trim().is_empty() {
        return Resolution {
            model,
            matched: false,
            changed: false,
            ambiguous: false,
        };
    }
    let found = lookup(entries, &model.id, hints);
    let hit = match found {
        Match::Found(e) => Some(&e.model),
        _ => None,
    };
    let or_hit = openrouter.and_then(|rows| lookup_openrouter(rows, &model.id, hints));
    let mut next = model.clone();
    // Missing means absent, including for empty input/maps and explicit false.
    // Per-field provenance records mixtures without labelling defaults as facts.
    //
    // A field that is `None` OR was previously stamped `fallback` is fillable:
    // the compat defaults (262144 / 32768 / text) are guesses, and once the
    // catalog learns the model (new release lands in models.dev or OpenRouter)
    // the corrected fact must replace the guess. The old None-only rule made
    // first-run guesses permanent — the exact failure the user hits with a
    // brand-new model. User-set fields (`manual`) are never touched.
    macro_rules! fill {
        ($field:ident, $key:literal, $fallback:expr) => {
            let stale_fallback = next
                .metadata_sources
                .get($key)
                .is_some_and(|s| *s == MetadataSource::Fallback);
            if next.$field.is_none() || stale_fallback {
                let catalog = hit
                    .and_then(|m| m.$field.clone())
                    .map(|v| (v, MetadataSource::ModelsDev))
                    .or_else(|| {
                        or_hit
                            .and_then(|o| o.$field.clone())
                            .map(|v| (v, MetadataSource::OpenRouter))
                    });
                if let Some((value, source)) = catalog {
                    next.$field = Some(value);
                    next.metadata_sources.insert($key.into(), source);
                } else if next.$field.is_none() {
                    if let Some(value) = $fallback {
                        next.$field = Some(value);
                        next.metadata_sources
                            .insert($key.into(), MetadataSource::Fallback);
                    }
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
        matched: hit.is_some() || or_hit.is_some(),
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
    /// `force` bypasses the freshness window for the explicit "更新目录"
    /// affordance; a fetch failure still leaves the cached catalog in place.
    async fn load(&mut self, root: &Path, url: &str, now: u64, force: bool) {
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
        if !force && (fresh || now < self.retry_at) {
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
    /// Whether the OpenRouter second tier was consulted for this batch at all
    /// (it may also be consulted-and-empty; the UI explains "未找到" honestly).
    pub openrouter_consulted: bool,
}

/// OpenRouter rows live in memory only (the public list is small and fast;
/// a restart pays one GET). On a fetch failure the previous rows survive.
#[derive(Default)]
struct OpenRouterCache {
    fetched_at: Option<u64>,
    retry_at: u64,
    rows: Vec<OpenRouterEntry>,
}

async fn fetch_openrouter(now: u64) -> Option<Vec<OpenRouterEntry>> {
    let response = crate::versions::http_client()
        .get(OPENROUTER_URL)
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|n| n > 16 * 1024 * 1024)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    let data: Value = serde_json::from_slice(&bytes).ok()?;
    let rows = parse_openrouter(&data);
    if rows.is_empty() {
        return None;
    }
    let _ = now;
    Some(rows)
}

#[tauri::command]
pub async fn enrich_model_metadata(
    phl: State<'_, PhlState>,
    models: Vec<ModelRef>,
    provider: Option<String>,
    // The route the models would be called on — its host identifies the
    // serving provider far more reliably than the user's display name ever
    // can (see `HOST_PROVIDERS`). `force_refresh` = "更新目录并补全";
    // `use_openrouter` is the settings gate (absent means enabled).
    base_url: Option<String>,
    force_refresh: Option<bool>,
    use_openrouter: Option<bool>,
) -> Result<EnrichmentBatch, String> {
    // Root-keyed and serialized across callers: one fetch, no concurrent cache writes.
    static CACHES: OnceLock<tokio::sync::Mutex<HashMap<PathBuf, CatalogCache>>> = OnceLock::new();
    static OPENROUTER: OnceLock<tokio::sync::Mutex<HashMap<PathBuf, OpenRouterCache>>> =
        OnceLock::new();
    let root = phl.root();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut caches = CACHES.get_or_init(Default::default).lock().await;
    let cache = caches.entry(root.clone()).or_default();
    cache
        .load(&root, URL, now, force_refresh.unwrap_or(false))
        .await;
    let catalog_status = match cache.fetched_at {
        Some(stamp) if stamp <= now && now - stamp < TTL => "fresh",
        Some(_) => "stale",
        None => "unavailable",
    }
    .into();
    // Disambiguation hints, best first: the base-URL host mapping, then the
    // legacy display-name hint (still right when the user happened to name the
    // provider like a catalog id).
    let mut hints = host_providers(base_url.as_deref());
    if let Some(name) = provider.as_deref() {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !hints.contains(&name) {
            hints.push(name);
        }
    }
    let wants_or = use_openrouter.unwrap_or(true) && uses_openrouter(&models, base_url.as_deref());
    let mut openrouter_rows: Option<Vec<OpenRouterEntry>> = None;
    if wants_or {
        let mut or_caches = OPENROUTER.get_or_init(Default::default).lock().await;
        let entry = or_caches.entry(root.clone()).or_default();
        let fresh = entry
            .fetched_at
            .is_some_and(|stamp| stamp <= now && now - stamp < OPENROUTER_TTL);
        if (!fresh && now >= entry.retry_at) || force_refresh.unwrap_or(false) {
            entry.retry_at = now.saturating_add(RETRY);
            if let Some(rows) = fetch_openrouter(now).await {
                entry.rows = rows;
                entry.fetched_at = Some(now);
            }
        }
        if !entry.rows.is_empty() {
            openrouter_rows = Some(entry.rows.clone());
        }
    }
    let openrouter_consulted = wants_or && openrouter_rows.is_some();
    let empty: Vec<OpenRouterEntry> = Vec::new();
    let or_slice = openrouter_rows.as_deref().unwrap_or(&empty);
    Ok(EnrichmentBatch {
        results: models
            .into_iter()
            .map(|m| {
                enrich(
                    m,
                    &cache.entries,
                    &hints,
                    openrouter_consulted.then_some(or_slice),
                )
            })
            .collect(),
        catalog_status,
        openrouter_consulted,
    })
}

#[cfg(test)]
mod tests;
