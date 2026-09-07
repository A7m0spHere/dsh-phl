//! The `.phlpack` container format (development spec parts 5–8, 22–23, 15).
//!
//! A `.phlpack` is a ZIP-compatible container — deliberately the same shape as
//! `bundle`'s JSON records, but *self-contained*: it carries the plugin files
//! an adopted environment needs (embedded or re-downloadable), optional session
//! history, and an integrity manifest, so a pack can be handed to another PHL
//! and installed without a registry being reachable.
//!
//! This crate owns the format itself: parsing `phlpack.json`, the archive-level
//! safety checks (spec §22) that run before anything is ever written to disk,
//! the incremental builder (producing), and the confined extractor (consuming).
//! It is **host-free** on purpose — no Tauri, no instance store, no credential
//! policy — so the desktop app (`src-tauri/src/pack/{export,install}.rs`) and
//! the `phl-pack` CLI are thin shells over one implementation (spec §15:
//! "不要长期维护两套 Pack 实现").
//!
//! What the host layers keep (by design, not omission): which env names count
//! as secrets (the credential-store boundary stays in PHL), where an installed
//! pack's files land (instance staging), and how errors surface to the user
//! (stable `[code]` prefixes). The core reports *what is wrong with the bytes*.
//!
//! Container layout (spec §6.1):
//! ```text
//! example.phlpack
//! ├── phlpack.json         # the manifest below (formatVersion-gated)
//! ├── embedded/plugins/    # plugin directories bundled by path
//! ├── overrides/           # settings/patch overlays applied at install
//! ├── sessions/            # optional — DSH session logs (privacy-gated)
//! └── assets/              # optional — icon etc.
//! ```
//!
//! `formatVersion` is independent of the instance-manifest schema: a newer
//! pack than this build understands is refused, never guessed (the same
//! discipline `classify_manifest` applies to `instance.json`).

pub mod format;
pub mod unpack;
pub mod write;

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

pub use format::{
    validate_manifest, EnvironmentSection, PackContent, PackDsh, PackIntegrity, PackMeta,
    PackPlugin, PackPrivacy, PackRuntime, PhlPackManifest, PluginSource, PACK_FORMAT_VERSION,
};

/// Hard ceilings on an untrusted archive (spec §22 "oversized decompression").
/// A legitimate coding environment is far under these; they exist so a hostile
/// or corrupt pack cannot make install read an unbounded amount.
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_UNCOMPRESSED_TOTAL: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB
pub const MAX_SINGLE_ENTRY: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB
/// `phlpack.json` is a small metadata document, capped tightly.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024; // 1 MiB

/// The archive path that must exist and parse to be a pack at all.
pub const MANIFEST_NAME: &str = "phlpack.json";

/// Errors a `.phlpack` can raise purely from its bytes — every one is a
/// refusal, and none of them may leave a partially-written file behind (the
/// validator runs entirely before extraction, spec §21–22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    /// The host requested cancellation while a streaming operation was active.
    Cancelled,
    /// The archive cannot be opened as a zip at all.
    Unreadable(String),
    /// `phlpack.json` is absent.
    MissingManifest,
    /// The manifest is present but not valid UTF-8 or not valid JSON/shape.
    MalformedManifest(String),
    /// The manifest's `formatVersion` is newer than this build supports.
    UnsupportedVersion { found: u64, supported: u64 },
    /// A semantic manifest violation (missing id, bad plugin source, …).
    Invalid(String),
    /// An entry escapes the root, is absolute, has a `..`/prefix component, or
    /// carries a backslash separator (all refused before any write).
    PathTraversal(String),
    /// A symlink/special entry — a `.phlpack` is a data container, and a link
    /// could point outside the instance tree.
    SymlinkEntry(String),
    /// Two entries resolve to the same output path.
    DuplicateEntry(String),
    /// Too many entries, or total/single size over the ceiling.
    TooLarge(String),
    /// The manifest declares a plugin/embedded path or an integrity hash that
    /// the archive does not actually contain (or whose bytes hash differently).
    Consistency(String),
}

