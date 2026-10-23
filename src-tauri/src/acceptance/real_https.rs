//! Read-only HTTPS smoke for dependency security updates. This uses the
//! production catalog clients, checks the configured updater feed, and never
//! downloads/installs an update or reads a user's DSH_HOME or credentials.
//!
//! Run with PHL_REAL_HTTPS=1 cargo test --locked --lib real_https --
//! --ignored --nocapture. This does not exercise the native updater UI.

use std::time::Duration;

use serde_json::Value;

async fn get_json(client: &reqwest::Client, url: &str) -> Value {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap_or_else(|error| panic!("HTTPS request to {url} failed: {error}"))
        .error_for_status()
        .unwrap_or_else(|error| panic!("HTTPS status from {url} failed: {error}"));
    println!("HTTPS {} {url}", response.status());
    response
        .json()
        .await
        .unwrap_or_else(|error| panic!("JSON from {url} failed: {error}"))
}

#[tokio::test]
#[ignore = "real HTTPS smoke; opt-in with PHL_REAL_HTTPS=1"]
async fn real_https_catalogs_and_updater_feed() {
    if std::env::var("PHL_REAL_HTTPS").as_deref() != Ok("1") {
        println!("SKIP real_https: set PHL_REAL_HTTPS=1");
        return;
    }

    let client = crate::versions::http_client();
    let releases = get_json(
        &client,
        "https://api.github.com/repos/deepseek-ai/deepseek-harness/releases?per_page=1",
    )
    .await;
    assert!(releases[0]["tag_name"].as_str().is_some());

    let package = get_json(
        &crate::plugins::http_client(),
        "https://registry.npmjs.org/@deepseek-ai%2Fdsh/latest",
    )
    .await;
    assert_eq!(package["name"], "@deepseek-ai/dsh");

    let config: Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let endpoints = config["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("configured updater endpoints");
    assert!(!endpoints.is_empty());
    for endpoint in endpoints {
        let endpoint = endpoint.as_str().unwrap();
        assert!(endpoint.starts_with("https://"));
        let manifest = get_json(&client, endpoint).await;
        let release: tauri_plugin_updater::RemoteRelease =
            serde_json::from_value(manifest.clone()).expect("valid Tauri updater manifest");
        println!("updater feed version {}", release.version);
        let windows = &manifest["platforms"]["windows-x86_64"];
        let url = windows["url"].as_str().expect("Windows updater URL");
        assert!(url.starts_with("https://") && url.ends_with(".exe"));
        assert!(!windows["signature"].as_str().unwrap_or_default().is_empty());
    }
}
