//! Real-upstream acceptance: the **official DSH packages from npm**, through
//! the real install pipeline, launched for real, two versions side by side.
//!
//! Everything else in this repository verifies PHL against controlled fixtures
//! (a fake DSH tarball served by a local mock registry). Those fixtures prove
//! PHL's own machinery; they cannot prove that the real upstream package from
//! `registry.npmjs.org` installs, boots, authenticates and coexists. That is
//! what this lane is for — and why it is NOT part of any gate:
//!
//! * it downloads from the public registry and runs `npm install` for the
//!   version's own dependency tree,
//! * it starts two real DSH web servers on this machine.
//!
//! Run it explicitly (opt-in, network required):
//!
//! ```text
//! PHL_REAL_DSH=1 cargo test --lib real_dsh -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Without `PHL_REAL_DSH=1` the test prints a SKIP line and returns, so an
//! accidental `--ignored` sweep cannot reach the network.
//!
//! Boundaries, stated rather than implied: this is the acceptance contract's
//! A01/A02 (real versions install → launch → authenticated WebUI → stop) and
//! the dual-instance half of C01/C06/C14/C15 (two versions, two HOMEs, two
//! ports, both serving at once). It does NOT cover plugins inside a real DSH,
//! the installer lifecycle or the DPI matrix.

use std::collections::HashMap;
use std::path::Path;

use super::super::release_e2e::driver::{launch, with_cleanup, Session};
use super::super::release_e2e::fixtures::NodeInfo;

/// The two upstream versions the baseline pins: the brief's upgrade target and
/// the fixed older baseline (never a floating dist-tag).
pub(super) const TARGET_VERSION: &str = "0.1.7-rc.2";
const OLD_VERSION: &str = "0.1.5-rc.3";
pub(super) const REGISTRY: &str = "https://registry.npmjs.org";

pub(super) fn opted_in() -> bool {
    std::env::var("PHL_REAL_DSH").ok().as_deref() == Some("1")
}

/// Install one real version through the real pipeline (`run_install`: download
/// → integrity → extract → dependency install → health gate → promote).
pub(super) async fn install_real(
    session: &Session,
    node: &Path,
    name: &str,
    tarball: &str,
    integrity: Option<&str>,
) {
    let id = format!("real-version-{name}");
    let flag = session.transfers.take(&id);
    let root = session.root();
    let registry = REGISTRY.to_string();
    let task = crate::resources::Tasks::default()
        .begin(
            crate::resources::TaskInfo::new(
                id.clone(),
                "version-install",
                format!("安装 DSH {name}"),
                &[],
            ),
            Some(flag.clone()),
        )
        .unwrap();
    let result = crate::versions::install::run_install(
        &flag,
        tarball,
        integrity,
        name,
        &root,
        &registry,
        false,
        None,
        &task,
        &crate::release_e2e::journeys::progress_channel(),
    )
    .await;
    session.transfers.release(&id);
    result.unwrap_or_else(|e| panic!("real DSH {name} install failed: {e}"));
    // The version's own dependency tree is part of a working install: without
    // it `dsh web` cannot resolve its imports.
    let dir = root.join("versions").join(name);
    assert!(
        dir.join("node_modules").exists() || !crate::versions::package_requires_deps(&dir),
        "real DSH {name}: dependencies must be materialised for a launch to be meaningful"
    );
    let _ = node;
}

/// One real instance, created through the real create path with a fixed port
/// left to the allocator (`auto_port`).
pub(super) async fn create_real_instance(
    session: &Session,
    id: &str,
    version_id: &str,
) -> std::path::PathBuf {
    let manifest: crate::instances::InstanceManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2,
        "id": id,
        "name": format!("真实 DSH {id}"),
        "kind": "sandbox",
        "hue": 0,
        "versionId": version_id,
        "runtimeId": "node-system",
        "port": 0,
        "autoPort": true,
        "profile": "web",
        "createdAt": crate::versions::now_iso(),
    }))
    .unwrap();
    crate::instances::create_instance_inner(&session.root(), manifest)
        .await
        .unwrap_or_else(|e| panic!("create {id} failed: {e}"));
    session.root().join("instances").join(id)
}