impl PackError {
    /// The user-facing explanation, free of any host error-code framing —
    /// each host decides how to surface it (PHL wraps it in a stable
    /// `[state]` tag at its command boundary).
    pub fn detail(&self) -> String {
        match self {
            PackError::Unreadable(s)
            | PackError::MalformedManifest(s)
            | PackError::Invalid(s)
            | PackError::PathTraversal(s)
            | PackError::SymlinkEntry(s)
            | PackError::DuplicateEntry(s)
            | PackError::TooLarge(s)
            | PackError::Consistency(s) => s.clone(),
            PackError::MissingManifest => "包内缺少 phlpack.json".to_string(),
            PackError::Cancelled => "cancelled".to_string(),
            PackError::UnsupportedVersion { found, supported } => {
                format!("整合包格式版本 {found} 超出当前支持的 {supported}，请升级 PHL")
            }
        }
    }
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail())
    }
}

impl std::error::Error for PackError {}

/// A fully-validated pack, ready for P1-4 to install. Nothing here may be
/// trusted for a filesystem path unless it passed [`read_pack`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedPack {
    pub manifest: PhlPackManifest,
    /// Normalised forward-slash entry names that exist in the archive.
    pub entries: Vec<String>,
    /// Embedded plugin directory names actually present under
    /// `embedded/plugins/` (used to cross-check the manifest).
    pub embedded_plugins: Vec<String>,
    /// Whether the archive physically carries `sessions/`.
    pub has_sessions: bool,
    /// Assets present (icon etc.), for the preview UI.
    pub assets: Vec<String>,
}

/// A safe relative path inside the pack: normalised, no traversal, forward
/// slashes. Rejects the shapes `ensure_under_root` would otherwise have to
/// catch at write time — doing it on the *name* is what lets validation run
/// entirely before any file is opened.
pub(crate) fn normalize_entry(name: &str) -> Result<PathBuf, PackError> {
    // A backslash is a valid path char on some systems but a separator in a
    // zip name — normalise to forward slashes, then reject the dangerous
    // components outright.
    let unified = name.replace('\\', "/");
    let mut out = PathBuf::new();
    for c in Path::new(&unified).components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            // ParentDir / RootDir / Prefix: an escape attempt or an absolute
            // entry. Refused outright, same reasoning as `safe_join`.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PackError::PathTraversal(format!(
                    "压缩包包含越界路径: {name}"
                )))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(PackError::PathTraversal(format!(
            "空的压缩包条目名: {name}"
        )));
    }
    Ok(out)
}

/// Read and fully validate a pack archive. Pure with respect to the caller's
/// filesystem: nothing is extracted, only the archive's central directory and
/// the manifest bytes are inspected. Used by P1-3's export round-trip and by
/// P1-4's install gate.
pub fn read_pack<R: Read + std::io::Seek + Send>(
    reader: R,
    total_bytes: u64,
) -> Result<ValidatedPack, PackError> {
    read_pack_with_cancel(reader, total_bytes, &|| false)
}

