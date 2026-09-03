//! Global API provider library + per-instance materialization.
//!
//! The problem this solves: DSH keeps its model/provider config in
//! `$DSH_HOME/settings.yaml`, and PHL isolates every instance behind its own
//! `DSH_HOME` (see `launch.rs`), so a freshly created instance starts with no
//! config at all and the user must redo it inside the DSH WebUI. This module
//! keeps *one* library in PHL (`<root>/config/api.json`) and pushes it into
//! each instance's `settings.yaml` at create time, on explicit sync, and as a
//! drift check right before launch.
//!
//! Secrets are deliberately not part of this: entries reference an
//! *environment variable name* (`apiKeyEnv`, DSH's own field), so the key can
//! live in the user's environment or in DSH's `.credentials.yaml` — PHL's
//! store stays secret-free.
//!
//! Merge rules (cc-switch's approach, adapted to DSH's layout):
//! - Only the two DSH sections are touched: `llm-pi-ai.providers.<name>` and
//!   `agent-default-model`. Providers are keyed by name, so the merge is
//!   per-provider; every other key in the document survives untouched, and
//!   because we only ever insert, the DSH built-in (`deepseek-official`) can
//!   never be dropped by a sync.
//! - `inheritance: "none"` means PHL never writes the instance again.
//! - `import` reads an instance's live file back so the first library can be
//!   seeded from a config the user already made by hand in the DSH UI.
//!
//! The file is a `serde_yaml::Value` round-trip: unknown keys keep their
//! values, but comments and key order in existing files are not preserved —
//! `settings.yaml` is a machine-owned file that DSH rewrites itself, so that
//! is the accepted trade-off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Key in `settings.yaml` holding the model catalog / providers.
const LLM_SECTION: &str = "llm-pi-ai";
const AGENT_DEFAULT_MODEL: &str = "agent-default-model";

/* ----------------------------- wire types ----------------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// 'official' | 'aggregator' | 'custom' — display only, DSH never sees it.
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// DSH: the wire protocol (`openai-completions`, …). Absent = DSH default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Name of the environment variable DSH reads the key from. Required so a
    /// library entry can never smuggle a literal secret into `api.json`.
    pub api_key_env: String,
    #[serde(default)]
    pub models: Vec<ModelRef>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultModel {
    /// The *provider name* as it appears under `llm-pi-ai.providers` — DSH
    /// keys its catalog by name, not by our library id.
    pub provider_name: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// The global library, persisted at `<root>/config/api.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiConfig {
    /// Format tag; refuse anything but 1 on load, like the bundle format.
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<DefaultModel>,
    #[serde(default)]
    pub providers: Vec<Provider>,
}

fn default_version() -> u32 {
    1
}

/// What the instance manifest stores: a *binding* to the library, not a copy
/// of it. cc-switch's split between "what gets written to live files" and
/// "management metadata" maps onto this as (inheritance/providers/defaultModel)
/// vs (syncedAt/syncedHash).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiBinding {
    /// 'default' = every enabled provider + global defaults; 'custom' = the
    /// listed subset with optional per-instance default model; 'none' = PHL
    /// hands the instance over to the user (sync refuses).
    #[serde(default)]
    pub inheritance: String,
    #[serde(default)]
    pub provider_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<DefaultModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_at: Option<String>,
    /// Fingerprint of the last materialized section set. Launch reconcile
    /// compares the *merged result* rather than this hash (DSH rewrites the
    /// file with different whitespace), but the frontend uses it to detect
    /// "library edited since last sync" without re-reading every instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_hash: Option<String>,
}

impl Default for ApiBinding {
    fn default() -> Self {
        ApiBinding {
            inheritance: "default".into(),
            provider_ids: Vec::new(),
            default_model: None,
            synced_at: None,
            synced_hash: None,
        }
    }
}

/* ------------------------------ validation ----------------------------- */

/// Env-var-name grammar. DSH does `process.env[apiKeyEnv]`, so a lowercase or
/// space-laden name would silently read as undefined; reject it at the UI
/// boundary instead.
pub(crate) fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The provider name is pasted straight into a YAML key; keep it to the
/// characters DSH's own providers use.
fn valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/* ------------------------------ resolution ----------------------------- */

