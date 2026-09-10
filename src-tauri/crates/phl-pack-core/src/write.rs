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
    ensure_not_cancelled, normalize_entry, read_pack_from_path_with_cancel, sha256_hex, PackError,
    PhlPackManifest, ValidatedPack, MANIFEST_NAME,
};

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

/// Accumulates payload files + their hashes, then writes the archive.
pub struct PackBuilder {
    writer: ZipWriter<std::io::BufWriter<std::fs::File>>,
    integrity: BTreeMap<String, String>,
}

/// What a whole-tree add did: how many files went in, which archive paths
/// were withheld because their names carry secrets (§12), and which links
/// were skipped. The withheld list is archive-relative (forward slashes) so
/// the host can report it verbatim; the skipped-links list exists for the
/// same reason — a pack is a data container and never carries a link
/// (machine-local absolute paths have no meaning at the far end), but
/// "silently excluded ~N entries" is exactly the kind of thing the user is
/// owed a line about rather than discovering as a missing file later.
#[derive(Debug, Default)]
pub struct TreeAdd {
    pub added: usize,
    pub withheld_secrets: Vec<String>,
    pub skipped_links: Vec<String>,
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
    // Judge the basename only. Callers legitimately hand us either a bare file
    // name (the CLI collector, the host preview) or a full archive-relative
    // path (`add_tree`), and a `config/.env` is exactly as secret as a root
    // `.env` — an earlier version that matched the *whole* path against the
    // `starts_with(".env")` family silently let nested credentials through, so
    // the preview said "excluded" while the archive carried the file. Collapsing
    // to the basename here makes every walker agree by construction instead of
    // relying on each call site to pre-strip. Both separators are handled so the
    // rule is invariant to whether a caller normalised backslashes yet.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
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

    /// Stream a file from disk into the archive, hashing as it goes, so a
    /// large payload is never materialised whole in memory (§R6: the previous
    /// `read_to_end` + `add_bytes` path peaked at the file's full size). The
    /// deflate writer and the SHA-256 hasher consume the same fixed buffer in
    /// one pass, so the recorded integrity covers exactly the bytes written.
    pub fn add_file_from_disk(&mut self, src: &Path, archive_name: &str) -> Result<(), String> {
        self.add_file_from_disk_with_cancel(src, archive_name, &|| false)
    }

