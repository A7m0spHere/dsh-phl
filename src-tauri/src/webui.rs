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
//!
//! # Page-failure probe (报错日志)
//!
//! A plugin that never activates — e.g. one injecting a Cordis service the
//! installed DSH version no longer provides — fails **in the browser**: the
//! web frontend's own boot code throws `web boot: N entr(y|ies) did not
//! activate` and renders it as the failure card. Nothing reaches `dsh web`'s
//! stderr, so the instance's launch log records only the `dsh web:` line and
//! PHL has no trace of what the user is staring at (2026-09-27, `dsh-bonk-pet`
//! on 0.1.7-rc.2 vs. its 0.1.6-era `settingsScope` inject).
//!
//! The probe keeps that failure observable without giving the page an IPC
//! surface: `initialization_script` injects a read-only observer that watches
//! the `[data-dsh-boot]` failure card, and reports once by navigating to
//! `phl-webui-error:report#<percent-encoded card text>`. `on_navigation`
//! intercepts that scheme on the Rust side, appends the text to
//! `<instance>/logs/webui-errors.log` (size-capped, next to the launch logs),
//! emits `phl://webui-page-error`, and returns false so the navigation never
//! leaves the window. The channel is one-way page→Rust text; the page can
//! only write into its own instance's log, which is strictly less than what
//! rendering a fake UI in that window already allows.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::paths::{sanitize_segment, PhlState};
use crate::versions::now_iso;

/// Window-label prefix. Every `dsh-web-*` window belongs to this module.
pub(crate) const WEBUI_PREFIX: &str = "dsh-web-";

/// Scheme the injected probe navigates to in order to hand its report over.
const PROBE_SCHEME: &str = "phl-webui-error";

/// Per-instance page-error log, living beside the launch logs it complements.
const PAGE_ERROR_LOG: &str = "webui-errors.log";

/// Event name the frontend may subscribe to when a WebUI page reports failure.
pub(crate) const WEBUI_PAGE_ERROR: &str = "phl://webui-page-error";

/// Char cap on one reported card: boot failures are a few hundred chars; the
/// bound only exists so a hostile page cannot stream megabytes per report.
const MAX_REPORT_CHARS: usize = 16_000;

/// Rotation ceiling for `webui-errors.log` — over this, the oldest half of
/// the file is dropped (at a line boundary) on the next append.
const MAX_LOG_BYTES: u64 = 256 * 1024;

/// The probe, injected into every `dsh-web-*` window's documents. It is
/// defensive by construction: any throw inside it is swallowed, because
/// breaking the user's DSH page to capture a diagnostic would be the worst
/// possible outcome. It reports at most once per distinct card text per
/// document, and never mutates the page.
const BOOT_PROBE_JS: &str = r##"
(function () {
  try {
    if (!/^https?:$/.test(location.protocol)) return;
    if (window.__phlWebuiProbe) return;
    window.__phlWebuiProbe = 1;
    var last = '';
    function cardText() {
      var card = document.querySelector('[data-dsh-boot]');
      if (!card) return null;
      var t = (card.innerText || card.textContent || '').trim();
      if (/did not activate|Failed to load plugins|startup failed/i.test(t)) return t;
      return null;
    }
    function check() {
      var t = cardText();
      if (!t || t === last) return;
      last = t;
      try {
        location.assign('phl-webui-error:report#' + encodeURIComponent(t.slice(0, 16000)));
      } catch (e) {}
    }
    function watch() {
      check();
      try {
        new MutationObserver(check).observe(document.documentElement,
          { childList: true, subtree: true, characterData: true });
      } catch (e) {}
      setInterval(check, 3000);
    }
    if (document.documentElement) watch();
    else document.addEventListener('DOMContentLoaded', watch);
  } catch (e) {}
})();
"##;

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

/* ------------------------------ probe ------------------------------- */

