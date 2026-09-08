//! Wire and persistence types for the global API library and instance binding.

use serde::{Deserialize, Serialize};

/* ----------------------------- wire types ----------------------------- */

// None of these carry `deny_unknown_fields`, deliberately. They describe files
// on disk (`api.json`, and the `api` block inside every `instance.json`), and a
// strict reader turns any version skew — a key written by a newer PHL, or a
// field name that does not match on one side — into a hard parse failure that
// erases the whole object. The `baseUrl`/`baseURL` mismatch did exactly that:
// it rejected the entire library instead of one field.

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<ModelInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<ReasoningEfforts>,
    /// Per-field provenance is PHL-only; never materialized into DSH YAML.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metadata_sources: std::collections::BTreeMap<String, MetadataSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelInput {
    Text,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReasoningEfforts {
    Disabled(FalseValue),
    Levels(std::collections::BTreeMap<String, Option<String>>),
}

/// An untagged boolean would also accept `true`, which DSH does not support.
#[derive(Debug, Clone, PartialEq)]
pub struct FalseValue;
impl Serialize for FalseValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(false)
    }
}
impl<'de> Deserialize<'de> for FalseValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if bool::deserialize(d)? {
            Err(serde::de::Error::custom(
                "reasoningEfforts must be false or a map",
            ))
        } else {
            Ok(Self)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetadataSource {
    #[serde(rename = "models.dev")]
    ModelsDev,
    #[serde(rename = "fallback")]
    Fallback,
    #[serde(rename = "manual")]
    Manual,
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