    /// Cancellable streaming file add. The callback is checked before every
    /// 64 KiB read/write chunk; cancellation returns the stable `cancelled`
    /// marker so the host can remove the partial archive.
    pub fn add_file_from_disk_with_cancel<F: Fn() -> bool + ?Sized>(
        &mut self,
        src: &Path,
        archive_name: &str,
        cancel: &F,
    ) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        ensure_not_cancelled(cancel).map_err(|e| e.detail())?;
        normalize_entry(archive_name).map_err(|e| e.detail())?;
        let mut file = std::fs::File::open(src).map_err(|e| format!("读取 {src:?} 失败: {e}"))?;
        self.writer
            .start_file(archive_name, options())
            .map_err(|e| format!("写入条目 {archive_name} 失败: {e}"))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65_536];
        loop {
            ensure_not_cancelled(cancel).map_err(|e| e.detail())?;
            let n = file
                .read(&mut buf)
                .map_err(|e| format!("读取 {src:?} 失败: {e}"))?;
            if n == 0 {
                break;
            }
            self.writer
                .write_all(&buf[..n])
                .map_err(|e| format!("写入条目内容失败: {e}"))?;
            hasher.update(&buf[..n]);
        }
        let digest = hex::encode(hasher.finalize());
        self.integrity
            .insert(archive_name.replace('\\', "/"), digest);
        Ok(())
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
        self.add_tree_with_cancel(dir, archive_prefix, &|| false)
    }

    pub fn add_tree_with_cancel<F: Fn() -> bool + ?Sized>(
        &mut self,
        dir: &Path,
        archive_prefix: &str,
        cancel: &F,
    ) -> Result<TreeAdd, String> {
        let mut added = 0;
        let mut secrets: Vec<String> = Vec::new();
        let mut links: Vec<String> = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        let prefix = archive_prefix.trim_end_matches('/');
        while let Some(current) = stack.pop() {
            ensure_not_cancelled(cancel).map_err(|e| e.detail())?;
            for entry in std::fs::read_dir(&current)
                .map_err(|e| format!("遍历 {current:?} 失败: {e}"))?
                .flatten()
            {
                ensure_not_cancelled(cancel).map_err(|e| e.detail())?;
                let path = entry.path();
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    // Skipped by design (a pack carries bytes, not machine
                    // paths) but reported: silently dropping entries from a
                    // distribution artifact is how "works on my machine" is
                    // born. The caller decides whether the user needs a word.
                    if let Ok(rel) = path.strip_prefix(dir) {
                        links.push(format!(
                            "{prefix}/{}",
                            rel.to_string_lossy().replace('\\', "/")
                        ));
                    }
                    continue;
                }
                let meta = entry
                    .metadata()
                    .map_err(|e| format!("读取属性失败 {path:?}: {e}"))?;
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
                self.add_file_from_disk_with_cancel(&path, &name, cancel)?;
                added += 1;
            }
        }
        Ok(TreeAdd {
            added,
            withheld_secrets: secrets,
            skipped_links: links,
        })
    }

    /// Write `phlpack.json` last, with the integrity map the builder assembled
    /// from the payload files, and finalise the archive.
    pub fn finish(self, manifest: PhlPackManifest) -> Result<(), String> {
        self.finish_with_cancel(manifest, &|| false)
    }

    pub fn finish_with_cancel<F: Fn() -> bool + ?Sized>(
        mut self,
        mut manifest: PhlPackManifest,
        cancel: &F,
    ) -> Result<(), String> {
        ensure_not_cancelled(cancel).map_err(|e| e.detail())?;
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
/// Skipped links are reported into `links` for the caller's warning surface.
fn collect_tree_files(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
    withheld: &mut Vec<String>,
    links: &mut Vec<String>,
    cancel: &impl Fn() -> bool,
) -> Result<(), PackError> {
    ensure_not_cancelled(cancel)?;
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| PackError::Unreadable(format!("读取打包目录 {dir:?} 失败: {e}")))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        ensure_not_cancelled(cancel)?;
        let path = entry.path();
        if path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            links.push(if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            });
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let next = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            collect_tree_files(&path, &next, out, withheld, links, cancel)?;
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
/// Links are likewise never packed and surface through `skipped_links`.
pub fn build_pack_from_dir(
    src_dir: &Path,
    out: &Path,
    withheld_secrets: &mut Vec<String>,
    skipped_links: &mut Vec<String>,
) -> Result<ValidatedPack, PackError> {
    build_pack_from_dir_with_cancel(src_dir, out, withheld_secrets, skipped_links, &|| false)
}

