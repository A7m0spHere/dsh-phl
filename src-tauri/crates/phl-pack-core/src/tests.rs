use super::*;
use serde_json::json;
use std::io::{Cursor, Write};

/// Build an in-memory pack archive from owned (name, bytes) pairs. Owned data
/// keeps the test call sites free of borrow juggling.
fn build_zip(files: Vec<(String, Vec<u8>)>, symlinks: &[(&str, &str)]) -> Cursor<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &files {
            w.start_file(name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        for (name, target) in symlinks {
            w.add_symlink(name, target, opts).unwrap();
        }
        w.finish().unwrap();
    }
    let mut cur = Cursor::new(buf);
    std::io::Seek::seek(&mut cur, std::io::SeekFrom::Start(0)).unwrap();
    cur
}

fn valid_manifest_json() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "formatVersion": 1,
        "pack": {"id":"demo","name":"Demo","version":"1.0.0"},
        "dsh": {"version":"0.1.2-rc.1"},
        "runtime": {"kind":"node","nodeVersion":"22"},
        "plugins": [
            {"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}
        ],
        "content": {"sessionsIncluded": true}
    }))
    .unwrap()
}

/// A manifest plus the one embedded plugin file it declares — the minimum a
/// pack must ship to be internally consistent.
fn base_files() -> Vec<(String, Vec<u8>)> {
    vec![
        ("phlpack.json".to_string(), valid_manifest_json()),
        (
            "embedded/plugins/mine/package.json".to_string(),
            b"{\"name\":\"mine\"}".to_vec(),
        ),
    ]
}

/// The embedded plugin payload bytes, so tests can mint a matching integrity hash.
const MINE_PKG: &[u8] = b"{\"name\":\"mine\"}";

fn manifest_with_integrity(integrity: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "formatVersion": 1,
        "pack": {"id":"demo","name":"Demo","version":"1.0.0"},
        "dsh": {"version":"0.1.2-rc.1"},
        "runtime": {"kind":"node","nodeVersion":"22"},
        "plugins": [
            {"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}
        ],
        "content": {"sessionsIncluded": false},
        "integrity": integrity
    }))
    .unwrap()
}

