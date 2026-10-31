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
//! Issue #7 adds the 0.2.x leg: `real_dsh_02x_compatibility_matrix` pins one
//! npm-confirmed 0.2.x release and walks the full per-platform step list
//! (upstream catalog → install → 目录识别 → instance → launch → token URL
//! 握手 → sessions read → plugin list & enable/disable → trial preview →
//! restart-adoption core → stop/port free). The repeatable entry points are
//! `npm run test:real-dsh` and the manual-only workflow
//! `.github/workflows/real-dsh-matrix.yml`; neither is a CI gate (flaky
//! upstream networks must not block every push — issue #7's own caveat).
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

/// Issue #7's 0.2.x matrix pin. Confirmed 2026-10-31 on
/// `https://registry.npmjs.org/@deepseek-ai/dsh`: `0.2.1-rc.5` is published
/// AND carries the `latest` dist-tag — npm-installable, not GitHub-only.
/// Bump it deliberately (with a fresh npm check), never to a floating tag.
pub(super) const MATRIX_VERSION: &str = "0.2.1-rc.5";

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

/// Issue #7's 0.2.x matrix leg: ONE npm-confirmed 0.2.x release through the
/// whole PHL pipeline, leg by leg, each printed with a stable `[matrix]`
/// marker so a real-machine run translates 1:1 into the acceptance matrix
/// (`internal/acceptance/…issue-7…/MATRIX.md`). Failure messages carry the
/// issue's failure classification inline (网络/镜像故障 vs 上游包不可用 vs
/// PHL 兼容性失败), because a red CI row must say WHICH kind of red it is.
///
/// Boundaries, stated: this leg covers the PHL machinery against real 0.2.x
/// layout — plugin enable/disable runs through the real cordis path on a
/// fixture package (no third-party download in the default matrix), the
/// trial leg is the read-only preview, and the adoption leg is the real
/// probe+decide core (`adopt_processes` itself needs a Tauri AppHandle).
#[tokio::test]
#[ignore = "real-upstream acceptance: needs network + npm; opt-in via PHL_REAL_DSH=1"]
async fn real_dsh_02x_compatibility_matrix() {
    if !opted_in() {
        println!(
            "SKIP real_dsh_02x_compatibility_matrix: set PHL_REAL_DSH=1 to run \
             (downloads {MATRIX_VERSION} from {REGISTRY} and starts a real DSH 0.2.x server)"
        );
        return;
    }
    let Some(node) = NodeInfo::discover() else {
        panic!(
            "real DSH matrix: no `node` on PATH — the lane drives real DSH with the host Node \
             (分类: 环境不满足, not a PHL failure)"
        );
    };
    println!(
        "real DSH matrix: host node {} ({})",
        node.version,
        node.exe.display()
    );

    let session = Session::fresh("real-dsh-matrix");
    with_cleanup(&session, async {
        // ---- leg 1/9: 上游目录（区分网络故障与上游包不可用）--------------
        let catalog = match crate::versions::list_dsh_versions(REGISTRY.to_string()).await {
            Ok(catalog) => catalog,
            Err(e) => panic!(
                "[matrix leg 1/9 上游目录] FAILED（分类: 网络/镜像故障 — catalog 无法访问）: {e}"
            ),
        };
        let entry = catalog
            .iter()
            .find(|v| v.name == MATRIX_VERSION)
            .unwrap_or_else(|| {
                panic!(
                    "[matrix leg 1/9 上游目录] FAILED（分类: 上游包不可用 — {MATRIX_VERSION} \
                     不在已发布目录中；若它应已发布，属上游/镜像问题）"
                )
            });
        let src = entry.source.as_ref().unwrap_or_else(|| {
            panic!("[matrix leg 1/9 上游目录] FAILED（分类: 上游包不可用 — 无 tarball）")
        });
        println!(
            "[matrix leg 1/9] OK: {MATRIX_VERSION} in catalog -> {} (integrity {})",
            src.tarball,
            src.integrity.is_some()
        );

        // ---- leg 2/9: 真实安装 + 目录识别（marker / 入口 / 依赖树）-------
        install_real(
            &session,
            &node.exe,
            MATRIX_VERSION,
            &src.tarball,
            src.integrity.as_deref(),
        )
        .await;
        let version_dir = session.root().join("versions").join(MATRIX_VERSION);
        assert!(
            version_dir.join("phl-install.json").exists(),
            "[matrix leg 2/9 版本安装/目录识别] FAILED: phl-install.json 不存在（分类: PHL 兼容性失败）"
        );
        assert!(
            crate::versions::read_marker(&version_dir).await.is_some(),
            "[matrix leg 2/9 版本安装/目录识别] FAILED: marker 无法解析（分类: PHL 兼容性失败）"
        );
        assert!(
            version_dir.join("lib").join("bin.js").exists(),
            "[matrix leg 2/9 版本安装/目录识别] FAILED: lib/bin.js 入口缺失（分类: PHL 兼容性失败 — \
             0.2.x 的包布局可能已变化）"
        );
        println!("[matrix leg 2/9] OK: install + 目录识别 (marker/entry/deps)");

        // ---- leg 3/9: 实例创建 ------------------------------------------
        create_real_instance(&session, "matrix-a", &format!("dsh-{MATRIX_VERSION}")).await;
        let instance_dir = session.root().join("instances").join("matrix-a");
        let manifest =
            crate::instances::load_manifest(&instance_dir, "matrix-a").await.expect(
                "[matrix leg 3/9 实例创建] FAILED: manifest 无法读回（分类: PHL 兼容性失败）",
            );
        let home = crate::instances::home_of(&instance_dir, &manifest);
        let profile = crate::instances::profile_dir(&session.root(), "matrix-a")
            .await
            .expect("[matrix leg 3/9 实例创建] FAILED: profile 目录无法解析");
        println!("[matrix leg 3/9] OK: instance matrix-a @ {}", home.display());

        // ---- leg 4/9: 启动 + token URL 握手 -----------------------------
        let held = crate::release_e2e::driver::lock_instance(&session.locks, "matrix-a");
        let launched = launch(
            &session,
            &held,
            "matrix-launch",
            "matrix-a",
            MATRIX_VERSION,
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
        .unwrap_or_else(|e| {
            panic!(
                "[matrix leg 4/9 启动/握手] FAILED: {e}（分类: 需读日志区分 — PHL 兼容性失败或 \
                 DSH 本体缺陷；latest_launch_log 会保留 DSH 的输出）"
            )
        });
        let port = launched.outcome.port;
        let pid = launched.outcome.pid;
        let url = launched
            .outcome
            .web_url
            .clone()
            .expect("[matrix leg 4/9 启动/握手] FAILED: 未取得 token URL（分类: PHL 兼容性失败 — \
             0.2.x 的 dsh web 输出格式可能变化）");
        let auth = web_authenticated(&url).await;
        assert_eq!(
            auth.handshake_status, 303,
            "[matrix leg 4/9 启动/握手] FAILED（分类: DSH 本体缺陷或 0.2.x 认证行为变化）"
        );
        assert!(
            auth.cookie_name.starts_with("dsh-auth-"),
            "[matrix leg 4/9 启动/握手] FAILED（分类: DSH 本体缺陷或 0.2.x 认证行为变化）"
        );
        assert_eq!(
            auth.status, 200,
            "[matrix leg 4/9 启动/握手] FAILED（分类: DSH 本体缺陷或 0.2.x 认证行为变化）"
        );
        assert_eq!(
            auth.without_cookie_status, 401,
            "[matrix leg 4/9 启动/握手] FAILED（分类: DSH 本体缺陷或 0.2.x 认证行为变化）"
        );
        println!(
            "[matrix leg 4/9] OK: {MATRIX_VERSION} on :{port} -> handshake {} -> 200 ({} bytes), \
             no-cookie {}",
            auth.handshake_status, auth.body_bytes, auth.without_cookie_status
        );

        // ---- leg 5/9: 会话只读检查 --------------------------------------
        let listing = tokio::task::spawn_blocking({
            let home = home.clone();
            move || crate::sessions::list_in_home(&home)
        })
        .await
        .expect("[matrix leg 5/9 会话只读] FAILED: blocking 任务失败");
        println!(
            "[matrix leg 5/9] OK: sessions read — {} readable / {} unreadable (fresh home)",
            listing.sessions.len(),
            listing.unreadable.len()
        );

        // ---- leg 6/9: 插件列表 + 真实 cordis 启停（合成注册，无第三方下载）
        let plugins = crate::instances::scan_plugins(&profile).await;
        assert!(
            plugins.iter().all(|p| !p.manageable),
            "[matrix leg 6/9 插件列表] FAILED: 全新 0.2.x profile 出现受管插件（分类: PHL 兼容性失败）: {plugins:?}"
        );
        let fixture = profile.join("node_modules").join("dsh-matrix-fixture");
        tokio::fs::create_dir_all(&fixture).await.unwrap();
        tokio::fs::write(
            fixture.join("package.json"),
            br#"{"name":"dsh-matrix-fixture","version":"0.0.1"}"#,
        )
        .await
        .unwrap();
        let task = crate::resources::Tasks::default()
            .begin(
                crate::resources::TaskInfo::new(
                    "matrix-plugin".into(),
                    "plugin-install",
                    "matrix fixture".into(),
                    &[],
                ),
                None,
            )
            .unwrap();
        let marker = serde_json::json!({
            "installedAt": crate::versions::now_iso(),
            "pluginId": "matrix/dsh-matrix-fixture",
            "version": "0.0.1",
            "kind": "npm",
            "registryId": "dsh-matrix-fixture",
            "trust": "unverified",
        });
        crate::plugins::install::commit_install(
            &fixture,
            &marker,
            &profile,
            "dsh-matrix-fixture",
            &task,
            None,
            &crate::plugins::deps::DependencyEnv::new(std::path::Path::new(""), "node-system", None),
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "[matrix leg 6/9 插件启停] FAILED: 合成插件注册失败（分类: PHL 兼容性失败 — \
                 0.2.x 的 cordis.patch.yml 格式可能变化）: {e}"
            )
        });
        let after = crate::instances::scan_plugins(&profile).await;
        assert!(
            after
                .iter()
                .any(|p| p.registry_id == "dsh-matrix-fixture" && p.manageable && p.enabled),
            "[matrix leg 6/9 插件列表] FAILED: 注册后的插件未出现在扫描结果中: {after:?}"
        );
        crate::plugins::cordis::set_plugin_disabled(&profile, "dsh-matrix-fixture", true)
            .await
            .unwrap_or_else(|e| panic!("[matrix leg 6/9 插件启停] FAILED（停用）: {e}"));
        assert!(
            crate::plugins::disabled_plugin_ids(&profile)
                .await
                .contains("dsh-matrix-fixture"),
            "[matrix leg 6/9 插件启停] FAILED: 停用未生效"
        );
        crate::plugins::cordis::set_plugin_disabled(&profile, "dsh-matrix-fixture", false)
            .await
            .unwrap_or_else(|e| panic!("[matrix leg 6/9 插件启停] FAILED（重新启用）: {e}"));
        assert!(
            !crate::plugins::disabled_plugin_ids(&profile)
                .await
                .contains("dsh-matrix-fixture"),
            "[matrix leg 6/9 插件启停] FAILED: 重新启用未生效"
        );
        // #10 wiring rides along: the package's own engines.dsh claim evaluated
        // against the real 0.2.x binding (undeclared → unknown, honestly).
        let compat_range = crate::plugins::engines::package_engine_range(&fixture).await;
        let (verdict, reason) = crate::plugins::engines::evaluate(
            compat_range.as_deref(),
            crate::plugins::engines::bare_dsh_version(&format!("dsh-{MATRIX_VERSION}")).as_ref(),
        );
        println!(
            "[matrix leg 6/9] OK: scan {}→{} entries, disable/enable round-trip, engines.dsh={} ({reason})",
            plugins.len(),
            after.len(),
            verdict.as_str()
        );

        // ---- leg 7/9: 试用预览（只读；创建复用既有拷贝管线，不在矩阵联网）-
        let req = crate::instances::trial::TrialRequest {
            source_id: "matrix-a".into(),
            target_version: crate::versions::install::bound_id_for_version("0.1.5-rc.3")
                .unwrap_or_else(|| "0.1.5-rc.3".into())
                .trim_start_matches("dsh-")
                .to_string(),
            target_runtime_id: "node-system".into(),
            name: "matrix trial preview".into(),
            scope: "config".into(),
            workspace: "fresh".into(),
            plan_id: String::new(),
            target_id: String::new(),
            source_fingerprint: String::new(),
        };
        let preview = crate::instances::trial::build_preview(
            &session.root(),
            &crate::launch::Processes::default(),
            &req,
        )
        .await
        .unwrap_or_else(|e| {
            panic!("[matrix leg 7/9 试用预览] FAILED（分类: PHL 兼容性失败）: {e}")
        });
        println!(
            "[matrix leg 7/9] OK: trial preview — target {} installed={}, pending {:?}, \
             plan {} (blocked: {:?})",
            preview.target_version,
            preview.target_installed,
            preview.pending_downloads,
            preview.plan_id,
            preview.blocked
        );

        // ---- leg 8/9: 重启接管核心（真实 probe + decide；命令壳需要 AppHandle）-
        let probe = crate::launch::probe_process(pid);
        assert!(
            probe.state == crate::launch::ProcessState::Alive,
            "[matrix leg 8/9 重启接管] FAILED: probe 报告 {:?}（分类: 环境/PHL 兼容性失败）",
            probe.state
        );
        let exe = node
            .exe
            .canonicalize()
            .unwrap_or_else(|e| panic!("[matrix leg 8/9 重启接管] FAILED: node.exe 无法规范化: {e}"));
        let record = crate::launch::registry::PersistedProcess {
            instance_id: "matrix-a".into(),
            pid,
            port,
            started_at_ms: crate::versions::now_millis() as i64,
            exe_path: exe.to_string_lossy().into_owned(),
            process_start_token: None,
        };
        let adoption = crate::launch::registry::decide(&record, &probe);
        assert!(
            matches!(adoption, crate::launch::registry::Adoption::Adopt),
            "[matrix leg 8/9 重启接管] FAILED: decide 拒绝 ({adoption:?}，分类: PHL 兼容性失败 — \
             probe exe 与记录不一致或创建时间窗口不匹配)"
        );
        println!(
            "[matrix leg 8/9] OK: probe(pid={pid}) Alive + exe 身份一致 → Adoption::Adopt"
        );

        // ---- leg 9/9: 停止 + 端口释放 -----------------------------------
        session.terminate("matrix-a").await;
        assert!(
            crate::release_e2e::wait_until(10_000, || crate::launch::port_free(port)).await,
            "[matrix leg 9/9 端口释放] FAILED: :{port} 停止后未释放（分类: PHL 兼容性失败）"
        );
        println!("[matrix leg 9/9] OK: stopped, :{port} free");
        println!(
            "[matrix] ALL LEGS PASSED on {MATRIX_VERSION} — record into \
             internal/acceptance/2026-10-31-issue-7-dsh-matrix/MATRIX.md with node {} / {}",
            node.version,
            std::env::consts::OS
        );
    })
    .await;
}