pub fn build_pack_from_dir_with_cancel<F: Fn() -> bool>(
    src_dir: &Path,
    out: &Path,
    withheld_secrets: &mut Vec<String>,
    skipped_links: &mut Vec<String>,
    cancel: &F,
) -> Result<ValidatedPack, PackError> {
    ensure_not_cancelled(cancel)?;
    if out.exists() {
        return Err(PackError::Unreadable(format!(
            "输出文件已存在，拒绝覆盖: {}",
            out.display()
        )));
    }
    let manifest_path = src_dir.join(MANIFEST_NAME);
    let bytes = std::fs::read(&manifest_path).map_err(|_| PackError::MissingManifest)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| PackError::MalformedManifest(format!("非 UTF-8: {e}")))?;
    let manifest = crate::parse_manifest(text)?;
    crate::validate_manifest_schema(&manifest)?;

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_tree_files(
        src_dir,
        "",
        &mut files,
        withheld_secrets,
        skipped_links,
        cancel,
    )?;
    files.retain(|(rel, _)| rel != MANIFEST_NAME);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let result = (|| {
        let mut builder = PackBuilder::create(out).map_err(PackError::Unreadable)?;
        for (rel, path) in &files {
            ensure_not_cancelled(cancel)?;
            builder
                .add_file_from_disk_with_cancel(path, rel, cancel)
                .map_err(PackError::Invalid)?;
        }
        builder
            .finish_with_cancel(manifest, cancel)
            .map_err(PackError::Invalid)?;
        read_pack_from_path_with_cancel(out, cancel)
    })();
    if result.is_err() {
        // A failed build is not an artifact: leaving a partial ZIP behind
        // makes the next safe (no-overwrite) retry fail and invites users to
        // mistake an invalid file for a completed pack.
        let _ = std::fs::remove_file(out);
    }
    result
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
    fn streaming_add_file_from_disk_matches_the_byte_digest() {
        // §R6: the streaming path must record the SAME sha256 as `add_bytes`
        // over identical content, across many fixed-buffer iterations.
        let dir = std::env::temp_dir().join(format!("phl-streamdigest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let content: Vec<u8> = (0..1_000_003u64).map(|i| (i % 251) as u8).collect();
        let src = dir.join("blob.bin");
        std::fs::write(&src, &content).unwrap();

        let streamed = dir.join("a.phlpack");
        {
            let mut b = PackBuilder::create(&streamed).unwrap();
            b.add_file_from_disk(&src, "embedded/plugins/blob.bin")
                .unwrap();
            let digest = b.integrity()["embedded/plugins/blob.bin"].clone();
            assert_eq!(digest, sha256_hex(&content), "streamed hash must match");
            b.finish(sample_manifest()).unwrap();
        }
        let by_bytes = dir.join("b.phlpack");
        {
            let mut b = PackBuilder::create(&by_bytes).unwrap();
            b.add_bytes("embedded/plugins/blob.bin", &content).unwrap();
            b.finish(sample_manifest()).unwrap();
        }
        // Both archives re-validate through the integrity verifier.
        read_pack_from_path(&streamed).expect("streamed pack validates");
        read_pack_from_path(&by_bytes).expect("bytes pack validates");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_handles_a_many_entry_pack_in_linear_time() {
        // §R6: `verify_integrity` used to re-scan every archive index for each
        // integrity key (O(entries²)). A fully-covered multi-entry pack must
        // still validate correctly through the single-pass index.
        let dir = std::env::temp_dir().join(format!("phl-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pack = dir.join("many.phlpack");
        const N: usize = 800;
        {
            let mut b = PackBuilder::create(&pack).unwrap();
            for i in 0..N {
                b.add_bytes(
                    &format!("embedded/plugins/p{i}/index.js"),
                    format!("export const i = {i}").as_bytes(),
                )
                .unwrap();
            }
            b.finish(sample_manifest()).unwrap();
        }
        let validated = read_pack_from_path(&pack).expect("a many-entry pack validates");
        let integrity = validated.manifest.integrity.expect("all payloads hashed");
        assert_eq!(integrity.len(), N);
        assert_eq!(
            integrity[&format!("embedded/plugins/p{}/index.js", N - 1)],
            sha256_hex(format!("export const i = {}", N - 1).as_bytes())
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
        let mut links = Vec::new();
        let pack = build_pack_from_dir(&src, &out, &mut withheld, &mut links)
            .expect("layout builds a valid pack");
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
        let err = build_pack_from_dir(&dir, &dir.join("x.phlpack"), &mut withheld, &mut Vec::new())
            .unwrap_err();
        assert_eq!(err, PackError::MissingManifest);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_whole_directory_build_removes_its_partial_output() {
        let dir = std::env::temp_dir().join(format!("phl-packpartial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sessions/project/session-1")).unwrap();
        std::fs::write(
            src.join(MANIFEST_NAME),
            serde_json::to_vec(&sample_manifest()).unwrap(),
        )
        .unwrap();
        // The validator rejects this privacy contradiction only after the ZIP
        // has been written, exercising cleanup of a genuinely created output.
        std::fs::write(
            src.join("sessions/project/session-1/session.jsonl"),
            b"{}\n",
        )
        .unwrap();
        let out = dir.join("partial.phlpack");
        assert!(build_pack_from_dir(&src, &out, &mut Vec::new(), &mut Vec::new()).is_err());
        assert!(!out.exists(), "a failed build must not leave an artifact");

        std::fs::write(&out, b"keep me").unwrap();
        assert!(build_pack_from_dir(&src, &out, &mut Vec::new(), &mut Vec::new()).is_err());
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"keep me",
            "pre-existing outputs are refused before any write"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory link of the shape npm lays out: a junction on Windows
    /// (no privilege needed), a plain symlink elsewhere.
    fn dir_link(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(windows)]
        {
            let out = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "mklink failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn build_from_dir_never_packs_links_but_reports_them() {
        // A pack is a data container: machine-local link paths are meaningless
        // at the far end, so they are skipped — but an *unannounced* skip is
        // how half-installed plugins are born, so the walker lists them.
        let dir = std::env::temp_dir().join(format!("phl-packlinks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("layout");
        std::fs::create_dir_all(src.join("embedded/plugins/mine")).unwrap();
        std::fs::write(
            src.join("embedded/plugins/mine/package.json"),
            b"{\"name\":\"mine\"}",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("shared-dep")).unwrap();
        std::fs::write(src.join("shared-dep/index.js"), b"shared").unwrap();
        dir_link(
            &src.join("shared-dep"),
            &src.join("embedded")
                .join("plugins")
                .join("mine")
                .join("node_modules-linked"),
        );
        std::fs::write(
            src.join("phlpack.json"),
            serde_json::to_vec(&json!({
                "formatVersion": 1,
                "pack": {"id":"lnk","name":"Lnk","version":"1.0.0"},
                "dsh": {"version":"0.1.2"},
                "runtime": {"kind":"node","nodeVersion":"22"},
                "plugins": [{"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}],
                "content": {"sessionsIncluded": false, "secretsExcluded": true}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut withheld = Vec::new();
        let mut links = Vec::new();
        let pack = build_pack_from_dir(&src, &dir.join("l.phlpack"), &mut withheld, &mut links)
            .expect("links do not fail the build");
        assert_eq!(
            links,
            vec!["embedded/plugins/mine/node_modules-linked".to_string()],
            "the skipped link is named exactly"
        );
        for entry in &pack.entries {
            assert!(
                !entry.contains("node_modules-linked"),
                "the link's name never rides into the archive: {entry}"
            );
        }
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
        let pack =
            build_pack_from_dir(&src, &dir.join("s.phlpack"), &mut withheld, &mut Vec::new())
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
    fn add_tree_withholds_nested_secret_files_and_keeps_examples() {
        // R1: the desktop export path walks whole trees through `add_tree`,
        // which hands the *relative* path to the secret test. Nested
        // `config/.env` / `auth/token.json` are exactly as secret as a root
        // `.env`, so the shared basename rule must drop them — and the
        // withheld report must carry the full archive path, while ordinary and
        // example files travel untouched.
        let dir = std::env::temp_dir().join(format!("phl-addtree-secret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("plugin");
        std::fs::create_dir_all(src.join("config")).unwrap();
        std::fs::create_dir_all(src.join("auth")).unwrap();
        std::fs::write(src.join(".env"), b"A=1").unwrap();
        std::fs::write(src.join("config").join(".env"), b"B=2").unwrap();
        std::fs::write(src.join("auth").join("token.json"), b"C=3").unwrap();
        std::fs::write(src.join(".env.example"), b"# docs").unwrap();
        std::fs::write(src.join("config").join("tokenizer.js"), b"// ok").unwrap();

        let mut b = PackBuilder::create(&dir.join("out.phlpack")).unwrap();
        let tree = b.add_tree(&src, "embedded/plugins/mine").unwrap();
        assert_eq!(tree.added, 2, "only the two non-secret files go in");
        let withheld: Vec<&str> = tree.withheld_secrets.iter().map(|s| s.as_str()).collect();
        assert!(withheld.contains(&"embedded/plugins/mine/.env"));
        assert!(withheld.contains(&"embedded/plugins/mine/config/.env"));
        assert!(withheld.contains(&"embedded/plugins/mine/auth/token.json"));
        let integrity = b.integrity();
        assert!(integrity.contains_key("embedded/plugins/mine/.env.example"));
        assert!(integrity.contains_key("embedded/plugins/mine/config/tokenizer.js"));
        for secret in [
            "embedded/plugins/mine/.env",
            "embedded/plugins/mine/config/.env",
            "embedded/plugins/mine/auth/token.json",
        ] {
            assert!(
                !integrity.contains_key(secret),
                "{secret} leaked into the pack"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_rule_matches_the_same_basename_at_any_depth() {
        // The whole point of collapsing to the basename: a caller passing a
        // root name, a nested relative path, or a normalised archive path all
        // get the same verdict, so the preview and every walker agree.
        assert!(is_secret_entry_name("config/.env"));
        assert!(is_secret_entry_name("a/b/c/token.json"));
        assert!(is_secret_entry_name(
            "embedded\\plugins\\mine\\config\\.env"
        ));
        assert!(!is_secret_entry_name("config/tokenizer.js"));
        assert!(!is_secret_entry_name("auth/credentials.sample.json"));
        assert!(!is_secret_entry_name("docs/.env.example"));
    }

    #[test]
    fn add_bytes_rejects_a_traversal_name() {
        let dir = std::env::temp_dir().join(format!("phl-packtrav-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut b = PackBuilder::create(&dir.join("x.phlpack")).unwrap();
        assert!(b.add_bytes("../escape", b"x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streaming_add_observes_cancel_inside_one_large_file() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir =
            std::env::temp_dir().join(format!("phl-pack-cancel-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("large.bin");
        std::fs::write(&source, vec![0x5au8; 1024 * 1024]).unwrap();
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 2;
        let mut builder = PackBuilder::create(&dir.join("partial.phlpack")).unwrap();
        let err = builder
            .add_file_from_disk_with_cancel(&source, "assets/large.bin", &cancel)
            .unwrap_err();
        assert_eq!(err, "cancelled");
        assert!(calls.load(Ordering::SeqCst) >= 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
