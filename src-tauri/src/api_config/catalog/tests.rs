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
fn hints(provider: Option<&str>) -> Vec<String> {
    provider.into_iter().map(|s| s.to_string()).collect()
}
fn hit<'a>(rows: &'a [Entry], id: &str, provider: Option<&str>) -> &'a Entry {
    match lookup(rows, id, &hints(provider)) {
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
    assert!(matches!(lookup(&rows, "gpt-5", &[]), Match::Ambiguous));
}

#[test]
fn stripped_and_tail_matches_must_be_unique() {
    let rows = vec![entry("owner", "model")];
    assert_eq!(hit(&rows, "custom/path/model", None).provider, "owner");
    let rows = vec![entry("owner", "family/model")];
    assert_eq!(hit(&rows, "model", None).model.id, "family/model");
    let rows = vec![entry("a", "family/model"), entry("b", "other/model")];
    assert!(matches!(lookup(&rows, "model", &[]), Match::Ambiguous));
    assert!(matches!(lookup(&rows, "unknown", &[]), Match::Missing));
    assert!(matches!(lookup(&rows, "", &[]), Match::Missing));
}

#[test]
fn ambiguity_never_depends_on_catalog_order_or_falls_through() {
    let mut rows = vec![
        entry("a", "vendor/model"),
        entry("b", "vendor/model"),
        entry("vendor", "model"),
    ];
    assert!(matches!(
        lookup(&rows, "vendor/model", &[]),
        Match::Ambiguous
    ));
    rows.reverse();
    assert!(matches!(
        lookup(&rows, "vendor/model", &[]),
        Match::Ambiguous
    ));
    let rows = vec![entry("openai", "gpt-5"), entry("anthropic", "claude")];
    assert!(matches!(
        lookup(&rows, "anthropic/gpt-5", &[]),
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
    let r = enrich(m.clone(), &rows, &[], None);
    assert!(!r.changed);
    assert_eq!(r.model, m);
    m.reasoning_efforts = Some(ReasoningEfforts::Levels(BTreeMap::new()));
    m.name = Some(String::new());
    assert_eq!(enrich(m.clone(), &rows, &[], None).model, m);
    m.max_tokens = None;
    let r = enrich(m, &rows, &[], None);
    assert_eq!(r.model.context_window, Some(123));
    assert_eq!(r.model.max_tokens, Some(128000));
    assert_eq!(r.model.metadata_sources.len(), 1);
    assert_eq!(
        r.model.metadata_sources["maxTokens"],
        MetadataSource::ModelsDev
    );
    assert!(!enrich(r.model, &rows, &[], None).changed);
}

#[test]
fn fallback_is_labelled_per_field_and_never_claims_reasoning() {
    let r = enrich(model("unknown-model"), &[], &[], None);
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
    let r = enrich(model("plain"), &parse(&sample()), &[], None);
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
        &[],
        None,
    );
    assert!(r.ambiguous);
    assert!(!r.matched);
    assert!(r.model.reasoning_efforts.is_none());
    assert!(!enrich(model(""), &[], &[], None).changed);
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

#[test]
fn canonical_layer_resolves_dated_and_aliased_ids_only_when_unique() {
    // The new-release shape: an endpoint speaks `gpt-5-20250807`, the catalog
    // carries the base — literal layers miss, the canonical one lands.
    let rows = vec![entry("openai", "gpt-5")];
    assert_eq!(hit(&rows, "gpt-5-20250807", None).model.id, "gpt-5");
    assert_eq!(hit(&rows, "gpt-5-latest", None).model.id, "gpt-5");
    assert_eq!(hit(&rows, "gpt-5:0", None).model.id, "gpt-5");
    // Ambiguity discipline is unchanged through the new layer.
    let rows = vec![entry("openai", "gpt-5"), entry("azure", "gpt-5")];
    assert!(matches!(
        lookup(&rows, "gpt-5-20250807", &[]),
        Match::Ambiguous
    ));
    assert_eq!(
        hit(&rows, "gpt-5-20250807", Some("azure")).provider,
        "azure"
    );
    // Word-shaped suffixes are NEVER stripped: `vision` is a product word,
    // not an alias — stripping it would invent a match for a different model.
    let rows = vec![entry("meta", "llama-3-instruct")];
    assert!(matches!(
        lookup(&rows, "llama-3-vision", &[]),
        Match::Missing
    ));
}

#[test]
fn base_url_host_disambiguates_where_display_names_cannot() {
    let rows = vec![
        entry("deepseek", "deepseek-chat"),
        entry("siliconflow", "deepseek-chat"),
    ];
    // The user's own label can never equal a catalog id — the old hint died
    // here with `Ambiguous` for every self-built provider.
    let legacy = hints(Some("我的硅基流动"));
    assert!(matches!(
        lookup(&rows, "deepseek-chat", &legacy),
        Match::Ambiguous
    ));
    // The configured endpoint says exactly who serves it.
    let host = host_providers(Some("https://api.siliconflow.cn/v1"));
    assert_eq!(host, vec!["siliconflow".to_string()]);
    assert_eq!(
        hit(&rows, "deepseek-chat", Some(&host[0])).provider,
        "siliconflow"
    );
    assert!(host_providers(Some("https://gateway.example.internal/v1")).is_empty());
}

#[test]
fn fallback_sourced_fields_are_replaced_when_catalogs_learn_the_model() {
    // First run: unknown release, everything lands on compat guesses.
    let first = enrich(model("brand-new-model"), &[], &[], None);
    assert_eq!(first.model.context_window, Some(262144));
    assert_eq!(
        first.model.metadata_sources["contextWindow"],
        MetadataSource::Fallback
    );
    // The release reaches models.dev; the next enrichment must correct the
    // guess (the old None-only rule made the guess permanent).
    let rows = vec![Entry {
        provider: "acme".into(),
        model: ModelRef {
            id: "brand-new-model".into(),
            context_window: Some(131072),
            ..Default::default()
        },
    }];
    let second = enrich(first.model.clone(), &rows, &[], None);
    assert_eq!(second.model.context_window, Some(131072));
    assert_eq!(
        second.model.metadata_sources["contextWindow"],
        MetadataSource::ModelsDev
    );
    // A manual value survives any catalog.
    let mut manual = first.model.clone();
    manual.context_window = Some(777);
    manual
        .metadata_sources
        .insert("contextWindow".into(), MetadataSource::Manual);
    let third = enrich(manual, &rows, &[], None);
    assert_eq!(third.model.context_window, Some(777));
}

#[test]
fn openrouter_rows_parse_and_resolve_tail_vendor_and_canonical_forms() {
    let rows = parse_openrouter(&json!({"data": [
        {"id":"deepseek/deepseek-v4","context_length":1048576,
         "architecture":{"input_modalities":["text","image"]}},
        {"id":"openai/gpt-x","context_length":400000,
         "supported_parameters":["tools","reasoning"]},
    ]}));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].context_window, Some(1048576));
    assert_eq!(
        rows[0].input,
        Some(vec![ModelInput::Text, ModelInput::Image])
    );
    // A bare tail name resolves when unique; a vendor-qualified id matches;
    // a dated id falls back to the canonical base.
    assert_eq!(
        lookup_openrouter(&rows, "deepseek-v4", &[]).unwrap().id,
        "deepseek/deepseek-v4"
    );
    assert_eq!(
        lookup_openrouter(&rows, "openai/gpt-x", &[]).unwrap().id,
        "openai/gpt-x"
    );
    assert_eq!(
        lookup_openrouter(&rows, "openai/gpt-x-20260101", &[])
            .unwrap()
            .id,
        "openai/gpt-x"
    );
    // Nothing invents: unknown ids and vendor ambiguity answer None.
    assert!(lookup_openrouter(&rows, "nosuch-model", &[]).is_none());
    let twin = parse_openrouter(&json!({"data":[
        {"id":"a/pro","context_length":1},{"id":"b/pro","context_length":2}]}));
    assert!(lookup_openrouter(&twin, "pro", &[]).is_none());
    assert_eq!(
        lookup_openrouter(&twin, "pro", &["b".into()]).unwrap().id,
        "b/pro"
    );
}

