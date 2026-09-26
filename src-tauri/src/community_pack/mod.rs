//! Community integration-pack import (R1 · M4): DSH-PackForge `.dspack`
//! containers v2/v3 with manifest v4/v5, `type: "profile"` subset.
//!
//! Detection is content-based (a plain ZIP whose root `dspack.json` names
//! the container version); extensions are a file-picker convenience only.
//! The manifest is the single source of truth for machine files — the
//! archive's `package.json` / `pnpm-*` are snapshots that never bypass the
//! plan. Installation is two-phase: create the instance from the archive
//! (overrides → web profile, `home/` → HOME root), then materialise the
//! pinned npm/git dependencies with the same fixed-argv, script-less npm
//! pipeline the version installer uses. A phase-B failure keeps the
//! instance registered as `needsDependencies` and is retriable in place;
//! nothing ever creates a second instance for a retry.
//!
//! Support matrix (see internal/acceptance/next-release/format-support.md):
//! dshhome, collection, non-empty `files[]`, manifest v2/v3 and every
//! unknown container/manifest version are recognized and refused — with
//! the reason — never silently degraded.
//!
//! Module layout: `plan` owns detection + the read-only preview, `install`
//! owns extraction / the authoritative machine-file rebuild / phase B.

pub(crate) mod install;
pub(crate) mod plan;

use serde::{Deserialize, Serialize};

/// Archive-level guards, aligned with the pack-core philosophy but scoped
/// to community profile packs (which are small: real market packs are
/// tens of KiB).
pub(crate) const MAX_PACK_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 20_000;
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_UNCOMPRESSED_TOTAL: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_SINGLE_ENTRY: u64 = 2 * 1024 * 1024 * 1024;

/* ------------------------------- manifest ------------------------------- */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommunityManifest {
    pub manifest_version: u32,
    /// `profile` (v4 may omit it), `dshhome`, `collection`…
    #[serde(default)]
    pub r#type: Option<String>,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    #[serde(default)]
    pub author: Option<serde_json::Value>,
    #[serde(default)]
    pub profile_name: Option<String>,
    /// Exact DSH pin; empty string or absent means "user must pick".
    #[serde(default)]
    pub dsh_version: Option<String>,
    /// Ordered bundle stack — order is semantics.
    #[serde(default)]
    pub bundles: Vec<String>,
    /// Coordinate → exact pin (`"name": "1.2.3"`, `"github:owner/repo": "<sha>"`).
    #[serde(default)]
    pub dependencies: std::collections::BTreeMap<String, String>,
    /// Text patch layer; `overrides/cordis.patch.yml` takes priority.
    #[serde(default)]
    pub patch: Option<String>,
    /// Download manifest — non-empty is blocked in R1 (the wire field is an
    /// array; empty is legal).
    #[serde(default)]
    pub files: Option<Vec<serde_json::Value>>,
}

