use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};

mod api_config;
mod credentials;
mod diagnostics;
mod discovery;
mod errors;
mod instances;
mod launch;
mod pack;
mod paths;
mod plugins;
mod repair;
mod resources;
mod runtimes;
mod sessions;
mod storage;
mod verify;
mod versions;
mod webui;

#[cfg(test)]
mod ipc_contract;

/// The Windows E2E release gate: drives the real pipelines (download →
/// install → create → launch → stop → adopt → snapshot → pack → migrate)
/// headlessly against throwaway roots, with system-level fault injection
/// (force-kill, mid-stream download abort, late-exiting processes). It runs
/// on the `windows-e2e` CI job via `cargo test --workspace -- --ignored
/// release_e2e`; the scenario map lives in maintainer-local gate docs.
#[cfg(all(test, windows))]
mod release_e2e;

/// Emitted when the OS (or the custom title bar) asks the window to close.
/// The frontend answers with its own confirmation dialog instead of letting
/// the window vanish under a user who has instances running.
const CLOSE_REQUESTED: &str = "phl://close-requested";

/// The window starts hidden so the user never sees an unpainted white frame.
/// The frontend calls this once React has mounted and the theme is applied.
#[tauri::command]
fn app_ready(window: tauri::Window) {
    if let Some(main) = window.get_webview_window("main") {
        reveal(&main);
    }
}

/// Make the window actually visible *and reachable*: `show()` alone does not
/// undo a minimized or off-screen placement, and both have been observed in
/// dev relaunches — the app is then fully alive behind a window parked at
/// (-32000,-32000), indistinguishable from "it never opened". Un-minimize,
/// show, focus, and if the restored frame lies outside every monitor, pull it
/// back to the primary display. This is the "never invisible" promise from
/// AGENTS.md applied to placement, not just visibility.
fn reveal(window: &tauri::WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) {
        let on_any = window
            .available_monitors()
            .map(|mons| {
                mons.iter().any(|m| {
                    let mp = m.position();
                    let ms = m.size();
                    // The window's centre must fall inside some monitor's
                    // rect; anything else and the title bar is unreachable.
                    pos.x + size.width as i32 / 2 >= mp.x
                        && pos.x < mp.x + ms.width as i32
                        && pos.y + size.height as i32 / 2 >= mp.y
                        && pos.y < mp.y + ms.height as i32
                })
            })
            .unwrap_or(true);
        if !on_any {
            let _ = window.center();
        }
    }
}

/// Gives the window the icon frame that matches this display's scaling.
///
/// Windows draws the taskbar button from the *window* icon, and Tauri can only
/// set one: the single frame it decoded out of `icons/icon.ico`, which is the
/// 16×16 one. A 125% display asks the taskbar for 20×20, so that bitmap gets
/// upscaled and the mark smears. Clearing the window icon instead (the first
/// attempt) only moved the problem — Windows then falls back to the icon
/// embedded in the executable, which Explorer serves from its icon cache, and
/// that cache still held the pre-whale artwork. Setting the right frame
/// explicitly sidesteps both the upscale and the cache.
fn apply_taskbar_icon(window: &tauri::WebviewWindow) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let png: &[u8] = if scale >= 1.5 {
        include_bytes!("../icons/24x24.png")
    } else if scale >= 1.25 {
        include_bytes!("../icons/20x20.png")
    } else {
        include_bytes!("../icons/16x16.png")
    };
    if let Ok(icon) = tauri::image::Image::from_bytes(png) {
        let _ = window.set_icon(icon);
    }
}

