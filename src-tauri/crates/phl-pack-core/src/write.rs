//! `.phlpack` production: an incremental ZIP builder that records a sha256 of
//! every payload file it writes, so the manifest's integrity map describes
//! exactly the bytes that landed in the archive.
//!
//! The builder is deliberately dumb (add bytes under a name, add a directory
//! tree) and holds *no* domain knowledge — deciding *what* a pack contains is
//! the export command's job in the host. Keeping the ZIP mechanics here means
//! the deflate settings, the `phlpack.json`-last ordering, and the integrity
//! bookkeeping have one home, shared by the PHL GUI and the `phl-pack` CLI
//! (development spec §15: "不要长期维护两套 Pack 实现").
//!
//! Entries are always written with forward-slash names and `Deflated`
//! compression; a name that would escape the archive is rejected here too, so
//! even a bug in a caller cannot emit a traversal entry.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::{
    normalize_entry, sha256_hex, PackError, PhlPackManifest, ValidatedPack, MANIFEST_NAME,
};

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

/// Accumulates payload files + their hashes, then writes the archive.
pub struct PackBuilder {
    writer: ZipWriter<std::io::BufWriter<std::fs::File>>,
    integrity: BTreeMap<String, String>,
}

/// What a whole-tree add did: how many files went in, and which archive paths
/// were withheld because their names carry secrets (§12). The withheld list is
/// archive-relative (forward slashes) so the host can report it verbatim.
#[derive(Debug, Default)]
pub struct TreeAdd {
    pub added: usize,
    pub withheld_secrets: Vec<String>,
}

/// Filename-level secret detection for tree packing (development spec §12).
/// These names *are* the secrets by convention — `.env` (with or without a
/// profile suffix, but not the `*.example`/`*.sample`/`*.template` docs),
/// SSH private keys, netrc/npmrc/piprc/gh auth, and credential/token JSON
/// documents — so no content sniffing is needed (and none would be reliable;
/// spec §33 explicitly refuses to "recognize secrets in prose"). Applied to
/// every whole-tree add (embedded plugins, sessions, CLI `build`), it is
/// archive hygiene, not policy: a pack that promises `secretsExcluded` may
/// not carry a file whose very name promises credentials.
pub fn is_secret_entry_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // The `.env` family, minus the documented examples — an example file is
    // config documentation and belongs in the pack.
    if lower.starts_with(".env") {
        return !(lower.ends_with(".example")
            || lower.ends_with(".sample")
            || lower.ends_with(".template")
            || lower.ends_with(".dist"));
    }
    // Credential stores named by spec §12 (`.env`, registry token, credential
    // store, SSH key) — matched by exact filename only. Deliberately no
    // substring "token"/"key" test: it would silently drop legitimate plugin
    // payloads (e.g. `tokenizer.js`), the exact "missing half the env" failure
    // this project refuses. Anything subtler is the exporter's job, not here.
    matches!(
        lower.as_str(),
        ".netrc"
            | "_netrc"
            | ".npmrc"
            | ".pypirc"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | "credentials.json"
            | "credentials.yml"
            | "credentials.yaml"
            | "token.json"
    )
}

impl PackBuilder {
    pub fn create(dest: &Path) -> Result<Self, String> {
        let file = std::fs::File::create(dest)
            .map_err(|e| format!("无法创建整合包文件 {}: {e}", dest.display()))?;
        Ok(PackBuilder {
            writer: ZipWriter::new(std::io::BufWriter::new(file)),
            integrity: BTreeMap::new(),
        })
    }

    /// Add one payload file by explicit archive-relative name, returning its
    /// sha256. The name is validated the same way a read entry is.
    pub fn add_bytes(&mut self, archive_name: &str, bytes: &[u8]) -> Result<(), String> {
        normalize_entry(archive_name).map_err(|e| e.detail())?;
        let digest = sha256_hex(bytes);
        self.integrity
            .insert(archive_name.replace('\\', "/"), digest);
        self.writer
            .start_file(archive_name, options())
            .map_err(|e| format!("写入条目 {archive_name} 失败: {e}"))?;
        self.writer
            .write_all(bytes)
            .map_err(|e| format!("写入条目内容失败: {e}"))?;
        Ok(())
    }

