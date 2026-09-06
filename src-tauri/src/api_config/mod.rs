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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialStore, Creds};

pub(crate) mod library;
pub(crate) mod models;
pub(crate) mod sync;

use sync::{merge_sections, read_settings, resolve_sections, sync_inner};

/// Key in `settings.yaml` holding the model catalog / providers.
const LLM_SECTION: &str = "llm-pi-ai";
const AGENT_DEFAULT_MODEL: &str = "agent-default-model";

/* ----------------------------- wire types ----------------------------- */

// None of these carry `deny_unknown_fields`, deliberately. They describe files
// on disk (`api.json`, and the `api` block inside every `instance.json`), and a
// strict reader turns any version skew — a key written by a newer PHL, or a
// field name that does not match on one side — into a hard parse failure that
// erases the whole object. The `baseUrl`/`baseURL` mismatch did exactly that:
// it rejected the entire library instead of one field.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
    /// Spelled `baseURL`, not the `baseUrl` that `rename_all = "camelCase"`
    /// would produce. That is DSH's own key in `settings.yaml` (see
    /// `provider_from_yaml`) and what the whole frontend uses — and because
    /// this struct is `deny_unknown_fields`, the mismatch rejected the *entire*
    /// `ApiConfig` on every save of a provider that had an endpoint, not just
    /// this one field. The alias keeps any file already written the old way
    /// loadable.
    #[serde(
        default,
        rename = "baseURL",
        alias = "baseUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_url: Option<String>,
    /// Name of the environment variable DSH reads the key from — the
    /// *address* of the secret. Every provider needs one: it is what lands
    /// in `settings.yaml` as `apiKeyEnv`, and where launch-injection puts a
    /// locally stored key.
    pub api_key_env: String,
    /// An optional locally stored key. The product decision followed
    /// cc-switch here: users paste a real key into the form and the tool
    /// owns delivering it — at launch we inject it into the DSH child's
    /// environment as this provider's `apiKeyEnv` (only when neither the
    /// system nor the instance already defines that variable). settings.yaml
    /// still carries just the variable name, so no instance directory ever
    /// holds the secret; the local `api.json` does, in plaintext, exactly
    /// like cc-switch's SQLite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelRef>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct ApiBinding {
    /// 'default' = every enabled provider + global defaults; 'custom' = the
    /// listed subset with optional per-instance default model; 'none' = PHL
    /// hands the instance over to the user (sync refuses).
    ///
    /// A bare `#[serde(default)]` here would use `String::default()` — the
    /// empty string, *not* the struct's `Default` impl. A manifest whose `api`
    /// object omitted this key then read as inheritance `""`, which is neither
    /// "none" (so the instance still counted as managed) nor a known mode (so
    /// every sync failed with an empty, unexplainable error).
    #[serde(default = "default_inheritance")]
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

fn default_inheritance() -> String {
    "default".into()
}