/// Called by the frontend after the user confirms the exit dialog.
#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// Opens a URL in the system browser. `<a target="_blank">` does nothing in
/// a Tauri window, so detail-page links (plugin repo / npm) go through here.
///
/// Both halves of this matter. The scheme whitelist keeps non-http(s) URLs
/// away from the OS handler, and the launcher deliberately avoids any shell:
/// these URLs come from the community registry and from screenshot links
/// scraped out of arbitrary plugin READMEs, so they are attacker-controlled.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 2048
        && url
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != '<' && c != '>' && c != '"');
    if !ok {
        return Err(format!("拒绝打开非 http(s) 链接: {url}"));
    }
    #[cfg(target_os = "windows")]
    {
        // NOT `cmd /c start`: cmd.exe re-parses its command line, and `&` is
        // a perfectly legal query-string character — `?a=1&calc` would run
        // `calc` as a second command. Rust's argument quoting does not
        // escape cmd metacharacters, so the only safe fix is to keep the
        // shell out of the picture entirely.
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Reveals a folder in the system file manager. `open_external` deliberately
/// refuses non-http(s) targets; folder reveals are a different job with a
/// different tool — and like it, no shell is involved anywhere.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.is_dir() {
        return Err(format!("目录不存在: {path}"));
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Every command that does not need the concrete Wry runtime, listed once.
///
/// `generate_handler!` is a proc macro and cannot expand a nested
/// `macro_rules!` itself, so this wrapper inlines the list for both callers:
/// `run()` appends the Wry-bound commands (window and app handles), while
/// `build_app` registers exactly this set so `tests/ipc_contract.rs` drives the
/// same wiring the desktop build uses.
macro_rules! phl_command_handler {
    ($($extra:path),* $(,)?) => {
        tauri::generate_handler![
            resources::list_tasks,
            open_external,
            reveal_path,
            paths::init_phl_root,
            paths::set_phl_root,
            versions::default_root,
            versions::catalog::list_dsh_versions,
            versions::download_dsh_version,
            versions::cancel_transfer,
            versions::install::list_installed_versions,
            versions::install::remove_version_dir,
            plugins::catalog::list_dsh_plugins,
            plugins::install::install_plugin,
            plugins::catalog::plugin_latest_version,
            plugins::cordis::set_plugin_enabled,
            plugins::cordis::uninstall_plugin,
            instances::list_instances,
            instances::create_instance,
            instances::save_instance,
            instances::delete_instance,
            instances::clone_instance,
            instances::instance_disk_usage,
            instances::instance_session_count,
            instances::scan_orphan_instances,
            instances::delete_orphan_instance,
            instances::adoption::preview_adoption,
            instances::adoption::adopt_instance,
            instances::adoption::list_adoption_sessions,
            instances::bundle::export_instance_bundle,
            instances::bundle::preview_instance_export,
            instances::bundle::read_instance_bundle,
            instances::bundle::import_instance_bundle,
            instances::snapshot::create_instance_snapshot,
            instances::snapshot::restore_instance_snapshot,
            instances::snapshot::delete_instance_snapshot,
            discovery::discover_dsh,
            discovery::inspect_dsh_home,
            discovery::inspect_dsh_executable,
            sessions::list_sessions,
            sessions::inspect_session,
            sessions::copy_session,
            sessions::copy_sessions,
            pack::export::preview_instance_pack_export,
            pack::export::export_instance_pack,
            pack::install::preview_pack,
            pack::install::install_pack,
            runtimes::list_node_runtimes,
            runtimes::list_installed_runtimes,
            runtimes::system_node_version,
            runtimes::download_node_runtime,
            runtimes::remove_runtime_dir,
            runtimes::runtimes_disk_usage,
            launch::stop_instance,
            launch::cancel_launch,
            launch::adopt_processes,
            api_config::library::load_api_config,
            api_config::library::save_api_config,
            api_config::sync::sync_instance_api,
            api_config::sync::import_instance_api,
            api_config::sync::instance_live_snapshot,
            api_config::models::fetch_provider_models,
            storage::free_space,
            storage::root_data_summary,
            storage::move_root_data,
            storage::storage_migration_status,
            storage::storage_migration_undo,
            storage::storage_migration_finish,
            repair::repair_instance,
            verify::verify_instance,
            diagnostics::run_diagnostics,
            diagnostics::clear_download_cache,
            $($extra),*
        ]
    };
}

/// Plugins and managed state — everything a command needs to exist, for both
/// the desktop entry point and the contract tests.
fn configure<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(
            tauri::plugin::Builder::<R, ()>::new("model-metadata")
                .invoke_handler(tauri::generate_handler![
                    api_config::catalog::enrich_model_metadata
                ])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(versions::Transfers::default())
        .manage(resources::ResourceLocks::default())
        .manage(resources::Tasks::default())
        .manage(launch::Launches::default())
        .manage(launch::Processes::default())
        .manage(launch::registry::Registry::default())
        .manage(paths::PhlState::load())
        .manage(credentials::Creds::platform_default())
}

/// The runtime-agnostic command surface, for `tests/ipc_contract.rs`.
pub fn build_app<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    configure(builder).invoke_handler(phl_command_handler!())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure(tauri::Builder::default())
        .invoke_handler(phl_command_handler![
            // Wry-bound: these take a `Window`/`AppHandle` of the concrete
            // runtime, so they cannot join the shared list.
            app_ready,
            exit_app,
            launch::launch_instance,
            webui::open_or_focus_webui,
        ])
        .setup(|app| {
            // Bind the process registry before any command can see it: the
            // records a previous run wrote are the input for boot adoption.
            {
                let state = app.state::<paths::PhlState>();
                let registry = app.state::<launch::registry::Registry>();
                if let Some(path) = state.sibling_file("processes.json") {
                    registry.bind(path);
                }
            }
            // The main window's taskbar icon: hand it the frame that matches
            // this display's scaling instead of letting Tauri's 16px one be
            // upscaled (see apply_taskbar_icon).
            if let Some(window) = app.get_webview_window("main") {
                apply_taskbar_icon(&window);
            }
            // Safety net: if the frontend fails to boot it can never call
            // `app_ready`, and a permanently invisible window looks like a
            // crash. Reveal it anyway so the error is at least visible.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(4));
                if let Some(window) = handle.get_webview_window("main") {
                    // Either never shown (frontend crash) or shown-but-parked:
                    // a minimized/off-screen window is just as unreachable.
                    let hidden = !window.is_visible().unwrap_or(true);
                    let parked = window.is_minimized().unwrap_or(false);
                    if hidden || parked {
                        reveal(&window);
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // The close-confirm flow is the *app's* lifecycle, owned by the
            // main window. Embedded WebUI windows (`webui` module) close
            // normally — closing one must never prompt "quit PHL?" — and the
            // instance keeps running behind it by design.
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.emit(CLOSE_REQUESTED, ());
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("PHL failed to start");
}