/// Cancellable form of [`read_pack`]. The callback is polled between archive
/// entries and for every fixed-size chunk hashed from an integrity-covered
/// payload, so a host can stop validation in the middle of a single large file.
pub fn read_pack_with_cancel<R, F>(
    reader: R,
    total_bytes: u64,
    cancel: &F,
) -> Result<ValidatedPack, PackError>
where
    R: Read + std::io::Seek + Send,
    F: Fn() -> bool + ?Sized,
{
    ensure_not_cancelled(cancel)?;
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| PackError::Unreadable(e.to_string()))?;
    let len = archive.len();
    if len > MAX_ENTRIES {
        return Err(PackError::TooLarge(format!(
            "压缩包条目数 {len} 超过上限 {MAX_ENTRIES}"
        )));
    }

    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut entries: Vec<String> = Vec::new();
    let mut embedded: BTreeSet<String> = BTreeSet::new();
    let mut assets: Vec<String> = Vec::new();
    let mut has_sessions = false;
    let mut total_uncompressed: u64 = 0;
    let mut manifest_bytes: Option<Vec<u8>> = None;

    for index in 0..len {
        ensure_not_cancelled(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| PackError::Unreadable(e.to_string()))?;
        let raw_name = entry.name().to_string();
        let path = normalize_entry(&raw_name)?;

        // Symlink / directory special entries: a pack must not smuggle links.
        if entry.is_symlink() {
            return Err(PackError::SymlinkEntry(raw_name));
        }
        let size = entry.size();
        if size > MAX_SINGLE_ENTRY {
            return Err(PackError::TooLarge(format!(
                "单个条目 {raw_name} 解压后 {size} 字节超过上限"
            )));
        }
        total_uncompressed = total_uncompressed.saturating_add(size);
        if total_uncompressed > MAX_UNCOMPRESSED_TOTAL {
            return Err(PackError::TooLarge(
                "压缩包解压后总大小超过上限".to_string(),
            ));
        }

        if !entry.is_dir() {
            if !seen.insert(path.clone()) {
                return Err(PackError::DuplicateEntry(raw_name));
            }
            entries.push(raw_name.clone());
            // Capture the manifest (bounded) for schema validation below.
            if path == Path::new(MANIFEST_NAME) {
                let mut buf = Vec::new();
                entry
                    .by_ref()
                    .take(MAX_MANIFEST_BYTES + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| PackError::Unreadable(e.to_string()))?;
                if buf.len() as u64 > MAX_MANIFEST_BYTES {
                    return Err(PackError::TooLarge("phlpack.json 过大".to_string()));
                }
                manifest_bytes = Some(buf);
            }
            if let Some(plugin) = embedded_plugin_under(&path) {
                embedded.insert(plugin);
            }
            if path.starts_with("sessions") {
                has_sessions = true;
            }
            if path.starts_with("assets") {
                if let Some(f) = path.file_name().and_then(|s| s.to_str()) {
                    assets.push(f.to_string());
                }
            }
        }
    }
    let _ = total_bytes; // kept for caller-side progress reporting; sizes guard above

    let manifest_bytes = manifest_bytes.ok_or(PackError::MissingManifest)?;
    let text = std::str::from_utf8(&manifest_bytes)
        .map_err(|e| PackError::MalformedManifest(format!("非 UTF-8: {e}")))?;
    let manifest: PhlPackManifest = parse_manifest(text)?;
    validate_manifest_schema(&manifest)?;

    // Cross-check: every embedded plugin the manifest names must physically
    // exist as a `package.json` under its declared path (spec §23 embedded file
    // presence). Integrity (declared sha256 vs the bytes actually in the
    // archive) is checked against the already-open archive in one pass.
    for plugin in &manifest.plugins {
        if let PluginSource::Embedded { path, .. } = &plugin.source {
            let probe = Path::new(path).join("package.json");
            if !entries
                .iter()
                .any(|e| normalize_entry(e).map(|p| p == probe).unwrap_or(false))
            {
                return Err(PackError::Consistency(format!(
                    "内置插件 {} 声明了路径 {path} 但包内没有 {path}/package.json",
                    plugin.id
                )));
            }
        }
    }
    if let Some(integrity) = &manifest.integrity {
        verify_integrity(&mut archive, integrity, cancel)?;
    }

    Ok(ValidatedPack {
        manifest,
        entries,
        embedded_plugins: embedded.into_iter().collect(),
        has_sessions,
        assets,
    })
}

/// Recompute sha256 over each entry named in the integrity map and compare
/// (spec §23 "SHA-256 … over embedded files"). A missing entry or a hash
/// mismatch is a corrupted/lying pack — refused before any byte is installed.
/// The manifest document itself and `phlpack.json` are not integrity-covered
/// (they are the signed-by-nobody header; the map is keyed by payload paths).
fn verify_integrity<R: Read + std::io::Seek, F: Fn() -> bool + ?Sized>(
    archive: &mut zip::ZipArchive<R>,
    integrity: &PackIntegrity,
    cancel: &F,
) -> Result<(), PackError> {
    // Build normalized-name → index ONCE. The previous version re-scanned all
    // `archive.len()` indices for every integrity key — O(entries²) on a fully
    // covered pack, which is exactly the common case (§R6). Later integrity keys
    // that don't resolve still error identically, just in O(1) per lookup.
    let mut index_of: HashMap<PathBuf, usize> = HashMap::new();
    for i in 0..archive.len() {
        ensure_not_cancelled(cancel)?;
        if let Ok(entry) = archive.by_index(i) {
            if let Ok(norm) = normalize_entry(entry.name()) {
                index_of.insert(norm, i);
            }
        }
    }
    for (name, want) in integrity {
        ensure_not_cancelled(cancel)?;
        let norm = normalize_entry(name).map_err(|_| PackError::Consistency(name.clone()))?;
        let index = *index_of.get(&norm).ok_or_else(|| {
            PackError::Consistency(format!("完整性校验引用了包内不存在的文件 {name}"))
        })?;
        let mut entry = archive
            .by_index(index)
            .map_err(|e| PackError::Consistency(e.to_string()))?;
        // Stream-hash in fixed buffers (no whole-entry `Vec`), capped by the
        // single-entry limit so a mis-declared size can't force a huge
        // allocation (§R6 memory peak).
        let got = sha256_of_reader_with_cancel(&mut entry, MAX_SINGLE_ENTRY, cancel)?;
        let want_clean = want.strip_prefix("sha256:").unwrap_or(want);
        if !got.eq_ignore_ascii_case(want_clean.trim()) {
            return Err(PackError::Consistency(format!(
                "文件 {name} 的校验值与内容不符，包可能已损坏或被篡改"
            )));
        }
    }
    Ok(())
}

