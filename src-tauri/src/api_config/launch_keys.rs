//! Instance environment reads and launch-time credential resolution.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::types::{ApiBinding, ApiConfig};
use crate::credentials::{CredentialStore, Creds};

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

/// Launch-time key resolution, below the environment in the priority chain:
/// a one-time key carried on the config wins, then the OS credential store.
/// The *caller* decides the first rank — an instance env or a real system
/// variable of the same name is never overwritten (see the injector in
/// `launch::launch_local_inner`), so what this returns is only consulted for
/// names the environment does not already define. Credentials
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
        // One credential read per provider (§M5): the previous code fetched the
        // stored key twice — once to test non-emptiness, once to use it — so a
        // read could disagree with itself, and it hit the store redundantly.
        // Read once, then gate on non-empty. A read error or a blank value still
        // yields "skip this provider" exactly as before (unchanged degradation).
        if let Some(key) = creds.get(&p.id).ok().flatten() {
            if !key.trim().is_empty() {
                out.push((p.api_key_env.clone(), key));
            }
        }
    }
    out
}
