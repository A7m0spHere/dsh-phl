//! The instance manifest (`instance.json`): its versioned schema, the
//! read/migrate/refuse classification, and the crash-safe writer.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::manifest_path;
use crate::api_config::ApiBinding;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceManifest {
    /// On-disk format version, stamped by the backend on every write. Absent
    /// means the manifest predates schema versioning — same shape, migrated
    /// in memory on read and stamped on the next write (original kept as a
    /// `.bak`). A version from a *newer* PHL is never guessed at; see
    /// `classify_manifest`.
    #[serde(default)]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: Option<String>,
    pub kind: String,
    pub hue: u32,
    pub version_id: String,
    pub runtime_id: String,
    pub port: u32,
    pub auto_port: bool,
    pub profile: String,
    pub created_at: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub total_runtime: u64,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Binding to the global API provider library (`api_config`). `None` for
    /// manifests written before the feature existed — treated as unmanaged.
    #[serde(default)]
    pub api: Option<ApiBinding>,
}

/// A manifest plus everything derived from the directory it lives in.
/// The manifest format this build reads and writes. Bump only together with a
/// migration story: older versions must keep parsing, and this build must
/// refuse (not guess at) anything written by a newer one.
pub(crate) const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// What reading an `instance.json` actually found. The distinctions matter:
/// `Missing` means "not an instance at all", `Corrupt` is reclaimable junk,
/// and `UnsupportedSchema` is a *valid instance this build cannot parse* —
/// it must stay invisible to every destructive path.
pub(crate) enum ManifestRead {
    Ok(Box<InstanceManifest>),
    Missing,
    Corrupt(String),
    UnsupportedSchema { found: u32, supported: u32 },
}

pub(crate) async fn classify_manifest(dir: &Path) -> ManifestRead {
    let Ok(raw) = tokio::fs::read_to_string(manifest_path(dir)).await else {
        return ManifestRead::Missing;
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(e) => return ManifestRead::Corrupt(format!("JSON 无法解析: {e}")),
    };
    // The version is inspected on the raw JSON, before a full parse: a
    // manifest from a newer PHL may carry fields this struct would reject, and
    // that failure must report itself as "too new", not as corruption.
    let found = value
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if found > MANIFEST_SCHEMA_VERSION {
        return ManifestRead::UnsupportedSchema {
            found,
            supported: MANIFEST_SCHEMA_VERSION,
        };
    }
    match serde_json::from_value::<InstanceManifest>(value) {
        Ok(mut manifest) => {
            // Legacy manifests migrate in memory; the stamp reaches disk the
            // next time this manifest is written, with the original saved.
            manifest.schema_version = MANIFEST_SCHEMA_VERSION;
            ManifestRead::Ok(Box::new(manifest))
        }
        Err(e) => ManifestRead::Corrupt(e.to_string()),
    }
}

// Instance ids and profile names are pasted straight into a filesystem path —
// the whitelist lives in `paths::sanitize_segment`, shared with every module
// that builds paths from user-supplied ids.

pub(crate) async fn read_manifest(dir: &Path) -> Option<InstanceManifest> {
    match classify_manifest(dir).await {
        ManifestRead::Ok(manifest) => Some(*manifest),
        ManifestRead::Missing => None,
        ManifestRead::Corrupt(e) => {
            // Silently returning None here made the instance vanish from the
            // list *and* from the orphan view (which used to key off the file
            // merely existing), leaving the user no signal at all beyond their
            // instance being gone. It is now reported as reclaimable, and the
            // reason goes to the log.
            eprintln!("[phl] 无法解析 {}: {e}", manifest_path(dir).display());
            None
        }
        ManifestRead::UnsupportedSchema { found, supported } => {
            eprintln!(
                "[phl] {}: schema 版本 {found} 超出当前支持的 {supported}，已隐藏该实例（请升级 PHL）",
                manifest_path(dir).display()
            );
            None
        }
    }
}

/// Load a manifest for an id-addressed command. Unlike `read_manifest`, every
/// failure mode is an explicit error — in particular a schema this build
/// cannot parse must fail loudly instead of being overwritten by a save.
pub(crate) async fn load_manifest(dir: &Path, id: &str) -> Result<InstanceManifest, String> {
    match classify_manifest(dir).await {
        ManifestRead::Ok(manifest) => Ok(*manifest),
        ManifestRead::Missing => Err(format!("实例不存在或缺少清单: {id}")),
        ManifestRead::Corrupt(e) => Err(format!("实例 {id} 清单损坏: {e}")),
        ManifestRead::UnsupportedSchema { found, supported } => Err(format!(
            "实例 {id} 的清单 schema 版本过新: {found}（当前支持 {supported}），请升级 PHL 后再操作"
        )),
    }
}

/// Resolves an instance's active profile directory — the one owning
/// `node_modules` and `cordis.patch.yml` — from the instance id. Plugin
/// commands key on this id so the WebView never supplies a filesystem path.
pub(crate) async fn write_manifest(dir: &Path, manifest: &InstanceManifest) -> Result<(), String> {
    // The schema version is backend-owned: whatever the caller sent, the file
    // always records the format this build writes. Callers load their copy
    // from disk (already migrated) or author a fresh one — either way the
    // stamp must reflect this build, not round-trip stale state.
    let mut manifest = manifest.clone();
    manifest.schema_version = MANIFEST_SCHEMA_VERSION;

    let path = manifest_path(dir);
    // One-time backup when this write changes the on-disk schema — a legacy
    // manifest gaining its version stamp. If the upgrade write is somehow
    // interrupted, the `.bak` still carries the last readable state.
    if let Ok(raw) = tokio::fs::read_to_string(&path).await {
        let on_disk_version = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("schemaVersion").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;
        if on_disk_version != MANIFEST_SCHEMA_VERSION {
            let _ = tokio::fs::copy(&path, dir.join("instance.json.legacy.bak")).await;
        }
    }

    let body = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // truncated manifest — that would make the instance unreadable entirely.
    let tmp = dir.join("instance.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))
}