    /// The integrity map accumulated so far (path → sha256). Exposed so a
    /// host that builds the manifest incrementally (or a CLI that reports
    /// what it packed) can inspect it without re-reading the archive.
    pub fn integrity(&self) -> &BTreeMap<String, String> {
        &self.integrity
    }

    /// Read a file from disk and add it, so callers never hold a whole plugin
    /// directory in memory.
    pub fn add_file_from_disk(&mut self, src: &Path, archive_name: &str) -> Result<(), String> {
        let mut buf = Vec::new();
        std::fs::File::open(src)
            .and_then(|mut f| f.read_to_end(&mut buf))
            .map_err(|e| format!("读取 {src:?} 失败: {e}"))?;
        self.add_bytes(archive_name, &buf)
    }

    /// Add a whole directory tree under an archive prefix, skipping symlinks
    /// (a pack is a data container; a link that points outside the tree could
    /// be re-created maliciously on install) **and** skipping secret-bearing
    /// files by name (development spec §12: `.env`/SSH private keys/credential
    /// stores never enter a pack — a plugin's own `.env` must not ship under a
    /// `secretsExcluded: true` promise, or that flag would lie). The skipped
    /// secret paths are returned so the host can surface them; the count
    /// returned is the number actually added.
    pub fn add_tree(&mut self, dir: &Path, archive_prefix: &str) -> Result<TreeAdd, String> {
        let mut added = 0;
        let mut secrets: Vec<String> = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        let prefix = archive_prefix.trim_end_matches('/');
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current)
                .map_err(|e| format!("遍历 {current:?} 失败: {e}"))?
                .flatten()
            {
                let path = entry.path();
                let meta = entry
                    .metadata()
                    .map_err(|e| format!("读取属性失败 {path:?}: {e}"))?;
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }
                if meta.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(dir)
                    .map_err(|_| "条目不在预期目录内".to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = format!("{prefix}/{rel}");
                if is_secret_entry_name(&rel) {
                    secrets.push(name);
                    continue;
                }
                self.add_file_from_disk(&path, &name)?;
                added += 1;
            }
        }
        Ok(TreeAdd {
            added,
            withheld_secrets: secrets,
        })
    }

    /// Write `phlpack.json` last, with the integrity map the builder assembled
    /// from the payload files, and finalise the archive.
    pub fn finish(mut self, mut manifest: PhlPackManifest) -> Result<(), String> {
        if self.integrity.is_empty() {
            manifest.integrity = None;
        } else {
            manifest.integrity = Some(self.integrity.clone());
        }
        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("序列化 phlpack.json 失败: {e}"))?;
        // The manifest itself is not integrity-covered (it is the thing that
        // carries the map), so it is written after the map is sealed.
        self.writer
            .start_file(MANIFEST_NAME, options())
            .map_err(|e| format!("写入 phlpack.json 失败: {e}"))?;
        self.writer
            .write_all(&json)
            .map_err(|e| format!("写入 phlpack.json 内容失败: {e}"))?;
        self.writer
            .finish()
            .map_err(|e| format!("完成整合包失败: {e}"))?;
        Ok(())
    }
}

