//! The instance manifest (`instance.json`): its versioned schema, the
//! read/migrate/refuse classification, and the crash-safe writer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest_path;
use crate::api_config::ApiBinding;

/// How PHL relates to the instance's DSH_HOME. The discriminator for every
/// dangerous operation's safety story (development spec §25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementMode {
    /// PHL owns an isolated copy at `<instance>/dsh-home`; the source
    /// environment (if any) was left untouched.
    #[default]
    ManagedCopy,
    /// The DSH_HOME lives in place, outside PHL's tree (adopted "原地接入").
    /// PHL launches and reads it but must not rewrite it: destructive
    /// operations are gated on this mode (snapshot restore, plugin writes).
    External,
    /// Created by installing a `.phlpack` (P1). Structurally a managed copy
    /// with extra provenance; defined now so the field never churns later.
    PackInstalled,
}

impl ManagementMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ManagementMode::ManagedCopy => "managed-copy",
            ManagementMode::External => "external",
            ManagementMode::PackInstalled => "pack-installed",
        }
    }
}

/// How the instance came to exist. Pure provenance for display and the
/// adoption record; it never gates behaviour the way `ManagementMode` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceSource {
    /// Created from scratch through the create wizard.
    #[default]
    Created,
    /// Adopted from an existing local DSH (copy or in-place).
    Adopted,
    /// Installed from a `.phlpack` (P1).
    Phlpack,
}

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
    /// Whether PHL owns a copy of the DSH_HOME or manages it in place. Schema
    /// v2; a v1 manifest has no field and defaults to `ManagedCopy` — exactly
    /// right, because every v1 instance is a PHL-owned copy.
    #[serde(default)]
    pub management_mode: ManagementMode,
    /// How this instance came to exist. Defaults to `Created` for the same
    /// reason: pre-adoption manifests were all created.
    #[serde(default)]
    pub source: InstanceSource,
    /// The absolute DSH_HOME path, present *only* for `External` instances
    /// (their home is outside the instance tree, so it cannot be derived).
    /// Every other mode derives the home from the instance directory so a
    /// data-root relocation keeps working.
    #[serde(default)]
    pub external_home: Option<String>,
    /// Adoption provenance: the source environment's path and version at the
    /// moment of adoption. Kept for display and forensics; adoption refuses
    /// to touch the source again, so nothing here is trusted for paths.
    #[serde(default)]
    pub adopted_from: Option<AdoptedFrom>,
}

/// Where an instance was adopted from, as recorded at adoption time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptedFrom {
    pub dsh_home: String,
    #[serde(default)]
    pub detected_version: Option<String>,
    pub adopted_at: String,
    /// The `ManagementMode` string the user chose (`managed-copy` / `external`),
    /// kept as a string so the record reads plainly in `instance.json`.
    pub mode: String,
}

/// The DSH_HOME a manifest points at: an external instance names its own
/// absolute path; everything else keeps the isolated copy inside the
/// instance directory. Centralised so no module re-derives the layout.
pub(crate) fn home_of(dir: &Path, manifest: &InstanceManifest) -> PathBuf {
    match manifest.management_mode {
        ManagementMode::External => match manifest.external_home.as_deref() {
            Some(home) if !home.trim().is_empty() => PathBuf::from(home),
            // A v2 manifest that claims external but has no path is corrupt
            // in a way we can survive: fall back to the local copy rather
            // than launch against an empty path. The adoption writer always
            // sets it, so this only guards hand-edited files.
            _ => dir.join("dsh-home"),
        },
        ManagementMode::ManagedCopy | ManagementMode::PackInstalled => dir.join("dsh-home"),
    }
}

/// The active profile directory (the one owning `node_modules`), resolved
/// through `home_of` so external instances point at their real home.
pub(crate) fn profile_root_of(dir: &Path, manifest: &InstanceManifest) -> PathBuf {
    home_of(dir, manifest)
        .join("profiles")
        .join(&manifest.profile)
}

/// The manifest format this build reads and writes. Bump only together with
/// a migration story: older versions must keep parsing, and this build must
/// refuse (not guess at) anything written by a newer one.
///
/// v2 adds the adoption fields (`managementMode`, `source`, `externalHome`,
/// `adoptedFrom`). Every one carries a serde default whose meaning is "the
/// v1 state of the world" — a v1 instance is by definition a PHL-owned copy
/// that was created, not adopted — so a v1 manifest parses into v2 with no
/// on-disk rewrite beyond the stamp `write_manifest` already performs. That
/// is the whole migration: additive fields with backward defaults, which is
/// why older files keep loading and this build still refuses (via
/// `classify_manifest`) anything stamped *above* it.
pub(crate) const MANIFEST_SCHEMA_VERSION: u32 = 2;

/// Makes each manifest write's temp file unique within the process (see
/// `write_manifest`); combined with the pid it also separates concurrent PHL
/// instances sharing a data root.
static MANIFEST_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
/// A temp name no other writer can share, and that a copy must not carry:
/// `write_manifest` writes here and renames onto `instance.json`.
fn manifest_tmp_path(dir: &Path) -> PathBuf {
    dir.join(format!(
        ".phl-manifest-{}-{}.tmp",
        std::process::id(),
        MANIFEST_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

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
    //
    // The temp name is unique per writer: `save_instance` holds the instance
    // resource lock, but the launch path's API sync does not, so two writers
    // can overlap. A shared `instance.json.tmp` let them interleave their
    // bytes and then rename each other's half-written file into place. The
    // `.phl-` prefix keeps a crashed temp file out of clones and snapshots
    // (see `copy::skipped`).
    let tmp = manifest_tmp_path(dir);
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("无法写入实例清单: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_temp_names_are_unique_and_invisible_to_copies() {
        // Two writers (save_instance under the instance lock, and the launch
        // path's API sync, which takes none) used to share `instance.json.tmp`
        // and could interleave their bytes before renaming.
        let dir = Path::new("phl").join("instances").join("a");
        let first = manifest_tmp_path(&dir);
        let second = manifest_tmp_path(&dir);
        assert_ne!(first, second, "two writers must not share a temp file");

        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            crate::instances::copy::skipped(&name),
            "a temp file left by a crash must not ride into a clone or snapshot: {name}"
        );
        assert_eq!(first.parent(), Some(dir.as_path()));
    }
}
