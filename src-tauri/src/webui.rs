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

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::paths::sanitize_segment;

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
    instance_id: String,
    url: String,
    title: Option<String>,
) -> Result<(), String> {
    let id = sanitize_segment(&instance_id, "实例 id")?;
    let parsed = ensure_loopback(&url)?;
    let label = label_of(&id);

    if let Some(window) = app.get_webview_window(&label) {
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
    fn labels_round_trip_on_the_prefix() {
        let label = label_of("11-9z3d");
        assert_eq!(label, "dsh-web-11-9z3d");
        assert_eq!(label.strip_prefix(WEBUI_PREFIX), Some("11-9z3d"));
    }
}
