//! The Windows E2E release gate (headless lane).
//!
//! * `fixtures` — scratch roots, the local mock HTTP server, the fake DSH
//!   tarball / Node dist builders, sha helpers.
//! * `driver` — `Session` (the plain-struct Tauri state, bound to a throwaway
//!   root) and the headless `launch`/`stop` wrappers that reproduce the
//!   `launch_instance`/`stop_instance` command shells without an `AppHandle`.
//! * `journeys` — the five acceptance journeys (J1–J5).
//! * `faults` — the fault-injection cases (F1–F4), system level.
//!
//! The module compiles only under `#[cfg(all(test, windows))]` (declared in
//! `lib.rs`), and its tests are `#[ignore]`-marked: the normal `cargo test`
//! sweep stays fast and deterministic, while the gate runs them explicitly —
//!
//! ```text
//! cargo test --workspace -- --ignored release_e2e --test-threads=1
//! ```
//!
//! which is exactly what the `windows-e2e` CI job executes. `--test-threads=1`
//! is required: journeys bind real sockets, spawn real children and probe
//! real pids, so they must not interleave on a shared machine.
//!
//! Scenario map (full 1:1 table in the maintainer-local gate docs):
//! J1 cold install → runtime → version → create → launch → stop ·
//! J2 restart adoption · J3 snapshot → rollback → pack export/install ·
//! J4 journaled storage migration (commit/resume/undo) · J5 updater manifest
//! contract against the real `updates` branch ·
//! F1 force-kill mid-run · F2 download abort mid-stream + resume ·
//! F3 locked destination (the CI-runnable disk-full sibling) ·
//! F4 old child exits *after* the new one launches.

pub(crate) mod driver;
pub(crate) mod faults;
pub(crate) mod fixtures;
pub(crate) mod journeys;

/// The fake DSH release the gate installs. A reserved pre-release so it can
/// never collide with a real published version.
pub(crate) const FAKE_DSH_VERSION: &str = "9.9.9-e2e";

/// The npm packument the mock registry serves: the single fake version, with
/// the tarball's real digest as `dist.integrity` (the catalog → download →
/// verify chain must exercise the genuine integrity check).
pub(crate) fn packument_json(base: &str, version: &str, integrity: &str) -> String {
    serde_json::json!({
        "name": "@deepseek-ai/dsh",
        "dist-tags": { "latest": version },
        "versions": {
            version: {
                "name": "@deepseek-ai/dsh",
                "version": version,
                "dist": {
                    "tarball": format!("{base}/dsh-{version}.tgz"),
                    "integrity": integrity,
                },
            }
        },
    })
    .to_string()
}

/// Same packument without `integrity` — verifies the pipeline still installs
/// when the registry omits a digest (`verify_integrity` skips on None).
pub(crate) fn packument_json_no_integrity(base: &str, version: &str) -> String {
    serde_json::json!({
        "name": "@deepseek-ai/dsh",
        "dist-tags": { "latest": version },
        "versions": {
            version: {
                "name": "@deepseek-ai/dsh",
                "version": version,
                "dist": {
                    "tarball": format!("{base}/dsh-{version}.tgz"),
                },
            }
        },
    })
    .to_string()
}

/// The TCP-connect liveness probe used by journeys and faults alike —
/// exactly PHL's own readiness signal (`launch/mod.rs:919`).
pub(crate) async fn port_listening(port: u16) -> bool {
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Poll `cond` every 100 ms up to `timeout_ms`. Every wait in this gate is
/// a bounded poll on an externally observable fact (socket open, file gone,
/// registry row cleared) — never a `sleep` and a hope.
pub(crate) async fn wait_until(timeout_ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if cond() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
