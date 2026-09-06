use super::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn sample() -> Value {
    json!({"openai": {"id":"openai", "models": {
        "gpt-5": {"id":"gpt-5", "name":"GPT-5", "limit":{"context":400000,"output":128000},
            "modalities":{"input":["text","image","audio"]}, "reasoning":true,
            "reasoning_options":[{"type":"effort","values":["low","high","xhigh","bogus"]}]},
        "plain": {"id":"plain", "reasoning":false},
        "unknown": {"id":"unknown", "reasoning":true}
    }}})
}
fn model(id: &str) -> ModelRef {
    ModelRef {
        id: id.into(),
        ..Default::default()
    }
}
fn entry(provider: &str, id: &str) -> Entry {
    Entry {
        provider: provider.into(),
        model: model(id),
    }
}
fn hit<'a>(rows: &'a [Entry], id: &str, provider: Option<&str>) -> &'a Entry {
    match lookup(rows, id, provider) {
        Match::Found(e) => e,
        _ => panic!("expected unique hit for {id}"),
    }
}

#[test]
fn parses_capacities_modalities_and_explicit_efforts() {
    let rows = parse(&sample());
    let m = &hit(&rows, "gpt-5", None).model;
    assert_eq!(m.context_window, Some(400000));
    assert_eq!(m.max_tokens, Some(128000));
    assert_eq!(m.input, Some(vec![ModelInput::Text, ModelInput::Image]));
    assert_eq!(
        serde_json::to_value(&m.reasoning_efforts).unwrap(),
        json!({"low":"low","high":"high","xhigh":"xhigh"})
    );
    assert_eq!(
        hit(&rows, "plain", None).model.reasoning_efforts,
        Some(ReasoningEfforts::Disabled(FalseValue))
    );
    let unknown = &hit(&rows, "unknown", None).model;
    assert!(unknown.reasoning_efforts.is_none());
    assert!(unknown.input.is_none());
    assert!(unknown.context_window.is_none());
    assert!(unknown.max_tokens.is_none());
    assert!(unknown.name.is_none());
}

#[test]
fn parser_ignores_invalid_rows_and_does_not_infer_vision_or_thinking() {
    let rows = parse(&json!({"p":{"models":{
        "bad": {"id":""}, "missing": {},
        "ok": {"id":"ok", "attachment":true,"limit":{"context":-1,"output":1.5},
            "reasoning_options":[{"type":"effort","values":["off","unsupported"]}]}
    }}}));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider, "p");
    assert!(rows[0].model.context_window.is_none());
    assert!(rows[0].model.max_tokens.is_none());
    assert!(rows[0].model.input.is_none());
    assert!(rows[0].model.reasoning_efforts.is_none());
    assert!(parse(&json!([])).is_empty());
}

#[test]
fn exact_id_wins_before_prefix_and_tail() {
    let rows = vec![entry("router", "openai/gpt-5"), entry("openai", "gpt-5")];
    assert_eq!(
        hit(&rows, "openai/gpt-5", Some("openai")).provider,
        "router"
    );
    assert_eq!(hit(&rows, " GPT-5 ", None).provider, "openai");
}

#[test]
fn explicit_provider_resolves_nested_paths_and_duplicate_ids() {
    let rows = vec![entry("openai", "gpt-5"), entry("azure", "gpt-5")];
    assert_eq!(
        hit(&rows, "openrouter/openai/gpt-5", None).provider,
        "openai"
    );
    assert_eq!(hit(&rows, "openai:gpt-5", Some("azure")).provider, "openai");
    assert_eq!(hit(&rows, "gpt-5", Some("azure")).provider, "azure");
    assert!(matches!(lookup(&rows, "gpt-5", None), Match::Ambiguous));
}

#[test]
fn stripped_and_tail_matches_must_be_unique() {
    let rows = vec![entry("owner", "model")];
    assert_eq!(hit(&rows, "custom/path/model", None).provider, "owner");
    let rows = vec![entry("owner", "family/model")];
    assert_eq!(hit(&rows, "model", None).model.id, "family/model");
    let rows = vec![entry("a", "family/model"), entry("b", "other/model")];
    assert!(matches!(lookup(&rows, "model", None), Match::Ambiguous));
    assert!(matches!(lookup(&rows, "unknown", None), Match::Missing));
    assert!(matches!(lookup(&rows, "", None), Match::Missing));
}