/// Minimal percent-decoder for what `encodeURIComponent` emits: `%XX` triples
/// over UTF-8 bytes; everything else passes through (`+` stays a `+`, because
/// the fragment never went through form encoding).
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Keep readable text only: newlines and tabs survive, every other control
/// character (ANSI, NUL, bidi overrides …) is dropped, then the report cap.
fn sanitize_report(raw: &str) -> String {
    raw.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .take(MAX_REPORT_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The report carried by a probe navigation, or `None` for a URL that is not
/// one (normal navigations keep going) or whose payload is empty after
/// sanitising.
fn probe_report(url: &Url) -> Option<String> {
    if url.scheme() != PROBE_SCHEME {
        return None;
    }
    let text = sanitize_report(&percent_decode(url.fragment().unwrap_or("")));
    (!text.is_empty()).then_some(text)
}

/// Append one report to the instance's page-error log, then keep the file
/// bounded. Log trouble never surfaces as an error to the user: the window
/// is already showing the failure — the log is post-mortem material.
fn write_page_error(logs_dir: &Path, instance_id: &str, report: &str) {
    let path = logs_dir.join(PAGE_ERROR_LOG);
    let entry = format!(
        "[{ts}] 实例「{instance_id}」的 WebUI 页面报告启动失败（页面原文）:\n{report}\n\n",
        ts = now_iso(),
    );
    if std::fs::create_dir_all(logs_dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        if file.write_all(entry.as_bytes()).is_ok() {
            rotate_page_log(&path);
        }
    }
}

/// Over the ceiling, keep only the newest half (cut at a line boundary).
fn rotate_page_log(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= MAX_LOG_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let keep_from = bytes.len() / 2;
    let start = bytes[keep_from..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| keep_from + p + 1)
        .unwrap_or(keep_from);
    let _ = std::fs::write(path, &bytes[start..]);
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

    // Probe plumbing (see the module docs): the closure owns everything it
    // needs because `on_navigation` demands `'static`. The log directory is
    // resolved at open time from the live root — a root switch while a window
    // stays open is rare and the old path is still where the launch logs are.
    let logs_dir: PathBuf = phl.root().join("instances").join(&id).join("logs");
    let probe_id = id.clone();
    let probe_app = app.clone();

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
        .initialization_script(BOOT_PROBE_JS)
        .on_navigation(move |url| match probe_report(url) {
            None => true,
            Some(report) => {
                write_page_error(&logs_dir, &probe_id, &report);
                let _ = probe_app.emit(
                    WEBUI_PAGE_ERROR,
                    serde_json::json!({
                        "instanceId": probe_id.clone(),
                        "logPath": logs_dir.join(PAGE_ERROR_LOG),
                        "detail": report,
                    }),
                );
                false
            }
        })
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

    /* ------------------------------ probe ------------------------------ */

    #[test]
    fn normal_navigations_keep_going_and_probe_navigations_are_intercepted() {
        assert_eq!(
            probe_report(&Url::parse("http://127.0.0.1:3080/").unwrap()),
            None
        );
        assert_eq!(
            probe_report(&Url::parse("https://example.com/x").unwrap()),
            None
        );
        let report =
            Url::parse("phl-webui-error:report#web%20boot%3A%201%20entry%20did%20not%20activate")
                .unwrap();
        assert_eq!(
            probe_report(&report).as_deref(),
            Some("web boot: 1 entry did not activate")
        );
    }

    #[test]
    fn a_real_bonk_pet_failure_survives_the_round_trip() {
        // The exact card the user saw on 2026-09-27, encoded the way
        // `encodeURIComponent` encodes it (newlines become %0A).
        let card = "HARNESS\nFailed to load plugins\nweb boot: 1 entry did not activate\ndsh-bonk-pet: pending (waiting for service: settingsScope)";
        let encoded = "HARNESS%0AFailed%20to%20load%20plugins%0Aweb%20boot%3A%201%20entry%20did%20not%20activate%0Adsh-bonk-pet%3A%20pending%20(waiting%20for%20service%3A%20settingsScope)";
        let url = Url::parse(&format!("phl-webui-error:report#{encoded}")).unwrap();
        assert_eq!(probe_report(&url).as_deref(), Some(card));
    }

    #[test]
    fn empty_or_control_only_payloads_are_refused() {
        let empty = Url::parse("phl-webui-error:report#").unwrap();
        assert_eq!(probe_report(&empty), None);
        let nul = Url::parse("phl-webui-error:report#%00%01%02").unwrap();
        assert_eq!(probe_report(&nul), None);
    }

    #[test]
    fn percent_decoding_passes_through_what_the_encoder_leaves_alone() {
        assert_eq!(percent_decode("a%20b!c'd(e)h_i~j"), "a b!c'd(e)h_i~j");
        // A truncated escape stays literal rather than eating bytes.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%e2%9c%93"), "✓");
        // Invalid UTF-8 after decoding is lossy, not a panic.
        assert!(percent_decode("%ff").contains('\u{fffd}'));
    }

    #[test]
    fn reports_are_capped_before_hitting_disk() {
        let huge = "x".repeat(MAX_REPORT_CHARS * 3);
        let kept = sanitize_report(&huge);
        assert_eq!(kept.chars().count(), MAX_REPORT_CHARS);
    }

    #[test]
    fn tabs_and_newlines_survive_sanitising_but_ansi_does_not() {
        assert_eq!(sanitize_report("a\u{1b}[31mb\nc\td"), "a[31mb\nc\td");
    }

    /// A temp `logs/` dir unique to the running test process.
    fn temp_logs_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-probe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn one_instance_writes_one_bounded_log_beside_its_launch_logs() {
        let logs = temp_logs_dir("append");
        write_page_error(&logs, "3-xfji", "first failure");
        write_page_error(&logs, "3-xfji", "second failure");
        let text = std::fs::read_to_string(logs.join(PAGE_ERROR_LOG)).unwrap();
        assert!(text.contains("first failure") && text.contains("second failure"));
        assert!(text.contains("实例「3-xfji」"));
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn an_oversized_page_log_keeps_its_newest_half_at_line_boundaries() {
        let logs = temp_logs_dir("rotate");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join(PAGE_ERROR_LOG);
        // Pre-fill past the ceiling with line-numbered noise, then append one
        // real report: the file must come back under the cap, still contain
        // the new entry, and start at a whole line.
        let mut filler = String::new();
        while (filler.len() as u64) < MAX_LOG_BYTES + 1024 {
            filler.push_str("old noise line\n");
        }
        std::fs::write(&path, filler).unwrap();
        write_page_error(&logs, "i", "the newest report");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!((after.len() as u64) <= MAX_LOG_BYTES, "log stayed too big");
        assert!(after.contains("the newest report"), "new entry was lost");
        assert!(after.starts_with(['[', 'o']), "cut landed mid-line");
        assert!(
            !after.contains("old noise line\nold noise line\nol\n"),
            "garbage head"
        );
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn the_probe_finds_the_failure_card_by_its_stable_marker() {
        // The selector, not the copy, is the contract with dsh-web-frontend;
        // a typo here silently unships the whole log.
        assert!(BOOT_PROBE_JS.contains("[data-dsh-boot]"));
        assert!(BOOT_PROBE_JS.contains("phl-webui-error:report#"));
    }
}