/// Collect a payload tree's files (relative, forward-slash names), sorted for
/// a deterministic archive, skipping symlinks for the same reason the
/// per-tree add does — a pack is a data container — and withholding
/// secret-bearing filenames into `withheld` (the same §12 hygiene
/// `add_tree` applies, so CLI-built packs carry no `.env`/keys either).
fn collect_tree_files(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
    withheld: &mut Vec<String>,
) -> Result<(), PackError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| PackError::Unreadable(format!("读取打包目录 {dir:?} 失败: {e}")))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let next = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            collect_tree_files(&path, &next, out, withheld)?;
        } else {
            if is_secret_entry_name(&name) {
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                withheld.push(rel);
                continue;
            }
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Build a `.phlpack` from a staging directory laid out per spec §6.1
/// (`phlpack.json` + payload directories). This is the shared "Pack Builder"
/// the spec asks for (part 15): the PHL GUI export path assembles payloads
/// itself and drives [`PackBuilder`] directly, while the `phl-pack` CLI and
/// the DSH Skill use this whole-directory form — both produce archives that
/// pass the same validator, so there is one format and one rulebook.
///
/// The manifest's own `integrity` field is ignored (the builder re-hashes
/// exactly the bytes it writes), and the result is re-read through
/// [`crate::read_pack_from_path`]: a pack that its own installer would refuse
/// is never reported as built. §12 secret hygiene is identical to `add_tree`:
/// a `.env`/`id_rsa`/`token.json` in the *staging* directory is withheld from
/// the archive and reported through `withheld_secrets` so the caller can warn
/// the user "staging contained secret-shaped files; they were not packed".
pub fn build_pack_from_dir(
    src_dir: &Path,
    out: &Path,
    withheld_secrets: &mut Vec<String>,
) -> Result<ValidatedPack, PackError> {
    let manifest_path = src_dir.join(MANIFEST_NAME);
    let bytes = std::fs::read(&manifest_path).map_err(|_| PackError::MissingManifest)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| PackError::MalformedManifest(format!("非 UTF-8: {e}")))?;
    let manifest = crate::parse_manifest(text)?;
    crate::validate_manifest_schema(&manifest)?;

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_tree_files(src_dir, "", &mut files, withheld_secrets)?;
    files.retain(|(rel, _)| rel != MANIFEST_NAME);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut builder = PackBuilder::create(out).map_err(PackError::Unreadable)?;
    for (rel, path) in &files {
        let payload = std::fs::read(path)
            .map_err(|e| PackError::Unreadable(format!("读取 {path:?} 失败: {e}")))?;
        // Builder errors (a bad archive name, a write failure) are format
        // errors from the caller's point of view: the produced bytes refused
        // to become a pack.
        builder
            .add_bytes(rel, &payload)
            .map_err(PackError::Invalid)?;
    }
    builder.finish(manifest).map_err(PackError::Invalid)?;
    crate::read_pack_from_path(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_pack_from_path;
    use serde_json::json;

    fn sample_manifest() -> PhlPackManifest {
        serde_json::from_value(json!({
            "formatVersion": 1,
            "pack": {"id":"t","name":"T","version":"1.0.0"},
            "dsh": {"version":"0.1.2"},
            "runtime": {"kind":"node","nodeVersion":"22"},
            "plugins": [],
            "content": {"sessionsIncluded": false}
        }))
        .unwrap()
    }

    #[test]
    fn builds_a_pack_the_validator_accepts_and_hashes_match() {
        let dir = std::env::temp_dir().join(format!("phl-packbuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pack_path = dir.join("out.phlpack");
        {
            let mut b = PackBuilder::create(&pack_path).unwrap();
            b.add_bytes("embedded/plugins/mine/package.json", b"{\"name\":\"mine\"}")
                .unwrap();
            b.add_bytes("embedded/plugins/mine/index.js", b"export default 42")
                .unwrap();
            b.finish(sample_manifest()).unwrap();
        }
        let pack = read_pack_from_path(&pack_path).expect("self-built pack is valid");
        let integrity = pack.manifest.integrity.expect("hashes recorded");
        assert_eq!(
            integrity["embedded/plugins/mine/index.js"],
            sha256_hex(b"export default 42")
        );
        assert!(
            !integrity.contains_key("phlpack.json"),
            "manifest not self-hashed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_from_dir_zips_a_layout_and_passes_its_own_validator() {
        // The Skill/CLI entry: a staging directory with `phlpack.json` +
        // payload dirs becomes an archive that `read_pack` accepts, with the
        // integrity map re-sealed over the bytes actually packed.
        let dir = std::env::temp_dir().join(format!("phl-packfromdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("layout");
        std::fs::create_dir_all(src.join("embedded/plugins/mine")).unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/package.json"),
            b"{\"name\":\"mine\"}",
        )
        .unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/index.js"),
            b"export default 1",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("sessions/--P--/session-abc")).unwrap();
        std::fs::write(
            src.join("sessions/--P--/session-abc/session.jsonl"),
            b"{\"type\":\"session\"}\n",
        )
        .unwrap();
        std::fs::write(
            src.join("phlpack.json"),
            serde_json::to_vec(&json!({
                "formatVersion": 1,
                "pack": {"id":"skill-made","name":"Skill Made","version":"1.0.0"},
                "dsh": {"version":"0.1.2"},
                "runtime": {"kind":"node","nodeVersion":"22"},
                "plugins": [{"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}],
                "content": {"sessionsIncluded": true, "sessionCount": 1}
            }))
            .unwrap(),
        )
        .unwrap();

        let out = dir.join("made.phlpack");
        let mut withheld = Vec::new();
        let pack =
            build_pack_from_dir(&src, &out, &mut withheld).expect("layout builds a valid pack");
        assert!(withheld.is_empty(), "clean layout withholds nothing");
        assert_eq!(pack.embedded_plugins, vec!["mine".to_string()]);
        assert!(pack.has_sessions);
        let integrity = pack.manifest.integrity.expect("payload hashed");
        assert_eq!(
            integrity["embedded/plugins/mine/index.js"],
            sha256_hex(b"export default 1")
        );
        assert!(!integrity.contains_key("phlpack.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_from_dir_refuses_a_layout_without_a_manifest() {
        let dir = std::env::temp_dir().join(format!("phl-packnodoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("embedded/plugins/mine")).unwrap();
        let mut withheld = Vec::new();
        let err = build_pack_from_dir(&dir, &dir.join("x.phlpack"), &mut withheld).unwrap_err();
        assert_eq!(err, PackError::MissingManifest);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_from_dir_withholds_secret_shaped_files_and_reports_them() {
        // §12/§33: the archive must not carry them, and the caller learns what
        // was withheld (an unannounced drop would look like a lying pack).
        let dir = std::env::temp_dir().join(format!("phl-packsecret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("layout");
        std::fs::create_dir_all(src.join("embedded/plugins/mine")).unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/package.json"),
            b"{\"name\":\"mine\"}",
        )
        .unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/.env"),
            b"OPENAI_API_KEY=sk-123",
        )
        .unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/.env.example"),
            b"OPENAI_API_KEY=",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("sessions/--P--/session-abc")).unwrap();
        std::fs::write(
            src.join("sessions/--P--/session-abc/session.jsonl"),
            b"{}\n",
        )
        .unwrap();
        std::fs::write(
            src.join("sessions/id_rsa"),
            b"-----BEGIN OPENSSH PRIVATE KEY-----",
        )
        .unwrap();
        std::fs::write(
            src.join("phlpack.json"),
            serde_json::to_vec(&json!({
                "formatVersion": 1,
                "pack": {"id":"sec","name":"Sec","version":"1.0.0"},
                "dsh": {"version":"0.1.2"},
                "runtime": {"kind":"node","nodeVersion":"22"},
                "plugins": [{"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}],
                "content": {"sessionsIncluded": true, "secretsExcluded": true}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut withheld = Vec::new();
        let pack = build_pack_from_dir(&src, &dir.join("s.phlpack"), &mut withheld)
            .expect("the pack still builds");
        assert!(withheld.contains(&"embedded/plugins/mine/.env".to_string()));
        assert!(withheld.contains(&"sessions/id_rsa".to_string()));
        assert!(
            !withheld.iter().any(|w| w.ends_with(".env.example")),
            "the example template travels normally"
        );
        for entry in &pack.entries {
            assert!(!is_secret_entry_name(
                std::path::Path::new(entry)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            ));
        }
        assert!(pack.entries.iter().any(|e| e.ends_with(".env.example")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_names_are_exact_families_not_substrings() {
        // Positive: the §12 families.
        for secret in [
            ".env",
            ".env.local",
            ".env.production",
            "id_rsa",
            "id_ed25519",
            "credentials.json",
            "token.json",
            ".npmrc",
            ".netrc",
        ] {
            assert!(is_secret_entry_name(secret), "{secret} is a secret name");
        }
        // Negative: documentation and ordinary payloads (no substring panic).
        for ok in [
            ".env.example",
            ".env.sample",
            ".env.template",
            "id_rsa.pub",
            "tokenizer.js",
            "keybindings.json",
            "package.json",
            "index.js",
        ] {
            assert!(!is_secret_entry_name(ok), "{ok} must travel");
        }
    }

    #[test]
    fn add_bytes_rejects_a_traversal_name() {
        let dir = std::env::temp_dir().join(format!("phl-packtrav-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut b = PackBuilder::create(&dir.join("x.phlpack")).unwrap();
        assert!(b.add_bytes("../escape", b"x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
