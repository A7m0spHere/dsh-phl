use super::launch_keys::{provider_launch_keys, resolve_launch_keys};
use super::models::{
    api_error_message, fetch_models_inner, models_url, parse_models_body, ENV_MISSING,
};
use super::sync::{import_inner, sections_hash, snapshot_inner};
use super::*;
use crate::credentials::{Creds, CredentialStore};
use std::path::PathBuf;
/// A credential store backed by the real OS on Windows; the test entries
/// it touches are namespaced and cleaned up by each test that uses one.
fn test_creds() -> Creds {
    Creds::platform_default()
}

fn provider(name: &str, env: &str) -> Provider {
    Provider {
        id: format!("p-{name}"),
        name: name.into(),
        kind: "aggregator".into(),
        notes: None,
        api: Some("openai-completions".into()),
        base_url: Some(format!("https://{name}.example/v1")),
        api_key_env: env.into(),
        api_key: None,
        models: vec![ModelRef {
            id: format!("{name}-model"),
            name: None,
            context_window: Some(65536),
            max_tokens: Some(8192),
            ..Default::default()
        }],
        enabled: true,
    }
}

fn config() -> ApiConfig {
    ApiConfig {
        version: 1,
        updated_at: String::new(),
        default_provider_id: Some("p-deepseek".into()),
        default_model: Some(DefaultModel {
            provider_name: "deepseek".into(),
            model: "deepseek-v4".into(),
            reasoning_effort: Some("max".into()),
        }),
        providers: vec![
            provider("deepseek", "DEEPSEEK_API_KEY"),
            provider("zen", "ZEN_API_KEY"),
        ],
    }
}

fn temp_instance(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-api-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
    std::fs::write(dir.join("instance.json"), "{}").unwrap();
    dir
}

