use super::*;
use serde_json::json;

fn manifest_json(over: serde_json::Value) -> serde_json::Value {
    let mut base = json!({
        "formatVersion": 1,
        "pack": {"id":"demo","name":"Demo Pack","version":"1.0.0"},
        "dsh": {"version":"0.1.2-rc.1"},
        "runtime": {"kind":"node","nodeVersion":"22.14.0","arch":"x64"},
        "plugins": [
            {"id":"remote-one","version":"1.2.0","source":{"type":"registry","registryId":"remote-one"}},
            {"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}
        ],
        "content": {"sessionsIncluded": false, "secretsExcluded": true}
    });
    if let (Some(map), Some(o)) = (base.as_object_mut(), over.as_object()) {
        for (k, v) in o {
            map.insert(k.clone(), v.clone());
        }
    }
    base
}

fn parse(v: serde_json::Value) -> PhlPackManifest {
    serde_json::from_value(v).expect("manifest must deserialize")
}

#[test]
fn accepts_a_well_formed_pack() {
    let m = parse(manifest_json(json!({})));
    assert_eq!(m.format_version, PACK_FORMAT_VERSION);
    assert_eq!(m.plugins.len(), 2);
    assert!(validate_manifest(&m).is_ok());
    // A remote plugin re-downloads; an embedded one carries bytes (spec §8).
    assert!(matches!(m.plugins[0].source, PluginSource::Registry { .. }));
    assert!(matches!(m.plugins[1].source, PluginSource::Embedded { .. }));
    // required defaults true so an unlabelled pack refuses rather than
    // half-installs (spec §20).
    assert!(m.plugins.iter().all(|p| p.required));
}

#[test]
fn refuses_newer_format_version() {
    let m = parse(manifest_json(json!({"formatVersion": 99})));
    let err = validate_manifest(&m).unwrap_err();
    assert!(err.contains("格式版本"), "got {err}");
}

#[test]
fn refuses_missing_core_identity_and_dependency() {
    for (label, over) in [
        ("no id", json!({"pack": {"id":"","name":"N","version":"1"}})),
        ("no dsh", json!({"dsh": {"version":""}})),
        (
            "no version",
            json!({"pack": {"id":"x","name":"N","version":""}}),
        ),
    ] {
        let m = parse(manifest_json(over));
        assert!(validate_manifest(&m).is_err(), "should refuse: {label}");
    }
}

#[test]
fn embedded_path_must_be_relative_and_in_pack() {
    for bad in ["/abs/path", "C:/evil", "embedded/../escape"] {
        let m = parse(manifest_json(json!({
            "plugins": [{"id":"p","version":"1","source":{"type":"embedded","path":bad}}]
        })));
        let err = validate_manifest(&m).unwrap_err();
        assert!(err.contains("path") || err.contains("相对"), "{bad}: {err}");
    }
}

#[test]
fn duplicate_plugin_ids_are_refused() {
    let m = parse(manifest_json(json!({
        "plugins": [
            {"id":"dup","version":"1","source":{"type":"registry","registryId":"a"}},
            {"id":"dup","version":"2","source":{"type":"registry","registryId":"b"}}
        ]
    })));
    assert!(validate_manifest(&m).unwrap_err().contains("重复"));
}

#[test]
fn unknown_content_fields_are_tolerated_for_forward_compatibility() {
    // No deny_unknown_fields: a field a newer PHL wrote must not erase the pack.
    let mut v = manifest_json(json!({}));
    v["pack"]["futureField"] = json!("ignored");
    let m = parse(v);
    assert!(validate_manifest(&m).is_ok());
}

#[test]
fn roundtrips_through_serde_json_preserving_optional_sections() {
    let original = parse(manifest_json(
        json!({"overrides":["overrides/settings-overlay.yaml"]}),
    ));
    let text = serde_json::to_string(&original).unwrap();
    let back: PhlPackManifest = serde_json::from_str(&text).unwrap();
    assert_eq!(back.overrides, original.overrides);
    assert_eq!(back.runtime.arch.as_deref(), Some("x64"));
    assert!(back.integrity.is_none());
}