/// The materialization unit: the exact provider map + default-model value the
/// binding produces for a library state. Deterministic, so the hash doubles as
/// the drift fingerprint and the merge payload.
fn resolve_sections(
    config: &ApiConfig,
    binding: &ApiBinding,
) -> Result<(serde_yaml::Mapping, serde_yaml::Value), String> {
    let providers: Vec<&Provider> = match binding.inheritance.as_str() {
        "default" => config.providers.iter().filter(|p| p.enabled).collect(),
        "custom" => {
            let mut out = Vec::new();
            for id in &binding.provider_ids {
                let p = config
                    .providers
                    .iter()
                    .find(|p| &p.id == id)
                    .ok_or_else(|| format!("绑定引用的供应商已不在全局库中: {id}"))?;
                if p.enabled {
                    out.push(p);
                }
            }
            out
        }
        other => return Err(format!("未知的继承模式: {other}")),
    };

    let mut map = serde_yaml::Mapping::new();
    for p in &providers {
        if !valid_provider_name(&p.name) {
            return Err(format!("非法的供应商标识名: {}", p.name));
        }
        let mut entry = serde_yaml::Mapping::new();
        if let Some(api) = p.api.as_deref().filter(|s| !s.is_empty()) {
            entry.insert("api".into(), api.into());
        }
        if let Some(base) = p.base_url.as_deref().filter(|s| !s.is_empty()) {
            entry.insert("baseURL".into(), base.into());
        }
        entry.insert("apiKeyEnv".into(), p.api_key_env.as_str().into());
        if !p.models.is_empty() {
            let models: Vec<serde_yaml::Value> = p
                .models
                .iter()
                .map(|m| {
                    let mut mm = serde_yaml::Mapping::new();
                    mm.insert("id".into(), m.id.as_str().into());
                    if let Some(n) = m.name.as_deref().filter(|s| !s.is_empty()) {
                        mm.insert("name".into(), n.into());
                    }
                    if let Some(cw) = m.context_window {
                        mm.insert("contextWindow".into(), cw.into());
                    }
                    if let Some(mt) = m.max_tokens {
                        mm.insert("maxTokens".into(), mt.into());
                    }
                    serde_yaml::Value::Mapping(mm)
                })
                .collect();
            entry.insert("models".into(), serde_yaml::Value::Sequence(models));
        }
        map.insert(p.name.as_str().into(), serde_yaml::Value::Mapping(entry));
    }

    let default = binding.default_model.as_ref().or(config.default_model.as_ref());
    let default_value = match default {
        Some(d) => {
            let mut mm = serde_yaml::Mapping::new();
            mm.insert("provider".into(), d.provider_name.as_str().into());
            mm.insert("model".into(), d.model.as_str().into());
            if let Some(re) = d.reasoning_effort.as_deref().filter(|s| !s.is_empty()) {
                mm.insert("reasoningEffort".into(), re.into());
            }
            serde_yaml::Value::Mapping(mm)
        }
        // With no default model anywhere in the chain, emit Null: writing an
        // empty `agent-default-model` would override DSH's own defaulting.
        None => serde_yaml::Value::Null,
    };

    Ok((map, default_value))
}

/// Stable fingerprint over the resolved sections; `serde_yaml::Value` has no
/// `Hash`, so serialize canonically and digest that.
fn sections_hash(llm: &serde_yaml::Mapping, default_model: &serde_yaml::Value) -> String {
    let joined = format!(
        "{}\u{1}{}",
        serde_yaml::to_string(&serde_yaml::Value::Mapping(llm.clone())).unwrap_or_default(),
        serde_yaml::to_string(default_model).unwrap_or_default(),
    );
    hex::encode(Sha256::digest(joined.as_bytes()))
}

/* ------------------------------ yaml merge ----------------------------- */

/// Insert/overwrite the managed sections in a live doc. The single writer for
/// sync, reconcile and create alike — one code path means the drift check and
/// the actual write can never disagree about what "in sync" means.
fn merge_sections(
    doc: &mut serde_yaml::Value,
    llm: &serde_yaml::Mapping,
    default_model: &serde_yaml::Value,
) -> Result<(), String> {
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| "settings.yaml 顶层不是映射，拒绝改写".to_string())?;
    if !llm.is_empty() {
        let section = map
            .entry(LLM_SECTION.into())
            .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        if !section.is_mapping() {
            return Err("llm-pi-ai 段不是映射，拒绝改写".into());
        }
        let providers = section
            .as_mapping_mut()
            .unwrap()
            .entry("providers".into())
            .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        if !providers.is_mapping() {
            return Err("providers 段不是映射，拒绝改写".into());
        }
        let providers = providers.as_mapping_mut().unwrap();
        for (name, value) in llm {
            providers.insert(name.clone(), value.clone());
        }
    }
    if !default_model.is_null() {
        map.insert(AGENT_DEFAULT_MODEL.into(), default_model.clone());
    }
    Ok(())
}