#[test]
fn ambiguity_never_depends_on_catalog_order_or_falls_through() {
    let mut rows = vec![
        entry("a", "vendor/model"),
        entry("b", "vendor/model"),
        entry("vendor", "model"),
    ];
    assert!(matches!(
        lookup(&rows, "vendor/model", None),
        Match::Ambiguous
    ));
    rows.reverse();
    assert!(matches!(
        lookup(&rows, "vendor/model", None),
        Match::Ambiguous
    ));
    let rows = vec![entry("openai", "gpt-5"), entry("anthropic", "claude")];
    assert!(matches!(
        lookup(&rows, "anthropic/gpt-5", None),
        Match::Missing
    ));
}

#[test]
fn enrichment_preserves_every_present_field_even_empty_and_false() {
    let rows = parse(&sample());
    let mut m: ModelRef =
        serde_json::from_value(json!({"id":"gpt-5","name":"mine","contextWindow":123,
        "maxTokens":45,"input":[],"reasoningEfforts":false}))
        .unwrap();
    let r = enrich(m.clone(), &rows, None);
    assert!(!r.changed);
    assert_eq!(r.model, m);
    m.reasoning_efforts = Some(ReasoningEfforts::Levels(BTreeMap::new()));
    m.name = Some(String::new());
    assert_eq!(enrich(m.clone(), &rows, None).model, m);
    m.max_tokens = None;
    let r = enrich(m, &rows, None);
    assert_eq!(r.model.context_window, Some(123));
    assert_eq!(r.model.max_tokens, Some(128000));
    assert_eq!(r.model.metadata_sources.len(), 1);
    assert_eq!(
        r.model.metadata_sources["maxTokens"],
        MetadataSource::ModelsDev
    );
    assert!(!enrich(r.model, &rows, None).changed);
}

#[test]
fn fallback_is_labelled_per_field_and_never_claims_reasoning() {
    let r = enrich(model("unknown-model"), &[], None);
    assert!(!r.matched);
    assert_eq!(r.model.context_window, Some(262144));
    assert_eq!(r.model.max_tokens, Some(32768));
    assert_eq!(r.model.input, Some(vec![ModelInput::Text]));
    assert!(r.model.reasoning_efforts.is_none());
    assert!(r.model.name.is_none());
    assert!(r
        .model
        .metadata_sources
        .values()
        .all(|s| *s == MetadataSource::Fallback));
    let r = enrich(model("plain"), &parse(&sample()), None);
    assert!(r.matched);
    assert_eq!(
        r.model.metadata_sources["contextWindow"],
        MetadataSource::Fallback
    );
    assert_eq!(
        r.model.metadata_sources["reasoningEfforts"],
        MetadataSource::ModelsDev
    );
    let r = enrich(
        model("same"),
        &[entry("a", "same"), entry("b", "same")],
        None,
    );
    assert!(r.ambiguous);
    assert!(!r.matched);
    assert!(r.model.reasoning_efforts.is_none());
    assert!(!enrich(model(""), &[], None).changed);
}

#[test]
fn old_json_loads_and_new_fields_roundtrip_without_accepting_true() {
    let old: ModelRef = serde_json::from_value(json!({"id":"xxx","contextWindow":200000})).unwrap();
    assert!(old.input.is_none());
    assert!(old.reasoning_efforts.is_none());
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        json!({"id":"xxx","contextWindow":200000})
    );
    for efforts in [json!(false), json!({"off":null,"high":"custom-high"})] {
        let raw = json!({"id":"x","input":["text","image"],"reasoningEfforts":efforts});
        let parsed: ModelRef = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), raw);
    }
    assert!(serde_json::from_value::<ModelRef>(json!({"id":"x","reasoningEfforts":true})).is_err());
}

#[test]
fn rejects_invalid_dsh_reasoning_declarations_without_mutating_them() {
    for efforts in [
        json!({}),
        json!({"off":null}),
        json!({"high":null}),
        json!({"high":""}),
        json!({"bogus":"bogus"}),
    ] {
        let m: ModelRef =
            serde_json::from_value(json!({"id":"x","reasoningEfforts":efforts})).unwrap();
        assert!(m.validate_capabilities().is_err());
    }
    for efforts in [json!(false), json!({"off":null,"high":"custom-high"})] {
        let m: ModelRef =
            serde_json::from_value(json!({"id":"x","reasoningEfforts":efforts})).unwrap();
        assert!(m.validate_capabilities().is_ok());
    }
}