/// The real DSH WebUI handshake, as the browser performs it:
///
/// ```text
/// GET /?token=<t>  ->  303 See Other, Set-Cookie: dsh-auth-…=<signed>, Location: ./
/// GET /            ->  200, carrying that cookie
/// ```
///
/// Measured on 2026-09-26 against `@deepseek-ai/dsh@0.1.7-rc.2` (see
/// `real_dsh_webui_auth_probe`): a plain GET that follows the redirect WITHOUT
/// a cookie jar lands on the 401 page, and neither `Cookie: token=…` nor
/// `Authorization: Bearer` is accepted. PHL itself is not in this path — it
/// hands the URL to the WebView2 window, a real browser — so the lane has to
/// reproduce the browser rather than invent a simpler request.
struct WebAuth {
    /// Status of the token URL (the real server answers 303).
    handshake_status: u16,
    /// The cookie the token URL handed out (DSH's own, `dsh-auth-…`).
    cookie_name: String,
    /// The redirect chain the authenticated client walked, for the record.
    chain: Vec<u16>,
    status: u16,
    without_cookie_status: u16,
    body_bytes: usize,
}

/// Resolve a (possibly relative) Location against the URL it arrived from.
fn resolve_location(current: &str, location: &str) -> String {
    if location.starts_with("http") {
        return location.to_string();
    }
    let origin = current
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(|host| format!("{}://{host}", current.split("://").next().unwrap_or("http")))
        .unwrap_or_else(|| current.to_string());
    format!(
        "{origin}/{}",
        location.trim_start_matches("./").trim_start_matches('/')
    )
}

async fn web_authenticated(url: &str) -> WebAuth {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("no-redirect client");
    let first = client
        .get(url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {url} failed: {e}"));
    let handshake_status = first.status().as_u16();
    let set_cookie = first
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let location = first
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("./")
        .to_string();
    let cookie_pair = set_cookie.split(';').next().unwrap_or_default().to_string();
    let cookie_name = cookie_pair
        .split('=')
        .next()
        .unwrap_or_default()
        .to_string();
    let base = url
        .split("?token=")
        .next()
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string();
    let mut target = resolve_location(&base, &location);

    // The browser keeps following with the cookie until it gets the document;
    // the lane does the same, bounded.
    let mut chain = Vec::new();
    let mut status = 0u16;
    let mut body = String::new();
    for _ in 0..5 {
        let res = client
            .get(&target)
            .header(reqwest::header::COOKIE, cookie_pair.clone())
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {target} (with cookie) failed: {e}"));
        status = res.status().as_u16();
        chain.push(status);
        if (300..400).contains(&status) {
            if let Some(next) = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                target = resolve_location(&target, next);
                continue;
            }
        }
        body = res.text().await.unwrap_or_default();
        break;
    }

    let without_cookie = client
        .get(&target)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {target} (no cookie) failed: {e}"));

    WebAuth {
        handshake_status,
        cookie_name,
        chain,
        status,
        without_cookie_status: without_cookie.status().as_u16(),
        body_bytes: body.len(),
    }
}