/// The `<root>/instances/<id>` layout that commands resolve through
/// `instance_dir` — snapshot tests need the parent, not just the dir.
fn temp_root_with_instance(tag: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("phl-apiroot-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("instances").join(tag);
    std::fs::create_dir_all(dir.join("dsh-home")).unwrap();
    std::fs::write(dir.join("instance.json"), "{}").unwrap();
    (root, dir)
}

#[test]
fn env_name_grammar() {
    assert!(valid_env_name("DEEPSEEK_API_KEY"));
    assert!(valid_env_name("_X"));
    assert!(!valid_env_name("9LIVES"));
    assert!(!valid_env_name("has space"));
    assert!(!valid_env_name(""));
}

#[test]
fn resolve_is_deterministic() {
    let c = config();
    let b = ApiBinding::default();
    let (l1, d1) = resolve_sections(&c, &b).unwrap();
    let (l2, d2) = resolve_sections(&c, &b).unwrap();
    assert_eq!(sections_hash(&l1, &d1), sections_hash(&l2, &d2));
}

#[test]
fn default_binding_injects_enabled_providers() {
    let c = config();
    let (llm, default_model) = resolve_sections(&c, &ApiBinding::default()).unwrap();
    // serde_yaml's generic `Into<Value>` key argument needs the concrete
    // type in tests (other crates also impl `From<&str>`); the production
    // code already goes through typed helpers.
    let key = |s: &str| serde_yaml::Value::from(s);
    assert!(llm.contains_key(key("deepseek")));
    assert!(llm.contains_key(key("zen")));
    assert_eq!(
        default_model
            .as_mapping()
            .unwrap()
            .get(key("model"))
            .unwrap()
            .as_str(),
        Some("deepseek-v4")
    );
}

#[test]
fn custom_binding_requires_existing_ids() {
    let c = config();
    let b = ApiBinding {
        inheritance: "custom".into(),
        provider_ids: vec!["nope".into()],
        ..Default::default()
    };
    assert!(resolve_sections(&c, &b).is_err());
}

#[test]
fn custom_binding_instances_default_model_wins() {
    let c = config();
    let b = ApiBinding {
        inheritance: "custom".into(),
        provider_ids: vec!["p-zen".into()],
        default_model: Some(DefaultModel {
            provider_name: "zen".into(),
            model: "zen-fast".into(),
            reasoning_effort: None,
        }),
        synced_at: None,
        synced_hash: None,
    };
    let (llm, default_model) = resolve_sections(&c, &b).unwrap();
    let key = |s: &str| serde_yaml::Value::from(s);
    assert!(!llm.contains_key(key("deepseek")));
    assert_eq!(
        default_model
            .as_mapping()
            .unwrap()
            .get(key("model"))
            .unwrap()
            .as_str(),
        Some("zen-fast")
    );
}

#[test]
fn merge_preserves_unknown_keys_and_existing_providers() {
    // The product promise: an instance already has a DSH-written
    // settings.yaml with unrelated sections and providers; syncing must
    // only add/overwrite the managed parts.
    let raw = "ui-theme: dark\nagent-default-model:\n  provider: other\n  model: keep-me\nllm-pi-ai:\n  providers:\n    google:\n      apiKeyEnv: GOOGLE_API_KEY\n";
    let mut doc: serde_yaml::Value = serde_yaml::from_str(raw).unwrap();
    let (llm, default_model) = resolve_sections(&config(), &ApiBinding::default()).unwrap();
    merge_sections(&mut doc, &llm, &default_model).unwrap();
    let out = serde_yaml::to_string(&doc).unwrap();
    assert!(
        out.contains("ui-theme: dark"),
        "unknown key survived: {out}"
    );
    assert!(out.contains("google:"), "existing provider survived: {out}");
    assert!(out.contains("deepseek:"), "bound provider injected: {out}");
    assert!(
        out.contains("deepseek-v4"),
        "default model overwritten: {out}"
    );
    assert!(!out.contains("keep-me"), "old default replaced: {out}");
}

#[test]
fn merge_keeps_user_default_when_chain_has_none() {
    let raw = "agent-default-model:\n  provider: other\n  model: keep-me\n";
    let mut doc: serde_yaml::Value = serde_yaml::from_str(raw).unwrap();
    let mut c = config();
    c.default_model = None;
    let b = ApiBinding::default();
    let (llm, default_model) = resolve_sections(&c, &b).unwrap();
    merge_sections(&mut doc, &llm, &default_model).unwrap();
    let out = serde_yaml::to_string(&doc).unwrap();
    assert!(
        out.contains("keep-me"),
        "null default must not erase: {out}"
    );
}

#[tokio::test]
async fn sync_writes_and_import_reads_back() {
    let dir = temp_instance("roundtrip");
    let c = config();
    let applied = sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();
    assert!(applied.synced_hash.is_some());

    let imported = import_inner(&dir).await.unwrap().unwrap();
    let names: Vec<&str> = imported.providers.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"deepseek") && names.contains(&"zen"));
    assert_eq!(
        imported.default_model.as_ref().unwrap().model,
        "deepseek-v4"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn model_capabilities_roundtrip_and_provenance_never_materializes() {
    let dir = temp_instance("capabilities-roundtrip");
    let mut c = config();
    c.providers[0].models = vec![serde_json::from_value(serde_json::json!({
        "id":"custom-model", "name":"My model", "contextWindow":200000, "maxTokens":32768,
        "input":["text","image"], "reasoningEfforts":{"off":null,"high":"custom-high"},
        "metadataSources":{"contextWindow":"models.dev", "maxTokens":"fallback"}
    }))
    .unwrap()];
    c.providers[1].models[0].reasoning_efforts = Some(ReasoningEfforts::Disabled(FalseValue));
    sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();
    let raw = tokio::fs::read_to_string(settings_path(&dir))
        .await
        .unwrap();
    assert!(!raw.contains("metadataSources"));
    assert!(!raw.contains("models.dev"));
    assert!(!raw.contains("fallback"));
    let imported = import_inner(&dir).await.unwrap().unwrap();
    for provider in &c.providers {
        let actual = imported
            .providers
            .iter()
            .find(|p| p.name == provider.name)
            .unwrap();
        let mut expected = provider.models.clone();
        for m in &mut expected {
            m.metadata_sources.clear();
        }
        assert_eq!(actual.models, expected);
    }
    let before = resolve_sections(&c, &ApiBinding::default()).unwrap();
    c.providers[0].models[0].metadata_sources.clear();
    assert_eq!(
        resolve_sections(&c, &ApiBinding::default()).unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sync_is_idempotent_and_drift_detects_edits() {
    let dir = temp_instance("drift");
    let c = config();
    sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();

    // Re-merging must not change the file.
    let mut doc = read_settings(&dir).await.unwrap();
    let before = serde_yaml::to_string(&doc).unwrap();
    let (llm, dm) = resolve_sections(&c, &ApiBinding::default()).unwrap();
    merge_sections(&mut doc, &llm, &dm).unwrap();
    assert_eq!(before, serde_yaml::to_string(&doc).unwrap());

    // A user hand-edit of an UNMANAGED key must not read as drift; an edit
    // inside the managed provider map must.
    let raw = tokio::fs::read_to_string(settings_path(&dir))
        .await
        .unwrap();
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    doc.as_mapping_mut()
        .unwrap()
        .insert("ui-theme".into(), "light".into());
    tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
        .await
        .unwrap();
    let mut doc = read_settings(&dir).await.unwrap();
    let before = serde_yaml::to_string(&doc).unwrap();
    merge_sections(&mut doc, &llm, &dm).unwrap();
    assert_eq!(
        before,
        serde_yaml::to_string(&doc).unwrap(),
        "unmanaged edit is not drift"
    );

    let raw = tokio::fs::read_to_string(settings_path(&dir))
        .await
        .unwrap();
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    let key = |s: &str| serde_yaml::Value::from(s);
    doc.as_mapping_mut()
        .unwrap()
        .get_mut(key("llm-pi-ai"))
        .unwrap()
        .get_mut(key("providers"))
        .unwrap()
        .get_mut(key("zen"))
        .unwrap()
        .as_mapping_mut()
        .unwrap()
        .insert(key("baseURL"), "https://tampered.example/v1".into());
    tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
        .await
        .unwrap();
    let mut doc = read_settings(&dir).await.unwrap();
    let before = serde_yaml::to_string(&doc).unwrap();
    merge_sections(&mut doc, &llm, &dm).unwrap();
    assert_ne!(
        before,
        serde_yaml::to_string(&doc).unwrap(),
        "managed edit is drift"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Real-data roundtrip probe: copies the machine's actual
/// `~/.dsh/settings.yaml` into a scratch instance, imports it into the
/// library, syncs it back, and re-imports. Run with
/// `cargo test -- --ignored real_settings_roundtrip`. It only asserts
/// that the *managed* parts survive — the rest is DSH's business.
#[tokio::test]
#[ignore]
async fn real_settings_roundtrip() {
    let real = dirs::home_dir().map(|h| h.join(".dsh").join("settings.yaml"));
    let Some(real) = real.filter(|p| p.exists()) else {
        println!("no ~/.dsh/settings.yaml on this machine — nothing to probe");
        return;
    };
    let raw = std::fs::read_to_string(&real).unwrap();
    let dir = temp_instance("real");
    std::fs::write(settings_path(&dir), &raw).unwrap();

    let imported = import_inner(&dir).await.unwrap().expect("should parse");
    println!(
        "imported {} providers: {:?}",
        imported.providers.len(),
        imported
            .providers
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
    );
    assert!(!imported.providers.is_empty());

    let applied = sync_inner(&dir, &ApiBinding::default(), &imported)
        .await
        .unwrap();
    let after = import_inner(&dir).await.unwrap().unwrap();
    assert_eq!(after.providers.len(), imported.providers.len());
    // The import/sync loop must be stable on the user's real file.
    assert_eq!(after.default_model, imported.default_model);

    // Unknown keys must still be present verbatim.
    let out = std::fs::read_to_string(settings_path(&dir)).unwrap();
    for key in ["ui-theme", "agent-presets", "permission"] {
        if raw.contains(&format!("{key}:")) {
            assert!(out.contains(&format!("{key}:")), "{key} lost in roundtrip");
        }
    }
    println!("roundtrip OK; {applied:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn never_synced_managed_instance_is_not_dirty() {
    // The regression this guards: an instance created before the library
    // existed (binding, no file) must count as a clean batch-sync target,
    // not as "has local changes".
    let (root, _dir) = temp_root_with_instance("never-synced");
    let b = ApiBinding::default(); // inheritance default, syncedHash None
    let snap = snapshot_inner(&root, "never-synced", &b, &config(), &test_creds())
        .await
        .unwrap();
    assert!(!snap.local_changes);
    assert!(!snap.default_model_changed);
    assert!(snap.live.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn snapshot_reports_live_state_and_local_changes() {
    let (root, dir) = temp_root_with_instance("snapshot");
    let c = config();
    // Snapshots compare against the *applied* binding — syncedHash is the
    // baseline gate for localChanges.
    let b = sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();

    // Fresh sync: the file matches, so a snapshot must not claim changes.
    let fresh = snapshot_inner(&root, "snapshot", &b, &c, &test_creds())
        .await
        .unwrap();
    assert!(!fresh.local_changes);
    assert!(!fresh.default_model_changed);
    assert_eq!(fresh.live.as_ref().unwrap().providers.len(), 2);

    // A local DSH-side edit (tamper a managed provider's URL) and an
    // unmanaged local provider both must surface.
    let raw = tokio::fs::read_to_string(settings_path(&dir))
        .await
        .unwrap();
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    {
        let key = |s: &str| serde_yaml::Value::from(s);
        doc.as_mapping_mut()
            .unwrap()
            .get_mut(key("llm-pi-ai"))
            .unwrap()
            .get_mut(key("providers"))
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .insert(key("local-only"), {
                let mut m = serde_yaml::Mapping::new();
                m.insert(key("apiKeyEnv"), "LOCAL_ONLY_KEY".into());
                serde_yaml::Value::Mapping(m)
            });
        doc.as_mapping_mut()
            .unwrap()
            .get_mut(key("agent-default-model"))
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .insert(key("model"), "deepseek-v9".into());
    }
    tokio::fs::write(settings_path(&dir), serde_yaml::to_string(&doc).unwrap())
        .await
        .unwrap();
    let snap = snapshot_inner(&root, "snapshot", &b, &c, &test_creds())
        .await
        .unwrap();
    assert!(snap.local_changes, "tampered managed provider surfaces");
    assert!(snap.default_model_changed, "edited default model surfaces");
    let live = snap.live.unwrap();
    let names: Vec<&str> = live.providers.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"local-only"),
        "live view shows the file, not the library"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn create_binding_gates_none_and_missing_library() {
    // Unmanaged bindings must be a no-op even when a library exists.
    let (root, dir) = temp_root_with_instance("gate-none");
    std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
    std::fs::write(
        api_config_path(&root),
        serde_json::to_string(&config()).unwrap(),
    )
    .unwrap();
    let none = ApiBinding {
        inheritance: "none".into(),
        provider_ids: vec![],
        default_model: None,
        synced_at: None,
        synced_hash: None,
    };
    assert!(apply_create_binding(&root, &dir, "gate-none", none)
        .await
        .is_none());
    assert!(
        !settings_path(&dir).exists(),
        "unmanaged means never written"
    );

    // Managed but no library: skipped, and the binding stays without a
    // baseline so the launch-time materialization can take over later.
    let _ = std::fs::remove_file(api_config_path(&root));
    let out = apply_create_binding(&root, &dir, "gate-none", ApiBinding::default()).await;
    assert!(out.is_none());
    assert!(!settings_path(&dir).exists());

    // Library returns: the same call materializes and persists.
    std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
    std::fs::write(
        api_config_path(&root),
        serde_json::to_string(&config()).unwrap(),
    )
    .unwrap();
    let out = apply_create_binding(&root, &dir, "gate-none", ApiBinding::default()).await;
    assert!(out.is_some());
    assert!(settings_path(&dir).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn unmanaged_snapshot_is_readonly_truth() {
    let (root, dir) = temp_root_with_instance("unmanaged");
    std::fs::write(
        settings_path(&dir),
        "llm-pi-ai:\n  providers:\n    solo:\n      apiKeyEnv: SOLO_KEY\n",
    )
    .unwrap();
    let b = ApiBinding {
        inheritance: "none".into(),
        provider_ids: vec![],
        default_model: None,
        synced_at: None,
        synced_hash: None,
    };
    let snap = snapshot_inner(&root, "unmanaged", &b, &config(), &test_creds())
        .await
        .unwrap();
    assert!(!snap.local_changes);
    assert_eq!(snap.live.unwrap().providers.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sync_refuses_unmanaged() {
    let dir = temp_instance("none");
    let b = ApiBinding {
        inheritance: "none".into(),
        ..Default::default()
    };
    assert!(sync_inner(&dir, &b, &config()).await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn launch_keys_follow_the_binding_and_blankness() {
    let mut c = config();
    // deepseek has a stored key (with incidental whitespace), zen a blank one.
    c.providers.iter_mut().for_each(|p| {
        p.api_key = Some(match p.id.as_str() {
            "p-deepseek" => "  sk-live  ".into(),
            _ => "   ".into(),
        })
    });
    let keys = provider_launch_keys(&c, &ApiBinding::default());
    assert_eq!(
        keys,
        vec![("DEEPSEEK_API_KEY".to_string(), "sk-live".to_string())],
        "blank stored keys are no keys; names are trimmed of the value only"
    );

    // custom binding only pulls what it selects.
    let custom = ApiBinding {
        inheritance: "custom".into(),
        provider_ids: vec!["p-zen".into()],
        ..Default::default()
    };
    assert!(provider_launch_keys(&c, &custom).is_empty());

    // unmanaged bindings expose nothing to inject.
    let none = ApiBinding {
        inheritance: "none".into(),
        ..Default::default()
    };
    assert!(provider_launch_keys(&c, &none).is_empty());
}

#[tokio::test]
async fn resolve_launch_keys_orders_config_key_above_the_store() {
    // Pins the documented chain BELOW the environment (the launcher's own
    // gate keeps env first): a one-time config key wins over the stored
    // credential, and the store fills only what the config does not carry.
    let creds = test_creds();
    let pid = std::process::id();
    let mut c = config();
    c.providers[0].id = format!("p-test-cfg-{pid}");
    c.providers[0].api_key_env = format!("PHL_TEST_CFG_{pid}");
    c.providers[0].api_key = Some("sk-from-config".into());
    c.providers[1].id = format!("p-test-sto-{pid}");
    c.providers[1].api_key_env = format!("PHL_TEST_STO_{pid}");
    c.providers[1].api_key = None;
    let binding = ApiBinding {
        inheritance: "custom".into(),
        provider_ids: vec![c.providers[0].id.clone(), c.providers[1].id.clone()],
        ..Default::default()
    };
    if let Err(e) = creds.set(&c.providers[0].id, "sk-that-must-lose") {
        // The platform store is Windows-only until T-301/T-302 land; the
        // chain below the store cannot be exercised elsewhere.
        eprintln!("skipping credential-store assertions: {e}");
        return;
    }
    creds.set(&c.providers[1].id, "sk-from-store").unwrap();
    let keys = resolve_launch_keys(&c, &binding, &creds).await;
    let _ = creds.delete(&c.providers[0].id);
    let _ = creds.delete(&c.providers[1].id);
    assert_eq!(
        keys,
        vec![
            (c.providers[0].api_key_env.clone(), "sk-from-config".to_string()),
            (c.providers[1].api_key_env.clone(), "sk-from-store".to_string()),
        ],
        "config key first, store fallback second"
    );
}

#[tokio::test]
async fn stored_key_satisfies_missing_env() {
    let (root, dir) = temp_root_with_instance("stored-key");
    let mut c = config();
    // Unique names so the test machine's environment cannot collide.
    c.providers.iter_mut().for_each(|p| {
        p.api_key_env = format!("PHL_TEST_UNSET_{}", p.name.to_uppercase());
    });
    c.providers[0].api_key = Some("sk-present".into());
    std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
    std::fs::write(api_config_path(&root), serde_json::to_string(&c).unwrap()).unwrap();
    let b = ApiBinding::default();
    let applied = sync_inner(&dir, &b, &c).await.unwrap();
    let snap = snapshot_inner(&root, "stored-key", &applied, &c, &test_creds())
        .await
        .unwrap();
    assert_eq!(
        snap.missing_keys,
        vec![format!(
            "PHL_TEST_UNSET_{}",
            c.providers[1].name.to_uppercase()
        )],
        "the provider with a stored key must not read as missing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn models_url_shapes() {
    assert_eq!(
        models_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/models"
    );
    assert_eq!(
        models_url("https://api.openai.com/v1/"),
        "https://api.openai.com/v1/models"
    );
    // trailing version segment: never double up /v1/v1
    assert_eq!(
        models_url("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        "https://dashscope.aliyuncs.com/compatible-mode/v1/models"
    );
    assert_eq!(
        models_url("https://g.example/paas/v2"),
        "https://g.example/paas/v2/models"
    );
    // bare host / missing scheme
    assert_eq!(
        models_url("api.deepseek.com"),
        "https://api.deepseek.com/v1/models"
    );
    // host with non-version path: treat as full base, append /v1/models —
    // a gateway that serves the API off a subpath is the user's problem,
    // the documented base always carries the version or is the root.
    assert_eq!(
        models_url("https://x.example/api"),
        "https://x.example/api/v1/models"
    );
    assert_eq!(models_url("  "), "");
}

#[test]
fn parse_models_is_defensive_about_fields() {
    let body = serde_json::json!({
        "object": "list",
        "data": [
            { "id": "gpt-a", "object": "model", "created": 1, "owned_by": "o" },
            { "id": "with-name", "name": "With Name" },
            { "id": "anthropic-style", "display_name": "Claude Opus" },
            { "id": "both-fields", "name": "family", "display_name": "Human Label" },
            { "name": "no-id-dropped" },
            { "id": "  " },
            { "id": "extra", "unknown_field": { "nested": true } }
        ]
    });
    let out = parse_models_body(&body);
    let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "gpt-a",
            "with-name",
            "anthropic-style",
            "both-fields",
            "extra"
        ]
    );
    assert_eq!(out[2].name.as_deref(), Some("Claude Opus"));
    assert_eq!(
        out[3].name.as_deref(),
        Some("Human Label"),
        "display_name wins over name"
    );
    assert_eq!(out[0].name, None);
    // total garbage → empty, not an error
    assert!(parse_models_body(&serde_json::json!({ "models": [] })).is_empty());
}

#[test]
fn api_error_prefers_provider_message() {
    let openai = r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error"}}"#;
    assert_eq!(
        api_error_message(401, openai),
        "端点返回错误 (401): Incorrect API key"
    );
    let plain = "Unauthorized";
    assert_eq!(api_error_message(401, plain), "端点返回 HTTP 401");
    let err_string = r#"{"error":"quota exceeded"}"#;
    assert!(api_error_message(429, err_string).contains("quota exceeded"));
}

#[tokio::test]
async fn missing_env_key_short_circuits_with_prefix() {
    // No network is reached: the key question must fail fast, and the
    // ENV_MISSING marker is what the UI keys its temp-key input on.
    let e = fetch_models_inner(
        &test_creds(),
        "https://example.invalid/v1".into(),
        None,
        "PHL_TEST_DEFINITELY_UNSET_KEY".into(),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(e.starts_with(ENV_MISSING), "unexpected error shape: {e}");
}

#[tokio::test]
async fn empty_base_url_rejected() {
    assert!(fetch_models_inner(
        &test_creds(),
        "".into(),
        None,
        "X".into(),
        Some("k".into()),
        None
    )
    .await
    .is_err());
}

/// End-to-end against a throwaway local server: exercises the real
/// reqwest path — URL joining, the Bearer header, and the tolerant parse
/// — without depending on the public internet.
#[tokio::test]
async fn fetch_models_against_local_server() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
        assert!(req.contains("get /v1/models"), "request line wrong:\n{req}");
        assert!(req.contains("bearer test-key"), "auth header wrong:\n{req}");
        let body = r#"{"object":"list","data":[{"id":"m1","created":1},{"id":"m2","display_name":"Two","unknown":true}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let out = fetch_models_inner(
        &test_creds(),
        format!("http://{addr}"),
        None,
        "IRRELEVANT_WHEN_TEMP_KEY_GIVEN".into(),
        Some("test-key".into()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        out.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert_eq!(out[1].name.as_deref(), Some("Two"));
    server.await.unwrap();
}

/// Real-network probe against a public OpenAI-compatible endpoint that
/// lists models without auth when possible. Run manually:
/// `cargo test -- --ignored provider_models_probe`
#[tokio::test]
#[ignore]
async fn provider_models_probe() {
    // DeepSeek's endpoint accepts any well-formed key for /models on some
    // deployments; if it 401s, the error text itself is the probe output.
    match fetch_models_inner(
        &test_creds(),
        "https://api.deepseek.com".into(),
        None,
        "DEEPSEEK_API_KEY".into(),
        None,
        None,
    )
    .await
    {
        Ok(models) => {
            println!(
                "models: {} → {:?}",
                models.len(),
                models.iter().map(|m| &m.id).collect::<Vec<_>>()
            );
        }
        Err(e) => println!("probe reported: {e}"),
    }
}

fn launch_manifest(id: &str, api: Option<ApiBinding>) -> crate::instances::InstanceManifest {
    crate::instances::InstanceManifest {
        schema_version: 0,
        id: id.into(),
        name: id.into(),
        note: None,
        kind: "sandbox".into(),
        hue: 0,
        version_id: "dsh-0.1.0".into(),
        runtime_id: "node-22".into(),
        port: 8080,
        auto_port: true,
        profile: "default".into(),
        created_at: String::new(),
        last_run_at: None,
        total_runtime: 0,
        favorite: false,
        env: std::collections::HashMap::new(),
        args: Vec::new(),
        api,
        management_mode: Default::default(),
        source: Default::default(),
        external_home: None,
        adopted_from: None,
    }
}

#[tokio::test]
async fn launch_observes_managed_drift_without_touching_settings() {
    // The §G promise in one chain: a synced instance whose settings.yaml the
    // user later edits must survive launch byte-for-byte. note_launch_drift
    // computes what a re-sync WOULD write — it must never write it.
    let (root, dir) = temp_root_with_instance("launch-drift");
    let c = config();
    std::fs::create_dir_all(api_config_path(&root).parent().unwrap()).unwrap();
    std::fs::write(api_config_path(&root), serde_json::to_vec(&c).unwrap()).unwrap();
    let binding = sync_inner(&dir, &ApiBinding::default(), &c).await.unwrap();
    assert!(binding.synced_hash.is_some(), "fixture is a synced binding");
    let path = settings_path(&dir);

    // (a) no drift: the merge is identity; still nothing is written.
    let synced_bytes = std::fs::read(&path).unwrap();
    note_launch_drift(
        &root,
        &dir,
        &launch_manifest("launch-drift", Some(binding.clone())),
    )
    .await;
    assert_eq!(std::fs::read(&path).unwrap(), synced_bytes);

    // (b) drift: a local edit inside the managed section. The merge would
    // rewrite it — the file must come out of note_launch_drift untouched.
    let raw = std::fs::read_to_string(&path).unwrap();
    let edited = raw.replace("deepseek-v4", "my-local-pick");
    assert_ne!(edited, raw, "fixture edit must land in the managed section");
    std::fs::write(&path, &edited).unwrap();
    let drifted_bytes = std::fs::read(&path).unwrap();
    note_launch_drift(
        &root,
        &dir,
        &launch_manifest("launch-drift", Some(binding.clone())),
    )
    .await;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        drifted_bytes,
        "the local edit survives launch observation"
    );

    // (c) short-circuits: unbound and explicitly un-managed bindings.
    for manifest in [
        launch_manifest("launch-drift", None),
        launch_manifest(
            "launch-drift",
            Some(ApiBinding {
                inheritance: "none".into(),
                ..Default::default()
            }),
        ),
    ] {
        note_launch_drift(&root, &dir, &manifest).await;
        assert_eq!(std::fs::read(&path).unwrap(), drifted_bytes);
    }

    let _ = std::fs::remove_dir_all(root);
}