async fn read_settings(dir: &Path) -> Result<serde_yaml::Value, String> {
    let path = settings_path(dir);
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_yaml::Value::Null),
        Err(e) => return Err(e.to_string()),
    };
    serde_yaml::from_str(&raw).map_err(|e| format!("实例 settings.yaml 解析失败: {e}"))
}

/* ------------------------------ commands ------------------------------ */

fn api_config_path(root: &Path) -> PathBuf {
    root.join("config").join("api.json")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("dsh-home").join("settings.yaml")
}

#[tauri::command]
pub async fn load_api_config(root: String) -> Result<Option<ApiConfig>, String> {
    let path = api_config_path(Path::new(&root));
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let config: ApiConfig =
        serde_json::from_str(&raw).map_err(|e| format!("API 配置库解析失败: {e}"))?;
    if config.version != 1 {
        return Err(format!("不支持的 API 配置版本: {}", config.version));
    }
    Ok(Some(config))
}

#[tauri::command]
pub async fn save_api_config(root: String, mut config: ApiConfig) -> Result<ApiConfig, String> {
    if config.version != 1 {
        return Err(format!("不支持的 API 配置版本: {}", config.version));
    }
    for p in &config.providers {
        if !valid_provider_name(&p.name) {
            return Err(format!("非法的供应商标识名: {}", p.name));
        }
        if !valid_env_name(&p.api_key_env) {
            return Err(format!(
                "环境变量名不合法（字母/下划线开头，仅含字母数字下划线）: {}",
                p.api_key_env
            ));
        }
    }
    config.updated_at = crate::versions::now_iso();
    let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    let path = api_config_path(Path::new(&root));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    // Same tmp+rename discipline as `write_manifest`: a crash mid-write must
    // not leave a half file that reads as "no config library".
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())?;
    Ok(config)
}

/// Materializes the binding into `<instance>/dsh-home/settings.yaml`.
/// Returns the binding as it should be persisted (sync metadata filled).
#[tauri::command]
pub async fn sync_instance_api(
    root: String,
    instance_id: String,
    binding: ApiBinding,
    config: ApiConfig,
) -> Result<ApiBinding, String> {
    let dir = crate::instances::instance_dir(Path::new(&root), &instance_id)?;
    sync_inner(&dir, &binding, &config).await
}