#[tokio::test]
#[ignore = "real-upstream acceptance: needs network + npm; opt-in via PHL_REAL_DSH=1"]
async fn real_dsh_upstream_install_launch_and_coexist() {
    if !opted_in() {
        println!(
            "SKIP real_dsh_upstream_install_launch_and_coexist: set PHL_REAL_DSH=1 to run \
             (downloads {TARGET_VERSION} and {OLD_VERSION} from {REGISTRY} and starts two real DSH servers)"
        );
        return;
    }
    let Some(node) = NodeInfo::discover() else {
        panic!(
            "real DSH acceptance: no `node` on PATH — the lane drives real DSH with the host Node"
        );
    };
    println!(
        "real DSH acceptance: host node {} ({})",
        node.version,
        node.exe.display()
    );

    let session = Session::fresh("real-dsh");
    with_cleanup(&session, async {
        // 1. The real catalog: both pinned versions must be published, with a
        //    tarball the pipeline can fetch (and a digest where npm offers one).
        let catalog = crate::versions::list_dsh_versions(REGISTRY.to_string())
            .await
            .expect("the real npm catalog must be reachable");
        let pick = |name: &str| {
            catalog
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("{name} is not in the published catalog"))
        };
        let target = pick(TARGET_VERSION);
        let old = pick(OLD_VERSION);
        let target_src = target.source.as_ref().expect("target has an install source");
        let old_src = old.source.as_ref().expect("old baseline has an install source");
        println!(
            "catalog: {TARGET_VERSION} -> {} (integrity {}), {OLD_VERSION} -> {}",
            target_src.tarball,
            target_src.integrity.is_some(),
            old_src.tarball
        );

        // 2. Real install ×2 through the real pipeline (download → verify →
        //    extract → npm install → health gate → promote).
        install_real(
            &session,
            &node.exe,
            TARGET_VERSION,
            &target_src.tarball,
            target_src.integrity.as_deref(),
        )
        .await;
        install_real(
            &session,
            &node.exe,
            OLD_VERSION,
            &old_src.tarball,
            old_src.integrity.as_deref(),
        )
        .await;
        for name in [TARGET_VERSION, OLD_VERSION] {
            let dir = session.root().join("versions").join(name);
            assert!(dir.join("phl-install.json").exists(), "{name}: install marker");
            assert!(dir.join("lib").join("bin.js").exists(), "{name}: entrypoint");
            // The marker exists and parses — the same signal the version page
            // and the instance binding read.
            assert!(
                crate::versions::read_marker(&dir).await.is_some(),
                "{name}: install marker parses"
            );
        }

        // 3. Two instances, one per version, both on the host Node.
        let a = create_real_instance(&session, "real-a", &format!("dsh-{TARGET_VERSION}")).await;
        let b = create_real_instance(&session, "real-b", &format!("dsh-{OLD_VERSION}")).await;
        assert_ne!(
            crate::instances::home_of(&a, &crate::instances::load_manifest(&a, "real-a").await.unwrap()),
            crate::instances::home_of(&b, &crate::instances::load_manifest(&b, "real-b").await.unwrap()),
            "the two instances must own separate DSH_HOMEs"
        );

        // 4. Launch A (the pinned target) and prove it really serves.
        let held_a = crate::release_e2e::driver::lock_instance(&session.locks, "real-a");
        let launched_a = launch(
            &session,
            &held_a,
            "real-a-launch",
            "real-a",
            TARGET_VERSION,
            "node-system",
            REGISTRY,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &crate::release_e2e::journeys::progress_channel(),
        )
        .await
        .expect("real DSH 0.1.7-rc.2 must launch");
        let port_a = launched_a.outcome.port;
        let url_a = launched_a
            .outcome
            .web_url
            .clone()
            .expect("a real `dsh web` prints its authenticated URL");
        assert!(url_a.contains("token="), "the URL carries this boot's token: {url_a}");
        let auth_a = web_authenticated(&url_a).await;
        assert_eq!(
            auth_a.handshake_status, 303,
            "the token URL must answer the cookie handshake (303), got {}",
            auth_a.handshake_status
        );
        assert!(
            auth_a.cookie_name.starts_with("dsh-auth-"),
            "the handshake hands out DSH's own auth cookie, got {:?}",
            auth_a.cookie_name
        );
        assert_eq!(auth_a.status, 200, "the WebUI must serve with that cookie");
        assert_eq!(
            auth_a.without_cookie_status, 401,
            "and must refuse the same path without it (the 200 really came from the auth)"
        );
        assert!(auth_a.body_bytes > 200, "the served page is a document, not an error stub");
        println!(
            "A: {TARGET_VERSION} on :{port_a} -> handshake {} / chain {:?} -> 200 ({} bytes), no-cookie {}",
            auth_a.handshake_status, auth_a.chain, auth_a.body_bytes, auth_a.without_cookie_status
        );

        // 5. Launch B (the fixed old baseline) WHILE A runs: coexistence is the
        //    product promise, not a nicety.
        let held_b = crate::release_e2e::driver::lock_instance(&session.locks, "real-b");
        let launched_b = launch(
            &session,
            &held_b,
            "real-b-launch",
            "real-b",
            OLD_VERSION,
            "node-system",
            REGISTRY,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &crate::release_e2e::journeys::progress_channel(),
        )
        .await
        .expect("real DSH 0.1.5-rc.3 must launch beside the newer one");
        let port_b = launched_b.outcome.port;
        let url_b = launched_b
            .outcome
            .web_url
            .clone()
            .expect("the old baseline prints its authenticated URL too");
        let auth_b = web_authenticated(&url_b).await;
        assert_eq!(auth_b.status, 200, "the old baseline must serve at {url_b}");
        assert_eq!(
            auth_b.without_cookie_status, 401,
            "the old baseline authenticates the same way"
        );
        assert_ne!(port_a, port_b, "two instances, two ports");
        assert_ne!(url_a, url_b);
        assert!(
            crate::release_e2e::port_listening(port_a).await
                && crate::release_e2e::port_listening(port_b).await,
            "both real DSH servers are listening at the same time"
        );
        println!(
            "B: {OLD_VERSION} on :{port_b} -> handshake {} -> 200 ({} bytes)",
            auth_b.handshake_status, auth_b.body_bytes
        );

        // 6. Each instance wrote its own launch log under its own HOME — the
        //    on-disk half of "isolated environments".
        for (id, port) in [("real-a", port_a), ("real-b", port_b)] {
            // PHL's own launch log lives at <instance>/logs/launch-*.log
            // (truncated per boot); it captures the child's output, which for
            // a real  includes the URL naming this boot's port.
            let logs = session.root().join("instances").join(id).join("logs");
            let mut found = false;
            if let Ok(entries) = std::fs::read_dir(&logs) {
                for entry in entries.flatten() {
                    let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
                    if body.contains(&format!(":{port}")) {
                        found = true;
                    }
                }
            }
            assert!(found, "{id}: its own HOME holds the launch log naming its own port :{port}");
        }

        // 7. Stop both through the real stop path; both ports free.
        session.terminate("real-a").await;
        session.terminate("real-b").await;
        assert!(
            crate::release_e2e::wait_until(10_000, || crate::launch::port_free(port_a)).await,
            "port A frees after stop"
        );
        assert!(
            crate::release_e2e::wait_until(10_000, || crate::launch::port_free(port_b)).await,
            "port B frees after stop"
        );
        println!("STOP: both real DSH instances stopped, both ports free");
    })
    .await;
}