#[test]
fn integrity_accepts_matching_and_rejects_tampered_bytes() {
    // A hash that matches the shipped payload installs clean.
    let good = json!({"embedded/plugins/mine/package.json": sha256_hex(MINE_PKG)});
    let mut files = vec![
        ("phlpack.json".to_string(), manifest_with_integrity(good)),
        (
            "embedded/plugins/mine/package.json".to_string(),
            MINE_PKG.to_vec(),
        ),
    ];
    let mut cur = build_zip(files.clone(), &[]);
    let size = cur.get_ref().len() as u64;
    assert!(
        read_pack(&mut cur, size).is_ok(),
        "matching integrity accepted"
    );

    // The same manifest, but the payload differs → the tamper is caught.
    files[1] = (
        "embedded/plugins/mine/package.json".to_string(),
        b"{\"name\":\"evil\"}".to_vec(),
    );
    let mut cur = build_zip(files, &[]);
    let size = cur.get_ref().len() as u64;
    let err = read_pack(&mut cur, size).unwrap_err();
    assert!(matches!(err, PackError::Consistency(_)), "tamper: {err:?}");

    // A hash naming a file that isn't in the archive is also refused.
    let ghost = json!({"embedded/plugins/nope/index.js": sha256_hex(b"x")});
    let mut cur = build_zip(
        vec![
            ("phlpack.json".to_string(), manifest_with_integrity(ghost)),
            (
                "embedded/plugins/mine/package.json".to_string(),
                MINE_PKG.to_vec(),
            ),
        ],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    assert!(matches!(
        read_pack(&mut cur, size).unwrap_err(),
        PackError::Consistency(_)
    ));
}

#[test]
fn integrity_accepts_the_sha256_prefix_spelling() {
    let v =
        json!({"embedded/plugins/mine/package.json": format!("sha256:{}", sha256_hex(MINE_PKG))});
    let mut cur = build_zip(
        vec![
            ("phlpack.json".to_string(), manifest_with_integrity(v)),
            (
                "embedded/plugins/mine/package.json".to_string(),
                MINE_PKG.to_vec(),
            ),
        ],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    assert!(read_pack(&mut cur, size).is_ok());
}

#[test]
fn accepts_a_complete_pack_and_indexes_its_payload() {
    let mut files = base_files();
    files.push((
        "embedded/plugins/mine/index.js".to_string(),
        b"export default 1".to_vec(),
    ));
    files.push((
        "sessions/--P--/session-x/session.jsonl".to_string(),
        b"{}".to_vec(),
    ));
    files.push(("assets/icon.png".to_string(), b"png".to_vec()));
    let mut cur = build_zip(files, &[]);
    let size = cur.get_ref().len() as u64;
    let pack = read_pack(&mut cur, size).expect("valid pack");
    assert_eq!(pack.embedded_plugins, vec!["mine".to_string()]);
    assert!(pack.has_sessions);
    assert!(pack.entries.contains(&"assets/icon.png".to_string()));
    assert!(pack.manifest.content.sessions_included);
}

#[test]
fn refuses_missing_manifest() {
    let mut cur = build_zip(
        vec![(
            "embedded/plugins/mine/package.json".to_string(),
            b"{}".to_vec(),
        )],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    assert_eq!(
        read_pack(&mut cur, size).unwrap_err(),
        PackError::MissingManifest
    );
}

#[test]
fn refuses_traversal_and_absolute_entries() {
    // A traversal / absolute filename is rejected purely by its name, before
    // any extraction. The good-pack case is covered by the accept test.
    for bad in ["../outside.txt", "/etc/passwd", "sessions/../../escape"] {
        let mut files = base_files();
        files.push((bad.to_string(), b"x".to_vec()));
        let mut cur = build_zip(files, &[]);
        let size = cur.get_ref().len() as u64;
        assert!(
            matches!(
                read_pack(&mut cur, size).unwrap_err(),
                PackError::PathTraversal(_)
            ),
            "{bad} must be refused as traversal"
        );
    }
}

#[test]
fn refuses_symlink_entries() {
    let mut cur = build_zip(base_files(), &[("embedded/escape", "/etc/passwd")]);
    let size = cur.get_ref().len() as u64;
    assert!(matches!(
        read_pack(&mut cur, size).unwrap_err(),
        PackError::SymlinkEntry(_)
    ));
}

#[test]
fn refuses_entry_declared_embedded_but_absent() {
    // Manifest names embedded/plugins/mine, but the archive carries no
    // mine/package.json → consistency failure (spec §23 corrupted/missing embed).
    let mut cur = build_zip(
        vec![("phlpack.json".to_string(), valid_manifest_json())],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    let err = read_pack(&mut cur, size).unwrap_err();
    assert!(matches!(err, PackError::Consistency(_)), "got {err:?}");
}

#[test]
fn refuses_unsupported_format_version_via_raw_gate() {
    let bad = serde_json::to_vec(&json!({
        "formatVersion": 999,
        "pack": {"id":"x","name":"N","version":"1"},
        "dsh": {"version":"0.1.2"}
    }))
    .unwrap();
    let mut cur = build_zip(vec![("phlpack.json".to_string(), bad)], &[]);
    let size = cur.get_ref().len() as u64;
    match read_pack(&mut cur, size).unwrap_err() {
        PackError::UnsupportedVersion { found, supported } => {
            assert_eq!(found, 999);
            assert_eq!(supported, PACK_FORMAT_VERSION);
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn refuses_malformed_and_non_utf8_manifests() {
    let mut cur = build_zip(
        vec![("phlpack.json".to_string(), b"{ not json".to_vec())],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    assert!(matches!(
        read_pack(&mut cur, size).unwrap_err(),
        PackError::MalformedManifest(_)
    ));
    let mut cur = build_zip(
        vec![("phlpack.json".to_string(), vec![0xffu8, 0xfe, 0x00])],
        &[],
    );
    let size = cur.get_ref().len() as u64;
    assert!(matches!(
        read_pack(&mut cur, size).unwrap_err(),
        PackError::MalformedManifest(_)
    ));
}

#[test]
fn normalize_entry_accepts_dirs_and_rejects_escapes() {
    assert_eq!(
        normalize_entry("embedded/plugins/a/b.js")
            .unwrap()
            .display()
            .to_string()
            .replace('\\', "/"),
        "embedded/plugins/a/b.js"
    );
    assert_eq!(normalize_entry("./x").unwrap(), PathBuf::from("x"));
    assert_eq!(normalize_entry("a//b").unwrap(), PathBuf::from("a/b"));
    assert!(normalize_entry("../x").is_err());
    assert!(normalize_entry("a/../../x").is_err());
    assert!(normalize_entry("").is_err());
    assert_eq!(
        normalize_entry("embedded\\plugins\\m").unwrap(),
        PathBuf::from("embedded/plugins/m")
    );
}