async fn sync_inner(dir: &Path, binding: &ApiBinding, config: &ApiConfig) -> Result<ApiBinding, String> {
    if !dir.join("instance.json").exists() {
        return Err("实例不存在，无法同步 API 配置".into());
    }
    if binding.inheritance == "none" {
        return Err("该实例未由 PHL 托管 API 配置，接管后才能同步".into());
    }
    let (llm, default_model) = resolve_sections(config, binding)?;

    let mut doc = read_settings(dir).await?;
    if doc.is_null() {
        doc = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    merge_sections(&mut doc, &llm, &default_model)?;

    let body = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
    let path = settings_path(dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("yaml.tmp");
    tokio::fs::write(&tmp, &body).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())?;

    let mut applied = binding.clone();
    applied.synced_at = Some(crate::versions::now_iso());
    applied.synced_hash = Some(sections_hash(&llm, &default_model));
    Ok(applied)
}

/// Reads an instance's live settings.yaml back into library-shaped providers
/// — the seed flow for "I already configured this instance in the DSH UI".
#[tauri::command]
pub async fn import_instance_api(
    root: String,
    instance_id: String,
) -> Result<Option<ApiConfig>, String> {
    let dir = crate::instances::instance_dir(Path::new(&root), &instance_id)?;
    import_inner(&dir).await
}

async fn import_inner(dir: &Path) -> Result<Option<ApiConfig>, String> {
    let doc = read_settings(dir).await?;
    if doc.is_null() {
        return Ok(None);
    }

    let providers_val = doc
        .get(LLM_SECTION)
        .and_then(|s| s.get("providers"))
        .and_then(|p| p.as_mapping())
        .cloned()
        .unwrap_or_default();
    let mut providers = Vec::new();
    for (name, value) in providers_val {
        let Some(name) = name.as_str() else { continue };
        let Some(vm) = value.as_mapping() else { continue };
        let get_str = |k: &str| vm.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let models = vm
            .get("models")
            .and_then(|m| m.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        Some(ModelRef {
                            id,
                            name: m.get("name").and_then(|v| v.as_str()).map(str::to_string),
                            context_window: m.get("contextWindow").and_then(|v| v.as_u64()),
                            max_tokens: m.get("maxTokens").and_then(|v| v.as_u64()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        providers.push(Provider {
            id: format!("imported-{name}"),
            name: name.to_string(),
            kind: "custom".into(),
            notes: None,
            api: get_str("api"),
            base_url: get_str("baseURL"),
            // Without a reference DSH has nothing to read; fall back to a
            // conventional variable name the user can point at later.
            api_key_env: get_str("apiKeyEnv")
                .unwrap_or_else(|| format!("DSH_{}_API_KEY", name.to_uppercase().replace(['-', '.'], "_"))),
            models,
            enabled: true,
        });
    }

    let default_model = doc
        .get(AGENT_DEFAULT_MODEL)
        .and_then(|v| v.as_mapping())
        .map(|m| DefaultModel {
            provider_name: m
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            model: m.get("model").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            reasoning_effort: m
                .get("reasoningEffort")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });

    if providers.is_empty() && default_model.is_none() {
        return Ok(None);
    }
    Ok(Some(ApiConfig {
        version: 1,
        updated_at: crate::versions::now_iso(),
        default_provider_id: providers.first().map(|p| p.id.clone()),
        default_model,
        providers,
    }))
}

async fn instance_env(root: &Path, instance_id: &str) -> HashMap<String, String> {
    let dir = match crate::instances::instance_dir(root, instance_id) {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };
    let raw = match tokio::fs::read_to_string(dir.join("instance.json")).await {
        Ok(raw) => raw,
        Err(_) => return HashMap::new(),
    };
    #[derive(Deserialize)]
    struct EnvProbe {
        #[serde(default)]
        env: HashMap<String, String>,
    }
    serde_json::from_str::<EnvProbe>(&raw).map(|p| p.env).unwrap_or_default()
}

/* ---------------------------- live snapshot ----------------------------- */

/// Everything the frontend needs to render one instance's *actual* API state
/// and the actions on it: the parsed file (for per-provider comparison),
/// whether the merge would change anything ("local changes" — including
/// deletions, which a content-by-content compare alone would miss), whether
/// the default model specifically diverges, and the env-var presence hints.
///
/// This replaced the old `instance_api_status`: with launch-time rewriting
/// gone, the file is the truth and the UI must show the truth plus a
/// diff-against-library, not just a drifted boolean.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceLiveSnapshot {
    pub inheritance: String,
    /// What `settings.yaml` actually contains. `None` = no file yet.
    pub live: Option<ApiConfig>,
    /// Merging the binding + library into the live file would change it.
    pub local_changes: bool,
    /// `agent-default-model` in the file differs from what the chain says
    /// (and the chain does set one — otherwise nothing is managed there).
    pub default_model_changed: bool,
    /// Bound providers whose `apiKeyEnv` is set nowhere PHL can see.
    pub missing_keys: Vec<String>,
}

#[tauri::command]
pub async fn instance_live_snapshot(
    root: String,
    instance_id: String,
    binding: ApiBinding,
    config: ApiConfig,
) -> Result<InstanceLiveSnapshot, String> {
    snapshot_inner(Path::new(&root), &instance_id, &binding, &config).await
}

async fn snapshot_inner(
    root: &Path,
    instance_id: &str,
    binding: &ApiBinding,
    config: &ApiConfig,
) -> Result<InstanceLiveSnapshot, String> {
    let dir = crate::instances::instance_dir(root, instance_id)?;
    let managed = binding.inheritance != "none";

    let live = import_inner(&dir).await?;

    let mut local_changes = false;
    let mut default_model_changed = false;
    // Never-synced managed instances have no baseline: a missing or foreign
    // file means "not materialized yet", not "the user edited it". Reporting
    // localChanges there would hide them from batch sync and trigger a
    // misleading overwrite confirmation on the first 同步.
    if managed && binding.synced_hash.is_some() {
        if let Ok((llm, default_model)) = resolve_sections(config, binding) {
            match read_settings(&dir).await {
                Ok(mut doc) => {
                    if doc.is_null() {
                        doc = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
                    }
                    // The default-model check reads the ORIGINAL file —
                    // after the test-merge below, `doc` already carries the
                    // expected value and the comparison would be vacuous.
                    if let Some(expected) = default_model.as_mapping() {
                        let yk = |s: &str| serde_yaml::Value::from(s);
                        let actual = doc.get(AGENT_DEFAULT_MODEL).and_then(|v| v.as_mapping());
                        default_model_changed = match actual {
                            Some(a) => {
                                a.get(yk("provider")) != expected.get(yk("provider"))
                                    || a.get(yk("model")) != expected.get(yk("model"))
                            }
                            None => true,
                        };
                    }
                    let before = serde_yaml::to_string(&doc).unwrap_or_default();
                    if merge_sections(&mut doc, &llm, &default_model).is_err() {
                        local_changes = true;
                    } else {
                        local_changes = serde_yaml::to_string(&doc).unwrap_or_default() != before;
                    }
                }
                Err(_) => local_changes = true,
            }
        } else {
            local_changes = true;
        }
    }

    let mut missing_keys: Vec<String> = Vec::new();
    if managed {
        // Env presence is a property of the binding, not of sync history.
        let env_extra = instance_env(root, instance_id).await;
        let ids: Vec<String> = match binding.inheritance.as_str() {
            "default" => config
                .providers
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.id.clone())
                .collect(),
            _ => binding.provider_ids.clone(),
        };
        for id in ids {
            let Some(p) = config.providers.iter().find(|p| p.id == id) else { continue };
            if std::env::var(&p.api_key_env).is_err() && !env_extra.contains_key(&p.api_key_env) {
                missing_keys.push(p.api_key_env.clone());
            }
        }
        missing_keys.sort();
        missing_keys.dedup();
    }

    Ok(InstanceLiveSnapshot {
        inheritance: binding.inheritance.clone(),
        live,
        local_changes,
        default_model_changed,
        missing_keys,
    })
}

/* ------------------------- launch-time detection ------------------------ */

/// Pre-launch drift *observation* — deliberately no rewriting.
///
/// The original design reconciled the live file back to the library before
/// every spawn. That silently destroyed edits the user made inside the DSH
/// WebUI (the product decision at the time was "global wins"), and it turned
/// out to be the wrong default: an instance whose config the user hand-tuned
/// is information, not a conflict. PHL now reports divergence and lets the
/// user choose per instance — 同步 (overwrite from global) or 采纳 (fold the
/// local state into the global library). This function keeps only the log
/// line, which makes the launch trail self-explanatory when the UI later
/// shows a 本地改动 badge.
pub(crate) async fn note_launch_drift(
    root: &Path,
    dir: &Path,
    manifest: &crate::instances::InstanceManifest,
) {
    let Some(binding) = manifest.api.clone() else { return };
    if binding.inheritance == "none" || binding.synced_hash.is_none() {
        return;
    }
    let Some(config) = load_config_file(root).await else { return };
    let Ok((llm, default_model)) = resolve_sections(&config, &binding) else { return };
    let Ok(mut doc) = read_settings(dir).await else { return };
    if doc.is_null() {
        doc = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    let before = serde_yaml::to_string(&doc).unwrap_or_default();
    if merge_sections(&mut doc, &llm, &default_model).is_err() {
        return;
    }
    if serde_yaml::to_string(&doc).unwrap_or_default() != before {
        eprintln!(
            "[phl] {} has local changes to managed api config; keeping them (use 同步 in PHL to overwrite)",
            manifest.id
        );
    }
}

pub(crate) async fn load_config_file(root: &Path) -> Option<ApiConfig> {
    let raw = tokio::fs::read_to_string(api_config_path(root)).await.ok()?;
    serde_json::from_str(&raw).ok()
}

/// Materialize a first-version binding at instance-create time (shared by the
/// create command and the launch path's missing-file backfill). Returns the
/// binding with sync metadata on success; `None` when there is no library to
/// push yet — never an error, callers keep the original binding on failure.
pub(crate) async fn apply_create_binding(
    root: &Path,
    dir: &Path,
    instance_id: &str,
    binding: ApiBinding,
) -> Option<ApiBinding> {
    if binding.inheritance == "none" {
        return None;
    }
    let config = load_config_file(root).await?;
    let applied = sync_inner(dir, &binding, &config).await.ok()?;
    let _ = crate::instances::set_instance_api(root, instance_id, &applied).await;
    Some(applied)
}

/* -------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, env: &str) -> Provider {
        Provider {
            id: format!("p-{name}"),
            name: name.into(),
            kind: "aggregator".into(),
            notes: None,
            api: Some("openai-completions".into()),
            base_url: Some(format!("https://{name}.example/v1")),
            api_key_env: env.into(),
            models: vec![ModelRef {
                id: format!("{name}-model"),
                name: None,
                context_window: Some(65536),
                max_tokens: Some(8192),
            }],
            enabled: true,
        }
    }

    fn config() -> ApiConfig {
        ApiConfig {
            version: 1,
            updated_at: String::new(),
            default_provider_id: Some("p-deepseek".into()),
            default_model: Some(DefaultModel {
                provider_name: "deepseek".into(),
                model: "deepseek-v4".into(),
                reasoning_effort: Some("max".into()),
            }),
            providers: vec![provider("deepseek", "DEEPSEEK_API_KEY"), provider("zen", "ZEN_API_KEY")],
        }
    }

    fn temp_instance(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
        std::fs::write(dir.join("instance.json"), "{}").unwrap();
        dir
    }

    /// The `<root>/instances/<id>` layout that commands resolve through
    /// `instance_dir` — snapshot tests need the parent, not just the dir.
    fn temp_root_with_instance(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("phl-apiroot-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("instances").join(tag);
        std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
        std::fs::write(dir.join("instance.json"), "{}").unwrap();
        (root, dir)
    }

    #[test]
    fn env_name_grammar() {
        assert!(valid_env_name("DEEPSEEK_API_KEY"));
        assert!(valid_env_name("_X"));
        assert!(!valid_env_name("9LIVES"));
        assert!(!valid_env_name("has space"));
        assert!(!valid_env_name(""));
    }

    #[test]
    fn resolve_is_deterministic() {
        let c = config();
        let b = ApiBinding::default();
        let (l1, d1) = resolve_sections(&c, &b).unwrap();
        let (l2, d2) = resolve_sections(&c, &b).unwrap();
        assert_eq!(sections_hash(&l1, &d1), sections_hash(&l2, &d2));
    }

    #[test]
    fn default_binding_injects_enabled_providers() {
        let c = config();
        let (llm, default_model) = resolve_sections(&c, &ApiBinding::default()).unwrap();
        // serde_yaml's generic `Into<Value>` key argument needs the concrete
        // type in tests (other crates also impl `From<&str>`); the production
        // code already goes through typed helpers.
        let key = |s: &str| serde_yaml::Value::from(s);
        assert!(llm.contains_key(key("deepseek")));
        assert!(llm.contains_key(key("zen")));
        assert_eq!(
            default_model.as_mapping().unwrap().get(key("model")).unwrap().as_str(),
            Some("deepseek-v4")
        );
    }

    #[test]
    fn custom_binding_requires_existing_ids() {
        let c = config();
        let b = ApiBinding {
            inheritance: "custom".into(),
            provider_ids: vec!["nope".into()],
            ..Default::default()
        };
        assert!(resolve_sections(&c, &b).is_err());
    }

    #[test]
    fn custom_binding_instances_default_model_wins() {
        let c = config();
        let b = ApiBinding {
            inheritance: "custom".into(),
            provider_ids: vec!["p-zen".into()],
            default_model: Some(DefaultModel {
                provider_name: "zen".into(),
                model: "zen-fast".into(),
                reasoning_effort: None,
            }),
            synced_at: None,
            synced_hash: None,
        };
        let (llm, default_model) = resolve_sections(&c, &b).unwrap();
        let key = |s: &str| serde_yaml::Value::from(s);
        assert!(!llm.contains_key(key("deepseek")));
        assert_eq!(
            default_model.as_mapping().unwrap().get(key("model")).unwrap().as_str(),
            Some("zen-fast")
        );
    }

    #[test]
    fn merge_preserves_unknown_keys_and_existing_providers() {
        // The product promise: an instance already has a DSH-written
        // settings.yaml with unrelated sections and providers; syncing must
        // only add/overwrite the managed parts.
        let raw = "ui-theme: dark\nagent-default-model:\n  provider: other\n  model: keep-me\nllm-pi-ai:\n  providers:\n    google:\n      apiKeyEnv: GOOGLE_API_KEY\n";
        let mut doc: serde_yaml::Value = serde_yaml::from_str(raw).unwrap();
        let (llm, default_model) = resolve_sections(&config(), &ApiBinding::default()).unwrap();
        merge_sections(&mut doc, &llm, &default_model).unwrap();
        let out = serde_yaml::to_string(&doc).unwrap();
        assert!(out.contains("ui-theme: dark"), "unknown key survived: {out}");
        assert!(out.contains("google:"), "existing provider survived: {out}");
        assert!(out.contains("deepseek:"), "bound provider injected: {out}");
        assert!(out.contains("deepseek-v4"), "default model overwritten: {out}");
        assert!(!out.contains("keep-me"), "old default replaced: {out}");
    }

    #[test]
    fn merge_keeps_user_default_when_chain_has_none() {
        let raw = "agent-default-model:\n  provider: other\n  model: keep-me\n";
        let mut doc: serde_yaml::Value = serde_yaml::from_str(raw).unwrap();
        let mut c = config();
        c.default_model = None;
        let b = ApiBinding::default();
        let (llm, default_model) = resolve_sections(&c, &b).unwrap();
        merge_sections(&mut doc, &llm, &default_model).unwrap();
        let out = serde_yaml::to_string(&doc).unwrap();
        assert!(out.contains("keep-me"), "null default must not erase: {out}");
    }

    #[tokio::test]
    async fn sync_writes_and_import_reads_back() {
        let dir = temp_instance("roundtrip");
        let c = config();
        let applied = sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();
        assert!(applied.synced_hash.is_some());

        let imported = import_inner(&dir).await.unwrap().unwrap();
        let names: Vec<&str> = imported.providers.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"deepseek") && names.contains(&"zen"));
        assert_eq!(imported.default_model.as_ref().unwrap().model, "deepseek-v4");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sync_is_idempotent_and_drift_detects_edits() {
        let dir = temp_instance("drift");
        let c = config();
        sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();

        // Re-merging must not change the file.
        let mut doc = read_settings(&dir).await.unwrap();
        let before = serde_yaml::to_string(&doc).unwrap();
        let (llm, dm) = resolve_sections(&c, &ApiBinding::default()).unwrap();
        merge_sections(&mut doc, &llm, &dm).unwrap();
        assert_eq!(before, serde_yaml::to_string(&doc).unwrap());

        // A user hand-edit of an UNMANAGED key must not read as drift; an edit
        // inside the managed provider map must.
        let raw = tokio::fs::read_to_string(settings_path(&dir)).await.unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        doc.as_mapping_mut().unwrap().insert("ui-theme".into(), "light".into());
        tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
            .await
            .unwrap();
        let mut doc = read_settings(&dir).await.unwrap();
        let before = serde_yaml::to_string(&doc).unwrap();
        merge_sections(&mut doc, &llm, &dm).unwrap();
        assert_eq!(before, serde_yaml::to_string(&doc).unwrap(), "unmanaged edit is not drift");

        let raw = tokio::fs::read_to_string(settings_path(&dir)).await.unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        let key = |s: &str| serde_yaml::Value::from(s);
        doc.as_mapping_mut()
            .unwrap()
            .get_mut(key("llm-pi-ai"))
            .unwrap()
            .get_mut(key("providers"))
            .unwrap()
            .get_mut(key("zen"))
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .insert(key("baseURL"), "https://tampered.example/v1".into());
        tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
            .await
            .unwrap();
        let mut doc = read_settings(&dir).await.unwrap();
        let before = serde_yaml::to_string(&doc).unwrap();
        merge_sections(&mut doc, &llm, &dm).unwrap();
        assert_ne!(before, serde_yaml::to_string(&doc).unwrap(), "managed edit is drift");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-data roundtrip probe: copies the machine's actual
    /// `~/.dsh/settings.yaml` into a scratch instance, imports it into the
    /// library, syncs it back, and re-imports. Run with
    /// `cargo test -- --ignored real_settings_roundtrip`. It only asserts
    /// that the *managed* parts survive — the rest is DSH's business.
    #[tokio::test]
    #[ignore]
    async fn real_settings_roundtrip() {
        let real = dirs::home_dir().map(|h| h.join(".dsh").join("settings.yaml"));
        let Some(real) = real.filter(|p| p.exists()) else {
            println!("no ~/.dsh/settings.yaml on this machine — nothing to probe");
            return;
        };
        let raw = std::fs::read_to_string(&real).unwrap();
        let dir = temp_instance("real");
        std::fs::write(settings_path(&dir), &raw).unwrap();

        let imported = import_inner(&dir).await.unwrap().expect("should parse");
        println!(
            "imported {} providers: {:?}",
            imported.providers.len(),
            imported.providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        );
        assert!(!imported.providers.is_empty());

        let applied = sync_inner(&dir, &ApiBinding::default(), &imported).await.unwrap();
        let after = import_inner(&dir).await.unwrap().unwrap();
        assert_eq!(after.providers.len(), imported.providers.len());
        // The import/sync loop must be stable on the user's real file.
        assert_eq!(after.default_model, imported.default_model);

        // Unknown keys must still be present verbatim.
        let out = std::fs::read_to_string(settings_path(&dir)).unwrap();
        for key in ["ui-theme", "agent-presets", "permission"] {
            if raw.contains(&format!("{key}:")) {
                assert!(out.contains(&format!("{key}:")), "{key} lost in roundtrip");
            }
        }
        println!("roundtrip OK; {applied:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn never_synced_managed_instance_is_not_dirty() {
        // The regression this guards: an instance created before the library
        // existed (binding, no file) must count as a clean batch-sync target,
        // not as "has local changes".
        let (root, _dir) = temp_root_with_instance("never-synced");
        let b = ApiBinding::default(); // inheritance default, syncedHash None
        let snap = snapshot_inner(&root, "never-synced", &b, &config()).await.unwrap();
        assert!(!snap.local_changes);
        assert!(!snap.default_model_changed);
        assert!(snap.live.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn snapshot_reports_live_state_and_local_changes() {
        let (root, dir) = temp_root_with_instance("snapshot");
        let c = config();
        // Snapshots compare against the *applied* binding — syncedHash is the
        // baseline gate for localChanges.
        let b = sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();

        // Fresh sync: the file matches, so a snapshot must not claim changes.
        let fresh = snapshot_inner(&root, "snapshot", &b, &c).await.unwrap();
        assert!(!fresh.local_changes);
        assert!(!fresh.default_model_changed);
        assert_eq!(fresh.live.as_ref().unwrap().providers.len(), 2);

        // A local DSH-side edit (tamper a managed provider's URL) and an
        // unmanaged local provider both must surface.
        let raw = tokio::fs::read_to_string(settings_path(&dir)).await.unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        {
            let key = |s: &str| serde_yaml::Value::from(s);
            doc.as_mapping_mut()
                .unwrap()
                .get_mut(key("llm-pi-ai"))
                .unwrap()
                .get_mut(key("providers"))
                .unwrap()
                .as_mapping_mut()
                .unwrap()
                .insert(key("local-only"), {
                    let mut m = serde_yaml::Mapping::new();
                    m.insert(key("apiKeyEnv"), "LOCAL_ONLY_KEY".into());
                    serde_yaml::Value::Mapping(m)
                });
            doc.as_mapping_mut()
                .unwrap()
                .get_mut(key("agent-default-model"))
                .unwrap()
                .as_mapping_mut()
                .unwrap()
                .insert(key("model"), "deepseek-v9".into());
        }
        tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
            .await
            .unwrap();
        let snap = snapshot_inner(&root, "snapshot", &b, &c).await.unwrap();
        assert!(snap.local_changes, "tampered managed provider surfaces");
        assert!(snap.default_model_changed, "edited default model surfaces");
        let live = snap.live.unwrap();
        let names: Vec<&str> = live.providers.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"local-only"), "live view shows the file, not the library");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_binding_gates_none_and_missing_library() {
        // Unmanaged bindings must be a no-op even when a library exists.
        let (root, dir) = temp_root_with_instance("gate-none");
        std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
        std::fs::write(api_config_path(&root), serde_json::to_string(&config()).unwrap())
            .unwrap();
        let none = ApiBinding {
            inheritance: "none".into(),
            provider_ids: vec![],
            default_model: None,
            synced_at: None,
            synced_hash: None,
        };
        assert!(apply_create_binding(&root, &dir, "gate-none", none).await.is_none());
        assert!(!settings_path(&dir).exists(), "unmanaged means never written");

        // Managed but no library: skipped, and the binding stays without a
        // baseline so the launch-time materialization can take over later.
        let _ = std::fs::remove_file(api_config_path(&root));
        let out = apply_create_binding(&root, &dir, "gate-none", ApiBinding::default()).await;
        assert!(out.is_none());
        assert!(!settings_path(&dir).exists());

        // Library returns: the same call materializes and persists.
        std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
        std::fs::write(api_config_path(&root), serde_json::to_string(&config()).unwrap())
            .unwrap();
        let out = apply_create_binding(&root, &dir, "gate-none", ApiBinding::default()).await;
        assert!(out.is_some());
        assert!(settings_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn unmanaged_snapshot_is_readonly_truth() {
        let (root, dir) = temp_root_with_instance("unmanaged");
        std::fs::write(
            settings_path(&dir),
            "llm-pi-ai:\n  providers:\n    solo:\n      apiKeyEnv: SOLO_KEY\n",
        )
        .unwrap();
        let b = ApiBinding {
            inheritance: "none".into(),
            provider_ids: vec![],
            default_model: None,
            synced_at: None,
            synced_hash: None,
        };
        let snap = snapshot_inner(&root, "unmanaged", &b, &config()).await.unwrap();
        assert!(!snap.local_changes);
        assert_eq!(snap.live.unwrap().providers.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sync_refuses_unmanaged() {
        let dir = temp_instance("none");
        let b = ApiBinding {
            inheritance: "none".into(),
            ..Default::default()
        };
        assert!(sync_inner(&dir, &b, &config()).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