/// Streaming SHA-256 over any `Read` (a `zip::ZipFile` qualifies), capped at
/// `max` actual bytes so an archive that under-declares an entry's size cannot
/// force an unbounded allocation. Replaces the previous "read the whole entry
/// into a `Vec`, then hash it" path in `verify_integrity` (§R6).
pub(crate) fn sha256_of_reader_with_cancel<R, F>(
    reader: &mut R,
    max: u64,
    cancel: &F,
) -> Result<String, PackError>
where
    R: Read,
    F: Fn() -> bool + ?Sized,
{
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65_536];
    let mut total = 0u64;
    loop {
        ensure_not_cancelled(cancel)?;
        let n = reader
            .read(&mut buf)
            .map_err(|e| PackError::Unreadable(format!("校验读取失败: {e}")))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > max {
            return Err(PackError::TooLarge(
                "条目解压后超过上限，校验中止".to_string(),
            ));
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// sha256-hex of arbitrary bytes — the primitive the export side records into
/// the integrity map and the validator side recomputes against.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub(crate) fn parse_manifest(text: &str) -> Result<PhlPackManifest, PackError> {
    // version-gate on raw JSON first, like classify_manifest: a too-new pack
    // may carry keys this struct would reject, and that must read as
    // "too new", not "corrupt".
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| PackError::MalformedManifest(e.to_string()))?;
    let found = value
        .get("formatVersion")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if found > PACK_FORMAT_VERSION {
        return Err(PackError::UnsupportedVersion {
            found,
            supported: PACK_FORMAT_VERSION,
        });
    }
    serde_json::from_value(value).map_err(|e| PackError::MalformedManifest(e.to_string()))
}

/// Structural manifest checks beyond serde (required id, non-empty version,
/// coherent plugin sources). Delegates the semantic half to the format module's
/// `validate_manifest` so export and install share one rulebook.
pub(crate) fn validate_manifest_schema(m: &PhlPackManifest) -> Result<(), PackError> {
    validate_manifest(m).map_err(PackError::Invalid)
}

/// Given a normalised entry path under `embedded/plugins/<name>/…`, return
/// `<name>` — the plugin directory identity. `None` for anything not under
/// that exact prefix (a bare `sessions/<p>/<id>/file` must not read as one).
pub(crate) fn embedded_plugin_under(path: &Path) -> Option<String> {
    let mut comps = path.components();
    match (comps.next(), comps.next(), comps.next()) {
        (Some(Component::Normal(a)), Some(Component::Normal(b)), Some(Component::Normal(name)))
            if a == "embedded" && b == "plugins" =>
        {
            name.to_str().map(str::to_string)
        }
        _ => None,
    }
}

/// Convenience: validate a pack sitting at `path` on disk (opens the file, reads
/// only the central directory). Returns the on-disk size too, for progress.
pub fn read_pack_from_path(path: &Path) -> Result<ValidatedPack, PackError> {
    read_pack_from_path_with_cancel(path, &|| false)
}

/// Cancellable on-disk validation; see [`read_pack_with_cancel`].
pub fn read_pack_from_path_with_cancel<F: Fn() -> bool + ?Sized>(
    path: &Path,
    cancel: &F,
) -> Result<ValidatedPack, PackError> {
    let file =
        std::fs::File::open(path).map_err(|e| PackError::Unreadable(format!("{path:?}: {e}")))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    read_pack_with_cancel(file, size, cancel)
}

pub(crate) fn ensure_not_cancelled<F: Fn() -> bool + ?Sized>(cancel: &F) -> Result<(), PackError> {
    if cancel() {
        Err(PackError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
