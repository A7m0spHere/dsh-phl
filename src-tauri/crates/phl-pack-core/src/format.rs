//! The `phlpack.json` manifest schema (development spec part 7-8) and the
//! semantic rules both export (P1-3) and install (P1-4) share.
//!
//! The schema is designed independently of PHL's own `instance.json` and is
//! versioned on its own `formatVersion` axis, per the spec's requirement that
//! the pack format be its own versioned contract. Every field beyond the
//! spec's minimum is optional so a hand-written or third-party pack that only
//! supplies the required core still installs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The pack format this build reads/writes. Bump only with a migration story;
/// older must keep parsing, newer must be refused (see `PackError::UnsupportedVersion`).
pub const PACK_FORMAT_VERSION: u64 = 1;

/// The `dsh` dependency section (spec §7.2): at least a version; a source hint
/// and a content hash are recorded when PHL can supply them, and are advisory
/// (the resolver decides installability, not these bytes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDsh {
    /// The DSH version this environment was built on (bare semver, as PHL's
    /// versions directory names it). Required.
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// sha256 hex of the version payload, when the exporter knows it — used to
    /// prefer an identical already-installed version over a re-download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// The runtime dependency (spec §7.3): PHL fetches by its own catalog, so only
/// the identifying fields travel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackRuntime {
    /// Runtime kind; `node` today. Absent is read as node.
    #[serde(default)]
    pub kind: Option<String>,
    /// The Node version (or major) the environment expects.
    #[serde(default)]
    pub node_version: Option<String>,
    /// Architecture the pack was produced on (`x64`, `arm64`, …) — a mismatch
    /// is surfaced in the preview, never silently patched.
    #[serde(default)]
    pub arch: Option<String>,
}

/// A plugin's origin (spec §8). Remote plugins carry only a registry id and are
/// re-downloaded at install; embedded plugins point at a packed directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PluginSource {
    #[serde(rename_all = "camelCase")]
    Registry { registry_id: String },
    #[serde(rename_all = "camelCase")]
    Embedded { path: String },
}

/// One plugin entry (spec §8). `required` gates install when a remote plugin
/// turns out unresolvable (spec §20: a required missing plugin blocks install).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackPlugin {
    pub id: String,
    #[serde(default)]
    pub version: String,
    pub source: PluginSource,
    /// A plugin the environment needs to function; default true when omitted so
    /// an unlabelled pack errs toward refusing rather than half-installing.
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

/// The `content` + privacy block (spec §7 minimum, §11, §33). `sessionsIncluded`
/// is the headline privacy signal; the finer fields record what the exporter
/// knew at build time so the installer can re-show it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackContent {
    #[serde(default)]
    pub sessions_included: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_count: Option<usize>,
    /// Secrets (API keys, credentials) were stripped by the exporter; the
    /// installer uses this to prompt for re-config (spec §12). Always true for
    /// PHL-produced packs; a third-party pack that omits it is treated as
    /// un-vetted and its credential env names are still filtered on import.
    #[serde(default)]
    pub secrets_excluded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy: Option<PackPrivacy>,
}

/// Free-form privacy metadata shown in the preview (spec §11, §33).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackPrivacy {
    /// Whether the exporter surfaced the "history may contain secrets" warning.
    #[serde(default)]
    pub warning_acknowledged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Integrity map: archive-relative path → sha256 hex (spec §23). Covers every
/// payload file the pack ships (embedded plugin files, session logs, override
/// files); the manifest and this map are themselves excluded.
pub type PackIntegrity = BTreeMap<String, String>;

/// The whole `phlpack.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhlPackManifest {
    pub format_version: u64,
    pub pack: PackMeta,
    #[serde(default)]
    pub dsh: PackDsh,
    #[serde(default)]
    pub runtime: PackRuntime,
    #[serde(default)]
    pub plugins: Vec<PackPlugin>,
    /// Override files relative to the archive root (empty list allowed).
    #[serde(default)]
    pub overrides: Vec<String>,
    #[serde(default)]
    pub content: PackContent,
    /// Optional; when present the installer verifies these hashes byte-for-byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<PackIntegrity>,
    /// A Bundle-compatible environment section (spec §15/§24): the same
    /// credential-stripped `InstanceBundle` the current Bundle importer already
    /// understands, so a pack rebuilds the *config* (version, runtime, env
    /// minus secrets, plugin records) while the ZIP payload carries the files a
    /// Bundle never did. Optional so a hand-authored pack that ships only
    /// plugins+sessions still installs; when absent the installer takes the
    /// environment from `dsh`/`runtime`/`plugins` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentSection>,
}

/// The embedded environment view. Deliberately a loose `serde_json::Value` at
/// this layer (it is an `InstanceBundle` written by the export side and read by
/// the install side, both of which know the concrete type); keeping it opaque
/// here means a pack produced by a newer export does not fail to *parse* into
/// this build — only the installer that understands it consumes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentSection(pub serde_json::Value);

/// The `pack` identity block (spec §7.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: String,
    /// Archive-relative icon path, when the pack ships one (assets/icon.…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The one semantic rulebook shared by export and install. Returns `Ok` only
/// for a manifest that is internally coherent; the message is user-facing and
/// is mapped to `PackError::Invalid` / the export path alike.
pub fn validate_manifest(m: &PhlPackManifest) -> Result<(), String> {
    if m.format_version == 0 {
        return Err("phlpack.json 缺少 formatVersion".to_string());
    }
    if m.format_version > PACK_FORMAT_VERSION {
        return Err(format!(
            "整合包格式版本 {} 超出当前支持的 {PACK_FORMAT_VERSION}",
            m.format_version
        ));
    }
    if m.pack.id.trim().is_empty() {
        return Err("整合包缺少 id".to_string());
    }
    if m.pack.name.trim().is_empty() {
        return Err("整合包缺少名称".to_string());
    }
    if m.pack.version.trim().is_empty() {
        return Err("整合包缺少版本号".to_string());
    }
    if m.dsh.version.trim().is_empty() {
        return Err("整合包未声明 DSH 依赖版本".to_string());
    }
    let mut ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for p in &m.plugins {
        if p.id.trim().is_empty() {
            return Err("存在缺少 id 的插件条目".to_string());
        }
        if !ids.insert(p.id.as_str()) {
            return Err(format!("插件 id 重复: {}", p.id));
        }
        match &p.source {
            PluginSource::Registry { registry_id } => {
                if registry_id.trim().is_empty() {
                    return Err(format!("远程插件 {} 未声明 registryId", p.id));
                }
            }
            PluginSource::Embedded { path } => {
                let clean = path.replace('\\', "/");
                if clean.trim().is_empty() {
                    return Err(format!("内置插件 {} 未声明 path", p.id));
                }
                // An embedded path must be relative, forward-slashed, and stay
                // inside the pack; refuse absolute or traversal at the manifest
                // layer so a bad path never reaches the resolver.
                if clean.starts_with('/') || looks_like_root(&clean) {
                    return Err(format!("内置插件 {} 的 path 必须是相对路径", p.id));
                }
                if clean.split('/').any(|c| c == "..") {
                    return Err(format!("内置插件 {} 的 path 含越界片段", p.id));
                }
            }
        }
    }
    Ok(())
}

fn looks_like_root(s: &str) -> bool {
    // Windows drive prefix (`C:/…`) — cheap detection without a Path round-trip
    // so the rule matches the string as it appears in the manifest.
    let b = s.as_bytes();
    b.len() >= 2 && b[1] == b':' && (b[0].is_ascii_alphabetic())
}

#[cfg(test)]
mod tests;
