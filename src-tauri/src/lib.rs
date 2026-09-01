use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![app_ready, exit_app])
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