/* ------------------------------ wire types ----------------------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDependency {
    pub coordinate: String,
    pub pin: String,
    /// `npm` | `git`
    pub kind: String,
    /// `template` bundles are in `bundles` but not `dependencies`; they
    /// resolve from the DSH version's own tree.
    pub in_bundles: bool,
    /// The npm package name this dependency materialises under. For git
    /// coordinates this is resolved from the pack's own bundle stack — a
    /// repo's last path segment is NOT the package name by contract.
    pub package_name: String,
    /// `full` | `short-unresolved` — short SHAs resolve at prepare time.
    pub pin_state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityPackPreview {
    pub format: String,
    pub container_version: u32,
    pub manifest_version: u32,
    pub pack_type: String,
    pub name: String,
    pub version: String,
    pub display_name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub profile_name: String,
    /// Exact version the pack pins, when it names one.
    pub dsh_version: Option<String>,
    pub bundles: Vec<String>,
    /// Bundles that resolve from the DSH version's own tree (in `bundles`,
    /// not in `dependencies`).
    pub template_bundles: Vec<String>,
    pub dependencies: Vec<PackDependency>,
    /// Where the patch layer comes from.
    pub patch_source: String,
    pub overrides_count: usize,
    pub home_count: usize,
    /// SHA-256 of the whole pack file — the plan binds to it; installing
    /// re-checks it so changed bytes force a re-preview (D12).
    pub pack_sha256: String,
    pub pack_size: u64,
    /// Non-empty → install refuses with these reasons.
    pub blocked: Vec<String>,
    pub warnings: Vec<String>,
    /// Archive entries that map onto the same target path from different
    /// namespaces (D07/D13): no contract → refused before anything lands.
    pub target_collisions: Vec<String>,
}

pub(crate) fn description_text(v: &Option<serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(map @ serde_json::Value::Object(_)) => {
            // Multilingual map: zh first, then en, then any.
            for key in ["zh", "zh-CN", "en"] {
                if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
                    return Some(s.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn author_text(v: &Option<serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Object(map)) => {
            map.get("name").and_then(|n| n.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

/// Import-side security re-check (the export side filters, but the import
/// never trusts that): entries under overrides//home/ matching these rules
/// block the whole import with the paths listed (D16).
pub(crate) fn forbidden_entry(rel: &str) -> Option<&'static str> {
    let lower = rel.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if lower.ends_with(".zip")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".dspack")
    {
        return Some("嵌套压缩包");
    }
    if name.starts_with(".env")
        && !name.contains(".example")
        && !name.contains(".sample")
        && !name.contains(".template")
        && !name.contains(".dist")
    {
        return Some(".env 类凭据文件");
    }
    for ext in [".key", ".pem", ".p12", ".pfx", ".crt", ".der", ".asc"] {
        if name.ends_with(ext) {
            return Some("密钥/证书文件");
        }
    }
    if name == "credentials.json"
        || name == "credentials.yml"
        || name == "credentials.yaml"
        || name == ".credentials.yaml"
    {
        return Some("凭据文件");
    }
    if name.starts_with("id_rsa")
        || name.starts_with("id_dsa")
        || name.starts_with("id_ecdsa")
        || name.starts_with("id_ed25519")
    {
        return Some("SSH 私钥");
    }
    if name == "token.json" {
        return Some("token 文件");
    }
    if lower.split('/').any(|seg| seg == "node_modules") {
        return Some("node_modules（依赖必须按 manifest 重建）");
    }
    None
}

/// Entry-path safety, mirroring the pack-core rules: no `..`, absolute
/// prefixes, drive letters, ADS colons, trailing dot/space, reserved names.
pub(crate) fn unsafe_entry(rel: &str) -> bool {
    let p = rel.replace('\\', "/");
    if p.is_empty() || p.starts_with('/') || p.contains("//") {
        return true;
    }
    let mut segments = p.split('/').peekable();
    while let Some(seg) = segments.next() {
        if seg == "." && segments.peek().is_some() {
            continue;
        }
        if seg == ".." || seg.is_empty() {
            return true;
        }
        if seg.contains(':') {
            return true; // ADS / drive prefix
        }
        if seg.chars().last().is_some_and(|c| c == '.' || c == ' ') {
            return true;
        }
        let upper = seg.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "LPT1"
                | "LPT2"
                | "LPT3"
        ) {
            return true;
        }
    }
    false
}

/// The relative-to-HOME target of an overrides/home entry, normalized to
/// forward slashes — the key collisions are detected on (CR-03).
pub(crate) fn target_rel_path(namespace: &str, rel: &str) -> Option<String> {
    match namespace {
        "overrides" => Some(format!("profiles/web/{}", rel.replace('\\', "/"))),
        "home" => Some(rel.replace('\\', "/")),
        _ => None,
    }
}

/// Machine-file paths the manifest owns outright (CR-02): wherever these
/// land under the profile root or HOME root, the archive copy is skipped
/// and the rebuilt file is the only one that exists. Nested copies (inside
/// plugin directories) are ordinary data.
pub(crate) fn is_authoritative_machine_path(namespace: &str, rel: &str) -> bool {
    let p = rel.replace('\\', "/");
    match namespace {
        // The profile's own package.json / pnpm files — rebuilt from the
        // manifest, never accepted from overrides/.
        "overrides" => matches!(
            p.as_str(),
            "package.json" | "pnpm-workspace.yaml" | "pnpm-lock.yaml"
        ),
        // A HOME-root package.json is a dshhome-style machine snapshot; the
        // profile form carries none, so an archive one is skipped as well.
        "home" => p == "package.json",
        _ => false,
    }
}

/// The import record persisted beside the manifest: what the plan was and
/// how far the install got. This is the honest difference between "instance
/// registered" and "dependencies ready" (D10), and the retry entry's source
/// of truth.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportMarker {
    #[serde(rename = "format")]
    pub format: String,
    pub pack_sha256: String,
    pub container_version: u32,
    pub manifest_version: u32,
    pub bundles: Vec<String>,
    /// The exact dependency set the plan generated for the profile
    /// package.json — phase B re-verifies the on-disk file against it
    /// before running npm (D15).
    pub planned_dependencies: serde_json::Map<String, serde_json::Value>,
    pub dependencies_failed: Vec<String>,
    /// `needsDependencies` | `ready`
    pub stage: String,
}

#[cfg(test)]
mod tests;