#[test]
fn openrouter_is_consulted_only_for_openrouter_shaped_requests() {
    assert!(uses_openrouter(&[model("openai/gpt-x")], None));
    assert!(!uses_openrouter(&[model("gpt-x")], None));
    assert!(uses_openrouter(
        &[model("any")],
        Some("https://openrouter.ai/api/v1")
    ));
    assert!(!uses_openrouter(
        &[model("any")],
        Some("https://api.deepseek.com")
    ));
}

#[test]
fn cache_ttl_is_daily_so_fresh_releases_arrive_fast() {
    // A 48h-old cache must refresh on the next enrich attempt (the old week
    // made new releases systematically invisible for days).
    assert_eq!(TTL, 24 * 60 * 60);
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
    cache.load(&root, URL, now, false).await;
    let result = enrich(model("gpt-5"), &cache.entries, &["openai".into()], None);
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
    cache.load(&root, "invalid-url", 100 + TTL - 1, false).await;
    assert_eq!(cache.retry_at, 0, "fresh cache must not attempt a request");
    let (url, task) = server(sample().to_string()).await;
    cache.load(&root, &url, 100 + TTL, false).await;
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
    cache.load(&root, "invalid-url", 100 + TTL, false).await;
    assert_eq!(cache.fetched_at, Some(100));
    assert!(!cache.entries.is_empty());
    let retry = cache.retry_at;
    cache.load(&root, "invalid-url", 100 + TTL + 1, false).await;
    assert_eq!(cache.retry_at, retry);
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn corrupt_empty_or_absent_catalog_degrades_without_breaking_enrichment() {
    let root = temp_root("missing");
    let mut cache = CatalogCache::default();
    cache.load(&root, "invalid-url", 100, false).await;
    assert!(cache.fetched_at.is_none());
    assert!(enrich(model("x"), &cache.entries, &[], None).changed);
    tokio::fs::create_dir_all(root.join("cache")).await.unwrap();
    tokio::fs::write(root.join("cache/models-dev.json"), b"broken")
        .await
        .unwrap();
    let mut cache = CatalogCache::default();
    let (url, task) = server("{}".into()).await;
    cache.load(&root, &url, 100, false).await;
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
    cache.load(&root, &url, 100 + TTL, false).await;
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
    cache.load(&root, &url, 100, false).await;
    task.await.unwrap();
    assert!(enrich(model("gpt-5"), &cache.entries, &[], None).matched);
    tokio::fs::remove_dir_all(root).await.unwrap();
}
