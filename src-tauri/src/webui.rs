//! Embedded DSH WebUI windows: one OS window per running instance, loaded
//! straight at the authenticated `dsh web` URL.
//!
//! The lifecycle contract is the one agreed with the launcher's UX model:
//! **closing a WebUI window does not stop the instance** — the dock still
//! reports it running and re-opening focuses the same window — while **the
//! process exiting (stop, crash, or clean shutdown) always takes its window
//! with it**, which the launch watcher enforces via [`close_for_instance`].
//!
//! Windows are labelled `dsh-web-<instance-id>`, so the id→window mapping is
//! total: no extra registry, and a stale label can never point at the wrong
//! instance. The label pattern is mirrored in `capabilities/default.json`;
//! the capability exists so the main window may *close* those windows, and
//! carries no `remote` origin — these pages are local DSH apps over HTTP,
//! never PHL render targets, so they have no IPC surface by construction.
//!
//! The URL arrives from the frontend (it already displays it via the launch
//! outcome and the fallback), so it is validated here as loopback-only,
//! http(s), and free of userinfo: a confused frontend can aim the window at
//! another local port, but not at an off-machine page dressed up as DSH.
//!
//! A frontend address that carries no token is not a usable WebUI address at
//! all — `dsh web` answers every token-less request with its 401 page ("dsh
//! web authentication required"), so opening it strands the user on a page
//! that can never load. Before navigating, `authenticated_url` therefore looks
//! for this instance's own `dsh web:` line in its newest launch log and
//! prefers it; that keeps 「打开 WebUI」 working when the frontend's copy of the
//! URL is missing or stale (an older record, a launch that raced the print, a
//! PHL restart) instead of trading the fix for a mystery page.

use std::path::Path;

use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::paths::{sanitize_segment, PhlState};

/// Window-label prefix. Every `dsh-web-*` window belongs to this module.
pub(crate) const WEBUI_PREFIX: &str = "dsh-web-";

fn label_of(instance_id: &str) -> String {
    format!("{WEBUI_PREFIX}{instance_id}")
}

/* ------------------------------ url gate ------------------------------- */

/// Accept only http(s) loopback URLs without credentials.
///
/// `Url` parses `http://localhost@evil/` with host `evil` (the userinfo is
/// everything before the last `@`), so the host check below is the real gate;
/// the explicit userinfo rejection is belt-and-braces for anything that ever
/// renders the authority naively.
fn ensure_loopback(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|e| format!("无效的 WebUI 地址: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("WebUI 地址必须是 http(s): {raw}"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("WebUI 地址不允许携带凭据".into());
    }
    let ok = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) => false,
        None => false,
    };
    if ok {
        Ok(url)
    } else {
        Err("WebUI 窗口只允许打开本机地址".into())
    }
}

/// Whether an existing window still points at the requested address. A window
/// outlives the process it was opened for, so "the window exists" is never the
/// same question as "the window is current".
fn needs_renavigation(current: Option<&Url>, requested: &Url) -> bool {
    current != Some(requested)
}

/* ------------------------- authenticated address ------------------------ */

/// Does this address still need DSH's per-boot token?
fn needs_token(url: &Url) -> bool {
    !url.query_pairs().any(|(key, _)| key == "token")
}

/// The address the window should really be opened at: the requested one when it
/// already carries a token, otherwise this instance's own authenticated URL
/// read back from its newest launch log.
///
/// `dsh web` prints that URL as the first line of every launch's log, and the
/// log is the only place the token lives on this machine. Reading it back is
/// what lets the window heal itself after the frontend's URL was lost or
/// stale — the alternative is the bare `host:port`, which DSH answers with a
/// 401 and no explanation.
///
/// The recovered URL must name the **same port** as the request: the port comes
/// from the instance's live record, so a log left over from an earlier boot on
/// another port can never aim the window at a socket that is not this
/// instance's. Anything else about the URL is re-validated through
/// [`ensure_loopback`], so a hand-edited log cannot smuggle in a remote host.
async fn authenticated_url(phl: &PhlState, instance_id: &str, requested: Url) -> Url {
    if !needs_token(&requested) {
        return requested;
    }
    let logs = phl.root().join("instances").join(instance_id).join("logs");
    match recovered_url_from_log(&logs, &requested).await {
        Some(url) => url,
        None => requested,
    }
}

/// The newest launch log's `dsh web:` line as a loopback URL on the requested
/// port, or `None` when there is nothing usable to read.
async fn recovered_url_from_log(logs_dir: &Path, requested: &Url) -> Option<Url> {
    let log = crate::launch::latest_launch_log(logs_dir).await?;
    let raw = crate::launch::read_web_url_once(&log).await?;
    let url = Url::parse(raw.trim()).ok()?;
    if url.port() != requested.port() {
        return None;
    }
    ensure_loopback(url.as_str()).ok()
}

