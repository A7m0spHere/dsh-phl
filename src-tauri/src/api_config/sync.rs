//! Instance-side API materialization: merging the global library into an
//! instance's `settings.yaml`, reading it back, and the live-snapshot view
//! the UI diffs against.

use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;

use super::{
    bound_provider_ids, instance_env, settings_path, valid_provider_name, ApiBinding, ApiConfig,
    DefaultModel, ModelRef, Provider, AGENT_DEFAULT_MODEL, LLM_SECTION,
};
use crate::credentials::{CredentialStore, Creds};
use crate::paths::PhlState;

/* ------------------------------ resolution ----------------------------- */

/// The materialization unit: the exact provider map + default-model value the
/// binding produces for a library state. Deterministic, so the hash doubles as
/// the drift fingerprint and the merge payload.
pub(crate) fn resolve_sections(
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
        for model in &p.models {
            model.validate_capabilities()?;
        }
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
                    if let Some(input) = &m.input {
                        mm.insert(
                            "input".into(),
                            serde_yaml::to_value(input).expect("input is serializable"),
                        );
                    }
                    if let Some(efforts) = &m.reasoning_efforts {
                        mm.insert(
                            "reasoningEfforts".into(),
                            serde_yaml::to_value(efforts).expect("efforts are serializable"),
                        );
                    }
                    serde_yaml::Value::Mapping(mm)
                })
                .collect();
            entry.insert("models".into(), serde_yaml::Value::Sequence(models));
        }
        map.insert(p.name.as_str().into(), serde_yaml::Value::Mapping(entry));
    }

    let default = binding
        .default_model
        .as_ref()
        .or(config.default_model.as_ref());
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
pub(crate) fn sections_hash(
    llm: &serde_yaml::Mapping,
    default_model: &serde_yaml::Value,
) -> String {
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
pub(crate) fn merge_sections(
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

pub(crate) async fn read_settings(dir: &Path) -> Result<serde_yaml::Value, String> {
    let path = settings_path(dir);
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_yaml::Value::Null),
        Err(e) => return Err(e.to_string()),
    };
    serde_yaml::from_str(&raw).map_err(|e| format!("实例 settings.yaml 解析失败: {e}"))
}

#[tauri::command]
pub async fn sync_instance_api(
    phl: State<'_, PhlState>,
    instance_id: String,
    binding: ApiBinding,
    config: ApiConfig,
) -> Result<ApiBinding, String> {
    let dir = crate::instances::instance_dir(&phl.root(), &instance_id)?;
    // Sync rewrites the instance's `settings.yaml` — a home write. External
    // instances own their own settings (that is the whole point of adopting
    // in place), so PHL refuses rather than reaching into the user's file.
    if let Some(manifest) = crate::instances::read_manifest(&dir).await {
        crate::instances::reject_external_write(&manifest, &instance_id, "同步全局 API 配置进")?;
    }
    sync_inner(&dir, &binding, &config).await
}

pub(crate) async fn sync_inner(
    dir: &Path,
    binding: &ApiBinding,
    config: &ApiConfig,
) -> Result<ApiBinding, String> {
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
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("yaml.tmp");
    tokio::fs::write(&tmp, &body)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| e.to_string())?;

    let mut applied = binding.clone();
    applied.synced_at = Some(crate::versions::now_iso());
    applied.synced_hash = Some(sections_hash(&llm, &default_model));
    Ok(applied)
}

/// Reads an instance's live settings.yaml back into library-shaped providers
/// — the seed flow for "I already configured this instance in the DSH UI".
#[tauri::command]
pub async fn import_instance_api(
    phl: State<'_, PhlState>,
    instance_id: String,
) -> Result<Option<ApiConfig>, String> {
    let dir = crate::instances::instance_dir(&phl.root(), &instance_id)?;
    import_inner(&dir).await
}

pub(crate) async fn import_inner(dir: &Path) -> Result<Option<ApiConfig>, String> {
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
        let Some(vm) = value.as_mapping() else {
            continue;
        };
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
                            input: m
                                .get("input")
                                .cloned()
                                .and_then(|v| serde_yaml::from_value(v).ok()),
                            reasoning_efforts: m
                                .get("reasoningEfforts")
                                .cloned()
                                .and_then(|v| serde_yaml::from_value(v).ok()),
                            ..Default::default()
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
            api_key_env: get_str("apiKeyEnv").unwrap_or_else(|| {
                format!(
                    "DSH_{}_API_KEY",
                    name.to_uppercase().replace(['-', '.'], "_")
                )
            }),
            // A live settings.yaml never contains the key itself (DSH keeps
            // it in credentials/env), so an import can only produce a name.
            api_key: None,
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
            model: m
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
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
    phl: State<'_, PhlState>,
    creds: State<'_, Creds>,
    instance_id: String,
    binding: ApiBinding,
    config: ApiConfig,
) -> Result<InstanceLiveSnapshot, String> {
    snapshot_inner(&phl.root(), &instance_id, &binding, &config, &creds).await
}

pub(crate) async fn snapshot_inner(
    root: &Path,
    instance_id: &str,
    binding: &ApiBinding,
    config: &ApiConfig,
    creds: &Creds,
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
        // A key is "present" when the library stores one for this provider
        // (launch injects it) or the environment already defines the
        // referenced variable — instance env counts too.
        let env_extra = instance_env(root, instance_id).await;
        for id in bound_provider_ids(config, binding) {
            let Some(p) = config.providers.iter().find(|p| p.id == id) else {
                continue;
            };
            let stored = p
                .api_key
                .as_deref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);
            let in_store = creds
                .get(&p.id)
                .ok()
                .flatten()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);
            if !stored
                && !in_store
                && std::env::var(&p.api_key_env).is_err()
                && !env_extra.contains_key(&p.api_key_env)
            {
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