impl Default for ApiBinding {
    fn default() -> Self {
        ApiBinding {
            inheritance: default_inheritance(),
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
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The provider name is pasted straight into a YAML key; keep it to the
/// characters DSH's own providers use.
pub(crate) fn valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/* ------------------------------ commands ------------------------------ */

fn api_config_path(root: &Path) -> PathBuf {
    root.join("config").join("api.json")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("dsh-home").join("settings.yaml")
}

pub(crate) async fn instance_env(root: &Path, instance_id: &str) -> HashMap<String, String> {
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
    serde_json::from_str::<EnvProbe>(&raw)
        .map(|p| p.env)
        .unwrap_or_default()
}

/// The provider ids a binding pulls from the library. Selection lives here
/// so the snapshot's missing-key check and launch-time key injection cannot
/// drift from what `resolve_sections` actually writes.
pub(crate) fn bound_provider_ids(config: &ApiConfig, binding: &ApiBinding) -> Vec<String> {
    match binding.inheritance.as_str() {
        "default" => config
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.clone())
            .collect(),
        "custom" => binding.provider_ids.clone(),
        _ => Vec::new(),
    }
}

/// `(envVarName, key)` pairs the launcher should inject for this binding:
/// bound providers with a locally stored, non-blank key. The launcher still
/// checks the inherited environment itself — a real system/instance variable
/// always beats the stored copy.
pub(crate) fn provider_launch_keys(
    config: &ApiConfig,
    binding: &ApiBinding,
) -> Vec<(String, String)> {
    let ids = bound_provider_ids(config, binding);
    config
        .providers
        .iter()
        .filter(|p| ids.iter().any(|id| id == &p.id))
        .filter_map(|p| {
            let key = p.api_key.as_deref()?.trim();
            if key.is_empty() {
                return None;
            }
            Some((p.api_key_env.clone(), key.to_string()))
        })
        .collect()
}

/// Launch-time key resolution with the credential store in the chain: a
/// one-time key carried on the config wins, then the OS credential store,
/// then nothing (the caller falls back to the system variable). Credentials
/// that cannot be read are skipped, not fatal — a broken store degrades to
/// today's env-var behavior.
pub(crate) async fn resolve_launch_keys(
    config: &ApiConfig,
    binding: &ApiBinding,
    creds: &Creds,
) -> Vec<(String, String)> {
    let mut out = provider_launch_keys(config, binding);
    let ids = bound_provider_ids(config, binding);
    for p in &config.providers {
        if !ids.iter().any(|id| id == &p.id) {
            continue;
        }
        if out
            .iter()
            .any(|(name, _): &(String, String)| name == &p.api_key_env)
        {
            continue;
        }
        let stored = creds
            .get(&p.id)
            .ok()
            .flatten()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false);
        if stored {
            let key = creds.get(&p.id).ok().flatten().unwrap_or_default();
            out.push((p.api_key_env.clone(), key));
        }
    }
    out
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
    let Some(binding) = manifest.api.clone() else {
        return;
    };
    if binding.inheritance == "none" || binding.synced_hash.is_none() {
        return;
    }
    let Some(config) = load_config_file(root).await else {
        return;
    };
    let Ok((llm, default_model)) = resolve_sections(&config, &binding) else {
        return;
    };
    let Ok(mut doc) = read_settings(dir).await else {
        return;
    };
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
    let raw = tokio::fs::read_to_string(api_config_path(root))
        .await
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// The env-var names PHL itself knows carry credentials: every provider's
/// `apiKeyEnv`, the exact name launch-injection puts a real key under. This
/// is the *explicit* half of the bundle env classification — no name
/// guessing involved — and it wins over the heuristic in `env_policy`.
pub(crate) async fn credential_env_names(root: &Path) -> HashSet<String> {
    match load_config_file(root).await {
        Some(config) => config
            .providers
            .iter()
            .map(|p| p.api_key_env.trim().to_ascii_uppercase())
            .filter(|n| !n.is_empty())
            .collect(),
        None => HashSet::new(),
    }
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
    use super::models::{
        api_error_message, fetch_models_inner, models_url, parse_models_body, ENV_MISSING,
    };
    use super::sync::{import_inner, sections_hash, snapshot_inner};
    use super::*;
    /// A credential store backed by the real OS on Windows; the test entries
    /// it touches are namespaced and cleaned up by each test that uses one.
    fn test_creds() -> Creds {
        Creds::platform_default()
    }

    fn provider(name: &str, env: &str) -> Provider {
        Provider {
            id: format!("p-{name}"),
            name: name.into(),
            kind: "aggregator".into(),
            notes: None,
            api: Some("openai-completions".into()),
            base_url: Some(format!("https://{name}.example/v1")),
            api_key_env: env.into(),
            api_key: None,
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
            providers: vec![
                provider("deepseek", "DEEPSEEK_API_KEY"),
                provider("zen", "ZEN_API_KEY"),
            ],
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
            default_model
                .as_mapping()
                .unwrap()
                .get(key("model"))
                .unwrap()
                .as_str(),
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
            default_model
                .as_mapping()
                .unwrap()
                .get(key("model"))
                .unwrap()
                .as_str(),
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
        assert!(
            out.contains("ui-theme: dark"),
            "unknown key survived: {out}"
        );
        assert!(out.contains("google:"), "existing provider survived: {out}");
        assert!(out.contains("deepseek:"), "bound provider injected: {out}");
        assert!(
            out.contains("deepseek-v4"),
            "default model overwritten: {out}"
        );
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
        assert!(
            out.contains("keep-me"),
            "null default must not erase: {out}"
        );
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
        assert_eq!(
            imported.default_model.as_ref().unwrap().model,
            "deepseek-v4"
        );

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
        let raw = tokio::fs::read_to_string(settings_path(&dir))
            .await
            .unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        doc.as_mapping_mut()
            .unwrap()
            .insert("ui-theme".into(), "light".into());
        tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
            .await
            .unwrap();
        let mut doc = read_settings(&dir).await.unwrap();
        let before = serde_yaml::to_string(&doc).unwrap();
        merge_sections(&mut doc, &llm, &dm).unwrap();
        assert_eq!(
            before,
            serde_yaml::to_string(&doc).unwrap(),
            "unmanaged edit is not drift"
        );

        let raw = tokio::fs::read_to_string(settings_path(&dir))
            .await
            .unwrap();
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
        assert_ne!(
            before,
            serde_yaml::to_string(&doc).unwrap(),
            "managed edit is drift"
        );

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
            imported
                .providers
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        );
        assert!(!imported.providers.is_empty());

        let applied = sync_inner(&dir, &ApiBinding::default(), &imported)
            .await
            .unwrap();
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
        let snap = snapshot_inner(&root, "never-synced", &b, &config(), &test_creds())
            .await
            .unwrap();
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
        let fresh = snapshot_inner(&root, "snapshot", &b, &c, &test_creds())
            .await
            .unwrap();
        assert!(!fresh.local_changes);
        assert!(!fresh.default_model_changed);
        assert_eq!(fresh.live.as_ref().unwrap().providers.len(), 2);

        // A local DSH-side edit (tamper a managed provider's URL) and an
        // unmanaged local provider both must surface.
        let raw = tokio::fs::read_to_string(settings_path(&dir))
            .await
            .unwrap();
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
        let snap = snapshot_inner(&root, "snapshot", &b, &c, &test_creds())
            .await
            .unwrap();
        assert!(snap.local_changes, "tampered managed provider surfaces");
        assert!(snap.default_model_changed, "edited default model surfaces");
        let live = snap.live.unwrap();
        let names: Vec<&str> = live.providers.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"local-only"),
            "live view shows the file, not the library"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_binding_gates_none_and_missing_library() {
        // Unmanaged bindings must be a no-op even when a library exists.
        let (root, dir) = temp_root_with_instance("gate-none");
        std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
        std::fs::write(
            api_config_path(&root),
            serde_json::to_string(&config()).unwrap(),
        )
        .unwrap();
        let none = ApiBinding {
            inheritance: "none".into(),
            provider_ids: vec![],
            default_model: None,
            synced_at: None,
            synced_hash: None,
        };
        assert!(apply_create_binding(&root, &dir, "gate-none", none)
            .await
            .is_none());
        assert!(
            !settings_path(&dir).exists(),
            "unmanaged means never written"
        );

        // Managed but no library: skipped, and the binding stays without a
        // baseline so the launch-time materialization can take over later.
        let _ = std::fs::remove_file(api_config_path(&root));
        let out = apply_create_binding(&root, &dir, "gate-none", ApiBinding::default()).await;
        assert!(out.is_none());
        assert!(!settings_path(&dir).exists());

        // Library returns: the same call materializes and persists.
        std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
        std::fs::write(
            api_config_path(&root),
            serde_json::to_string(&config()).unwrap(),
        )
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
        let snap = snapshot_inner(&root, "unmanaged", &b, &config(), &test_creds())
            .await
            .unwrap();
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

    #[test]
    fn launch_keys_follow_the_binding_and_blankness() {
        let mut c = config();
        // deepseek has a stored key (with incidental whitespace), zen a blank one.
        c.providers.iter_mut().for_each(|p| {
            p.api_key = Some(match p.id.as_str() {
                "p-deepseek" => "  sk-live  ".into(),
                _ => "   ".into(),
            })
        });
        let keys = provider_launch_keys(&c, &ApiBinding::default());
        assert_eq!(
            keys,
            vec![("DEEPSEEK_API_KEY".to_string(), "sk-live".to_string())],
            "blank stored keys are no keys; names are trimmed of the value only"
        );

        // custom binding only pulls what it selects.
        let custom = ApiBinding {
            inheritance: "custom".into(),
            provider_ids: vec!["p-zen".into()],
            ..Default::default()
        };
        assert!(provider_launch_keys(&c, &custom).is_empty());

        // unmanaged bindings expose nothing to inject.
        let none = ApiBinding {
            inheritance: "none".into(),
            ..Default::default()
        };
        assert!(provider_launch_keys(&c, &none).is_empty());
    }

    #[tokio::test]
    async fn stored_key_satisfies_missing_env() {
        let (root, dir) = temp_root_with_instance("stored-key");
        let mut c = config();
        // Unique names so the test machine's environment cannot collide.
        c.providers.iter_mut().for_each(|p| {
            p.api_key_env = format!("PHL_TEST_UNSET_{}", p.name.to_uppercase());
        });
        c.providers[0].api_key = Some("sk-present".into());
        std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
        std::fs::write(api_config_path(&root), serde_json::to_string(&c).unwrap()).unwrap();
        let b = ApiBinding::default();
        let applied = sync_inner(&dir, &b, &c).await.unwrap();
        let snap = snapshot_inner(&root, "stored-key", &applied, &c, &test_creds())
            .await
            .unwrap();
        assert_eq!(
            snap.missing_keys,
            vec![format!(
                "PHL_TEST_UNSET_{}",
                c.providers[1].name.to_uppercase()
            )],
            "the provider with a stored key must not read as missing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn models_url_shapes() {
        assert_eq!(
            models_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/models"
        );
        // trailing version segment: never double up /v1/v1
        assert_eq!(
            models_url("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/models"
        );
        assert_eq!(
            models_url("https://g.example/paas/v2"),
            "https://g.example/paas/v2/models"
        );
        // bare host / missing scheme
        assert_eq!(
            models_url("api.deepseek.com"),
            "https://api.deepseek.com/v1/models"
        );
        // host with non-version path: treat as full base, append /v1/models —
        // a gateway that serves the API off a subpath is the user's problem,
        // the documented base always carries the version or is the root.
        assert_eq!(
            models_url("https://x.example/api"),
            "https://x.example/api/v1/models"
        );
        assert_eq!(models_url("  "), "");
    }

    #[test]
    fn parse_models_is_defensive_about_fields() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                { "id": "gpt-a", "object": "model", "created": 1, "owned_by": "o" },
                { "id": "with-name", "name": "With Name" },
                { "id": "anthropic-style", "display_name": "Claude Opus" },
                { "id": "both-fields", "name": "family", "display_name": "Human Label" },
                { "name": "no-id-dropped" },
                { "id": "  " },
                { "id": "extra", "unknown_field": { "nested": true } }
            ]
        });
        let out = parse_models_body(&body);
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "gpt-a",
                "with-name",
                "anthropic-style",
                "both-fields",
                "extra"
            ]
        );
        assert_eq!(out[2].name.as_deref(), Some("Claude Opus"));
        assert_eq!(
            out[3].name.as_deref(),
            Some("Human Label"),
            "display_name wins over name"
        );
        assert_eq!(out[0].name, None);
        // total garbage → empty, not an error
        assert!(parse_models_body(&serde_json::json!({ "models": [] })).is_empty());
    }

    #[test]
    fn api_error_prefers_provider_message() {
        let openai = r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error"}}"#;
        assert_eq!(
            api_error_message(401, openai),
            "端点返回错误 (401): Incorrect API key"
        );
        let plain = "Unauthorized";
        assert_eq!(api_error_message(401, plain), "端点返回 HTTP 401");
        let err_string = r#"{"error":"quota exceeded"}"#;
        assert!(api_error_message(429, err_string).contains("quota exceeded"));
    }

    #[tokio::test]
    async fn missing_env_key_short_circuits_with_prefix() {
        // No network is reached: the key question must fail fast, and the
        // ENV_MISSING marker is what the UI keys its temp-key input on.
        let e = fetch_models_inner(
            &test_creds(),
            "https://example.invalid/v1".into(),
            None,
            "PHL_TEST_DEFINITELY_UNSET_KEY".into(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(e.starts_with(ENV_MISSING), "unexpected error shape: {e}");
    }

    #[tokio::test]
    async fn empty_base_url_rejected() {
        assert!(fetch_models_inner(
            &test_creds(),
            "".into(),
            None,
            "X".into(),
            Some("k".into()),
            None
        )
        .await
        .is_err());
    }

    /// End-to-end against a throwaway local server: exercises the real
    /// reqwest path — URL joining, the Bearer header, and the tolerant parse
    /// — without depending on the public internet.
    #[tokio::test]
    async fn fetch_models_against_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            assert!(req.contains("get /v1/models"), "request line wrong:\n{req}");
            assert!(req.contains("bearer test-key"), "auth header wrong:\n{req}");
            let body = r#"{"object":"list","data":[{"id":"m1","created":1},{"id":"m2","display_name":"Two","unknown":true}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        let out = fetch_models_inner(
            &test_creds(),
            format!("http://{addr}"),
            None,
            "IRRELEVANT_WHEN_TEMP_KEY_GIVEN".into(),
            Some("test-key".into()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            out.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"]
        );
        assert_eq!(out[1].name.as_deref(), Some("Two"));
        server.await.unwrap();
    }

    /// Real-network probe against a public OpenAI-compatible endpoint that
    /// lists models without auth when possible. Run manually:
    /// `cargo test -- --ignored provider_models_probe`
    #[tokio::test]
    #[ignore]
    async fn provider_models_probe() {
        // DeepSeek's endpoint accepts any well-formed key for /models on some
        // deployments; if it 401s, the error text itself is the probe output.
        match fetch_models_inner(
            &test_creds(),
            "https://api.deepseek.com".into(),
            None,
            "DEEPSEEK_API_KEY".into(),
            None,
            None,
        )
        .await
        {
            Ok(models) => {
                println!(
                    "models: {} → {:?}",
                    models.len(),
                    models.iter().map(|m| &m.id).collect::<Vec<_>>()
                );
            }
            Err(e) => println!("probe reported: {e}"),
        }
    }
}