/* ------------------------------ commands ------------------------------- */

/// Open (or focus) the embedded WebUI window for one instance. Idempotent by
/// label, so a double-click race opens exactly one window.
///
/// MUST be an `async` command: on Windows a *sync* command that builds a
/// webview deadlocks against the event loop (the window appears but the
/// webview never finishes initializing → blank page), as documented in
/// tauri-apps/tauri#3597. Async moves execution off the main thread so the
/// webview-creation callback can complete.
#[tauri::command]
pub async fn open_or_focus_webui(
    app: AppHandle,
    phl: State<'_, PhlState>,
    instance_id: String,
    url: String,
    title: Option<String>,
) -> Result<(), String> {
    let id = sanitize_segment(&instance_id, "实例 id")?;
    let parsed = authenticated_url(&phl, &id, ensure_loopback(&url)?).await;
    let label = label_of(&id);

    if let Some(window) = app.get_webview_window(&label) {
        // The label identifies the instance, not the launch. A restart reuses
        // the window for a new port and a new token, so focusing it without
        // correcting the address would keep showing the previous session's
        // URL — and its credentials (2026-09-09 review, P2).
        if needs_renavigation(window.url().ok().as_ref(), &parsed) {
            window
                .navigate(parsed.clone())
                .map_err(|e| format!("无法更新 WebUI 窗口地址: {e}"))?;
        }
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let host = parsed.host_str().unwrap_or("localhost");
    let fallback = format!("DSH · {host}");
    let name = title.unwrap_or_default();
    let name: String = name.chars().take(60).collect();
    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title(if name.trim().is_empty() {
            &fallback
        } else {
            &name
        })
        .inner_size(1180.0, 800.0)
        .min_inner_size(480.0, 360.0)
        .center()
        .resizable(true)
        .build()
        .map_err(|e| format!("无法打开 WebUI 窗口: {e}"))?;
    // Same taskbar-icon treatment as the main window (see lib.rs).
    crate::apply_taskbar_icon(&window);
    let _ = window.set_focus();
    Ok(())
}

/* ------------------------------ lifecycle ------------------------------ */

/// Process-exit hook (stop / crash / clean shutdown, from the launch
/// watcher): an instance that is no longer running must not keep a window
/// where its UI pretends to be live.
pub(crate) fn close_for_instance<R: tauri::Runtime>(app: &AppHandle<R>, instance_id: &str) {
    if let Some(window) = app.get_webview_window(&label_of(instance_id)) {
        let _ = window.close();
    }
}

/* -------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_pass_and_off_machine_ones_are_refused() {
        assert!(ensure_loopback("http://127.0.0.1:3080/?token=abc").is_ok());
        assert!(ensure_loopback("http://localhost:3080/").is_ok());
        assert!(ensure_loopback("http://127.0.0.42:8080/x").is_ok());
        assert!(ensure_loopback("http://[::1]:3080/").is_ok());

        assert!(ensure_loopback("http://192.168.1.7:3080/").is_err());
        assert!(ensure_loopback("http://dsh.example.com/").is_err());
        assert!(ensure_loopback("http://127.example.com/").is_err());
        assert!(ensure_loopback("http://127.attacker.io/").is_err());
        assert!(ensure_loopback("file:///C:/etc/passwd").is_err());
        assert!(ensure_loopback("javascript:alert(1)").is_err());
        assert!(ensure_loopback("not a url").is_err());
    }

    #[test]
    fn userinfo_spoofing_cannot_disguise_a_remote_host() {
        // Parsed: username `localhost`, host `evil.example`. The host gate
        // must catch this even though the authority *starts* with a loopback
        // name — and the userinfo rule refuses it before the host is read.
        assert!(ensure_loopback("http://localhost@evil.example/").is_err());
        assert!(ensure_loopback("http://user:pw@127.0.0.1:3080/").is_err());
    }

    #[test]
    fn a_restarted_instance_reuses_the_window_but_not_the_url() {
        let old = ensure_loopback("http://127.0.0.1:3080/?token=old").unwrap();
        let new = ensure_loopback("http://127.0.0.1:3099/?token=new").unwrap();
        // Same address: focusing is enough.
        assert!(!needs_renavigation(Some(&old), &old));
        // New port and token after a restart: the window must be re-navigated.
        assert!(needs_renavigation(Some(&old), &new));
        // The window's address could not be read: treat it as stale.
        assert!(needs_renavigation(None, &new));
    }

    #[test]
    fn labels_round_trip_on_the_prefix() {
        let label = label_of("11-9z3d");
        assert_eq!(label, "dsh-web-11-9z3d");
        assert_eq!(label.strip_prefix(WEBUI_PREFIX), Some("11-9z3d"));
    }

    /* --------------------- authenticated address --------------------- */

    /// A root with one instance's `logs/` holding `line` as its newest launch
    /// log — the shape the launcher leaves behind.
    fn root_with_launch_log(tag: &str, instance_id: &str, name: &str, line: &str) -> PhlState {
        let root = std::env::temp_dir().join(format!("phl-webui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let logs = root.join("instances").join(instance_id).join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join(name), line).unwrap();
        let state = PhlState::with_pointer(Some(root.join("cfg").join("root.json")));
        state.set_root(&root.to_string_lossy()).unwrap();
        state
    }

    #[test]
    fn a_tokenless_address_is_the_one_that_needs_healing() {
        let bare = Url::parse("http://localhost:3081/").unwrap();
        let tokenized = Url::parse("http://127.0.0.1:3081/?token=abc").unwrap();
        assert!(needs_token(&bare));
        assert!(!needs_token(&tokenized));
    }

    #[tokio::test]
    async fn a_tokenless_request_is_replaced_by_this_instance_s_own_token() {
        // The reported bug (2026-09-15): the frontend's URL was lost, 「打开
        // WebUI」 fell back to `http://localhost:3081/`, and DSH answered 401
        // in a window PHL reported as running. The token is on disk in the
        // instance's own launch log, so the window is opened at it instead.
        let phl = root_with_launch_log(
            "recover",
            "dsh-p5ig",
            "launch-2026-09-15T04-56-18Z.log",
            "dsh web: http://127.0.0.1:3081/?token=tOpz9F4sEu8M\n",
        );
        let requested = ensure_loopback("http://localhost:3081/").unwrap();
        let healed = authenticated_url(&phl, "dsh-p5ig", requested).await;
        assert_eq!(healed.as_str(), "http://127.0.0.1:3081/?token=tOpz9F4sEu8M");
    }

    #[tokio::test]
    async fn a_request_that_already_carries_a_token_is_left_alone() {
        // The frontend's own (fresh) URL is authoritative — the log is only a
        // repair path, never a veto on a live token.
        let phl = root_with_launch_log(
            "keep",
            "dsh-p5ig",
            "launch-2026-09-15T04-56-18Z.log",
            "dsh web: http://127.0.0.1:3081/?token=older\n",
        );
        let requested = ensure_loopback("http://127.0.0.1:3081/?token=current").unwrap();
        let kept = authenticated_url(&phl, "dsh-p5ig", requested).await;
        assert_eq!(kept.as_str(), "http://127.0.0.1:3081/?token=current");
    }

    #[tokio::test]
    async fn only_the_requested_port_is_healed_from_a_log() {
        // A log left over from an earlier boot on another port must not aim the
        // window at a socket that is not this instance's.
        let phl = root_with_launch_log(
            "port",
            "dsh-p5ig",
            "launch-2026-09-12T07-54-55Z.log",
            "dsh web: http://127.0.0.1:3080/?token=from-another-boot\n",
        );
        let requested = ensure_loopback("http://localhost:3081/").unwrap();
        let unchanged = authenticated_url(&phl, "dsh-p5ig", requested).await;
        assert_eq!(unchanged.as_str(), "http://localhost:3081/");
    }

    #[tokio::test]
    async fn nothing_to_read_leaves_the_request_as_it_was() {
        // No logs directory at all: the caller still gets a validated address,
        // never an error — a DSH that serves without a token is legitimate.
        let root = std::env::temp_dir().join(format!("phl-webui-nolog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let phl = PhlState::with_pointer(Some(root.join("cfg").join("root.json")));
        phl.set_root(&root.to_string_lossy()).unwrap();
        let requested = ensure_loopback("http://localhost:3081/").unwrap();
        let unchanged = authenticated_url(&phl, "dsh-p5ig", requested).await;
        assert_eq!(unchanged.as_str(), "http://localhost:3081/");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_log_pointing_off_machine_is_refused() {
        // The log is a file on disk, so it gets the same loopback gate as the
        // frontend's address.
        let phl = root_with_launch_log(
            "remote",
            "dsh-p5ig",
            "launch-2026-09-15T04-56-18Z.log",
            "dsh web: http://dsh.example.com:3081/?token=evil\n",
        );
        let requested = ensure_loopback("http://localhost:3081/").unwrap();
        let unchanged = authenticated_url(&phl, "dsh-p5ig", requested).await;
        assert_eq!(unchanged.as_str(), "http://localhost:3081/");
    }
}