#[tokio::test]
#[ignore = "live models.dev network probe; run explicitly"]
async fn live_catalog_probe() {
    let root = temp_root("live-probe");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut cache = CatalogCache::default();
    cache.load(&root, URL, now).await;
    let result = enrich(model("gpt-5"), &cache.entries, Some("openai"));
    let count = cache.entries.len();
    let persisted = root.join("cache/models-dev.json").exists();
    let _ = tokio::fs::remove_dir_all(&root).await;
    assert!(count > 0, "live catalog was unavailable");
    assert!(persisted);
    assert!(result.matched);
    assert!(result.model.context_window.is_some());
    assert!(result
        .model
        .input
        .as_ref()
        .unwrap()
        .contains(&ModelInput::Image));
    println!("live catalog: {count} entries; GPT-5 matched with capacities and image input; cache written");
}

fn temp_root(tag: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("phl-catalog-{tag}-{}-{id}", std::process::id()))
}

async fn write_cache(root: &Path, stamp: u64) {
    tokio::fs::create_dir_all(root.join("cache")).await.unwrap();
    tokio::fs::write(
        root.join("cache/models-dev.json"),
        serde_json::to_vec(&CacheFile {
            version: 1,
            fetched_at: stamp,
            data: sample(),
        })
        .unwrap(),
    )
    .await
    .unwrap();
}

async fn server(body: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let received = socket.read(&mut request).await.unwrap();
        assert!(received > 0);
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn fresh_cache_needs_no_network_and_expired_cache_refreshes_atomically() {
    let root = temp_root("fresh");
    write_cache(&root, 100).await;
    let mut cache = CatalogCache::default();
    cache.load(&root, "invalid-url", 100 + TTL - 1).await;
    assert_eq!(cache.retry_at, 0, "fresh cache must not attempt a request");
    let (url, task) = server(sample().to_string()).await;
    cache.load(&root, &url, 100 + TTL).await;
    task.await.unwrap();
    assert_eq!(cache.fetched_at, Some(100 + TTL));
    let saved: CacheFile = serde_json::from_slice(
        &tokio::fs::read(root.join("cache/models-dev.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.fetched_at, 100 + TTL);
    assert!(!root.join("cache/models-dev.json.tmp").exists());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn refresh_failure_uses_old_cache_and_backoff_avoids_repeated_requests() {
    let root = temp_root("stale");
    write_cache(&root, 100).await;
    let mut cache = CatalogCache::default();
    cache.load(&root, "invalid-url", 100 + TTL).await;
    assert_eq!(cache.fetched_at, Some(100));
    assert!(!cache.entries.is_empty());
    let retry = cache.retry_at;
    cache.load(&root, "invalid-url", 100 + TTL + 1).await;
    assert_eq!(cache.retry_at, retry);
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn corrupt_empty_or_absent_catalog_degrades_without_breaking_enrichment() {
    let root = temp_root("missing");
    let mut cache = CatalogCache::default();
    cache.load(&root, "invalid-url", 100).await;
    assert!(cache.fetched_at.is_none());
    assert!(enrich(model("x"), &cache.entries, None).changed);
    tokio::fs::create_dir_all(root.join("cache")).await.unwrap();
    tokio::fs::write(root.join("cache/models-dev.json"), b"broken")
        .await
        .unwrap();
    let mut cache = CatalogCache::default();
    let (url, task) = server("{}".into()).await;
    cache.load(&root, &url, 100).await;
    task.await.unwrap();
    assert!(cache.fetched_at.is_none());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn invalid_refresh_does_not_replace_good_disk_cache() {
    let root = temp_root("invalid");
    write_cache(&root, 100).await;
    let before = tokio::fs::read(root.join("cache/models-dev.json"))
        .await
        .unwrap();
    let (url, task) = server("[]".into()).await;
    let mut cache = CatalogCache::default();
    cache.load(&root, &url, 100 + TTL).await;
    task.await.unwrap();
    assert_eq!(cache.fetched_at, Some(100));
    assert_eq!(
        tokio::fs::read(root.join("cache/models-dev.json"))
            .await
            .unwrap(),
        before
    );
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn cache_write_failure_keeps_downloaded_metadata_usable() {
    let root = temp_root("readonly");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("cache"), b"file blocks directory")
        .await
        .unwrap();
    let (url, task) = server(sample().to_string()).await;
    let mut cache = CatalogCache::default();
    cache.load(&root, &url, 100).await;
    task.await.unwrap();
    assert!(enrich(model("gpt-5"), &cache.entries, None).matched);
    tokio::fs::remove_dir_all(root).await.unwrap();
}
