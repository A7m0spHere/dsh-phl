use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};

mod api_config;
mod credentials;
mod diagnostics;
mod instances;
mod launch;
mod paths;
mod plugins;
mod repair;
mod runtimes;
mod storage;
mod verify;
mod versions;

/// Emitted when the OS (or the custom title bar) asks the window to close.
/// The frontend answers with its own confirmation dialog instead of letting
/// the window vanish under a user who has instances running.
const CLOSE_REQUESTED: &str = "phl://close-requested";

/// The window starts hidden so the user never sees an unpainted white frame.
/// The frontend calls this once React has mounted and the theme is applied.
#[tauri::command]
fn app_ready(window: tauri::Window) {
    let _ = window.show();
    let _ = window.set_focus();
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(versions::Transfers::default())
        .manage(launch::Launches::default())
        .manage(launch::Processes::default())
        .manage(paths::PhlState::load())
        .manage(credentials::Creds::platform_default())
        .invoke_handler(tauri::generate_handler![
            app_ready,
            exit_app,
            open_external,
            reveal_path,
            paths::init_phl_root,
            paths::set_phl_root,
            versions::default_root,
            versions::list_dsh_versions,
            versions::download_dsh_version,
            versions::cancel_transfer,
            versions::list_installed_versions,
            versions::remove_version_dir,
            plugins::list_dsh_plugins,
            plugins::install_plugin,
            plugins::plugin_latest_version,
            plugins::set_plugin_enabled,
            plugins::uninstall_plugin,
            instances::list_instances,
            instances::create_instance,
            instances::save_instance,
            instances::delete_instance,
            instances::clone_instance,
            instances::instance_disk_usage,
            instances::scan_orphan_instances,
            instances::delete_orphan_instance,
            instances::export_instance_bundle,
            instances::read_instance_bundle,
            instances::import_instance_bundle,
            instances::create_instance_snapshot,
            instances::restore_instance_snapshot,
            instances::delete_instance_snapshot,
            runtimes::list_node_runtimes,
            runtimes::list_installed_runtimes,
            runtimes::system_node_version,
            runtimes::download_node_runtime,
            runtimes::remove_runtime_dir,
            runtimes::runtimes_disk_usage,
            launch::launch_instance,
            launch::stop_instance,
            launch::cancel_launch,
            api_config::load_api_config,
            api_config::save_api_config,
            api_config::sync_instance_api,
            api_config::import_instance_api,
            api_config::instance_live_snapshot,
            api_config::fetch_provider_models,
            storage::free_space,
            storage::root_data_summary,
            storage::move_root_data,
            repair::repair_instance,
            repair::scan_residue,
            verify::verify_instance,
            diagnostics::run_diagnostics,
            diagnostics::clear_download_cache,
        ])
        .setup(|app| {
            // Safety net: if the frontend fails to boot it can never call
            // `app_ready`, and a permanently invisible window looks like a
            // crash. Reveal it anyway so the error is at least visible.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(4));
                if let Some(window) = handle.get_webview_window("main") {
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit(CLOSE_REQUESTED, ());
            }
        })
        .run(tauri::generate_context!())
        .expect("PHL failed to start");
}