/// Diagnostic companion: install ONE real version, launch it, and print
/// everything observable about the WebUI auth exchange (status, headers, body
/// head, and the launch log's own words). This is how the 401 observed on
/// 2026-09-26 was investigated — it stays because the fake-DSH lanes cannot
/// show this shape at all.
#[tokio::test]
#[ignore = "real-upstream diagnostic: needs network + npm; opt-in via PHL_REAL_DSH=1"]
async fn real_dsh_webui_auth_probe() {
    if !opted_in() {
        println!("SKIP real_dsh_webui_auth_probe: set PHL_REAL_DSH=1");
        return;
    }
    let node = NodeInfo::discover().expect("node on PATH");
    let session = Session::fresh("real-dsh-auth");
    with_cleanup(&session, async {
        let catalog = crate::versions::list_dsh_versions(REGISTRY.to_string())
            .await
            .expect("catalog");
        let entry = catalog
            .iter()
            .find(|v| v.name == TARGET_VERSION)
            .expect("target version published");
        let src = entry.source.as_ref().unwrap();
        install_real(
            &session,
            &node.exe,
            TARGET_VERSION,
            &src.tarball,
            src.integrity.as_deref(),
        )
        .await;
        create_real_instance(&session, "probe-a", &format!("dsh-{TARGET_VERSION}")).await;

        let held = crate::release_e2e::driver::lock_instance(&session.locks, "probe-a");
        let launched = launch(
            &session,
            &held,
            "probe-launch",
            "probe-a",
            TARGET_VERSION,
            "node-system",
            REGISTRY,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &crate::release_e2e::journeys::progress_channel(),
        )
        .await
        .expect("launch");
        let url = launched.outcome.web_url.clone().expect("url");
        println!("PROBE web_url: {url}");

        // Raw exchange: status, headers, body head — and the same request
        // with the token as a header and as a Cookie, to see which the real
        // server accepts.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let res = client.get(&url).send().await.unwrap();
        println!("PROBE status(no-redirect): {}", res.status());
        for (k, v) in res.headers() {
            println!("PROBE header: {k}: {}", v.to_str().unwrap_or("<bin>"));
        }
        let body = res.text().await.unwrap_or_default();
        println!(
            "PROBE body head: {}",
            body.chars().take(400).collect::<String>()
        );

        let token = url.split("token=").nth(1).unwrap_or("").to_string();
        let base = url.split("?token=").next().unwrap_or(&url).to_string();
        let with_cookie = client
            .get(&base)
            .header("Cookie", format!("token={token}"))
            .send()
            .await
            .unwrap();
        println!("PROBE status(cookie): {}", with_cookie.status());
        let with_auth = client
            .get(&base)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        println!("PROBE status(bearer): {}", with_auth.status());

        // The DSH's own words about this boot.
        let logs_dir = session
            .root()
            .join("instances")
            .join("probe-a")
            .join("logs");
        if let Some(log) = crate::launch::latest_launch_log(&logs_dir).await {
            if let Ok(text) = std::fs::read_to_string(&log) {
                println!(
                    "PROBE launch log tail:\n{}",
                    text.chars()
                        .rev()
                        .take(1200)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                );
            }
        }
        session.terminate("probe-a").await;
    })
    .await;
}
