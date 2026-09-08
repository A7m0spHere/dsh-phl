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
//! Secrets live by reference, not by value: an entry names an environment
//! variable (`apiKeyEnv`, DSH's own field), so the key is delivered from the
//! user's environment or DSH's `.credentials.yaml`, and only the *variable name*
//! is ever written into an instance's `settings.yaml` — so instance directories
//! and shareable packs stay secret-free (§12 strips credentials on export). PHL's
//! own library (`<root>/config/api.json`) may hold a user-entered key in
//! plaintext to deliver at launch (the `api_key` field below), which is a
//! deliberate product choice, not a leak into the shared/instance surfaces.
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) mod catalog;
mod launch_keys;
pub(crate) mod library;
pub(crate) mod models;
pub(crate) mod sync;
mod types;
mod validation;

pub(crate) use launch_keys::{bound_provider_ids, instance_env, resolve_launch_keys};
pub use types::{
    ApiBinding, ApiConfig, DefaultModel, FalseValue, MetadataSource, ModelInput, ModelRef,
    Provider, ReasoningEfforts,
};
pub(crate) use validation::{valid_env_name, valid_provider_name};

use sync::{merge_sections, read_settings, resolve_sections, sync_inner};

/// Key in `settings.yaml` holding the model catalog / providers.
const LLM_SECTION: &str = "llm-pi-ai";
const AGENT_DEFAULT_MODEL: &str = "agent-default-model";

/* ------------------------------ commands ------------------------------ */

fn api_config_path(root: &Path) -> PathBuf {
    root.join("config").join("api.json")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("dsh-home").join("settings.yaml")
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
mod tests;
