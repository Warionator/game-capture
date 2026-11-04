//! Tauri application shell for game-capture.

mod commands;
mod logger;
mod overlay;
mod state;
mod tray;

use state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn run() {
    let minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(overlay::OverlayState::default())
        .setup(move |app| {
            state::init(app.handle(), minimized).map_err(|e| e.to_string())?;
            tray::create(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window
                    .app_handle()
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.read().system.close_to_tray)
                    .unwrap_or(true);
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_status,
            commands::list_media,
            commands::get_media,
            commands::list_games,
            commands::game_icon,
            commands::delete_media,
            commands::reveal_media,
            commands::open_media_external,
            commands::copy_media,
            commands::open_storage_folder,
            commands::rescan_library,
            commands::list_processes,
            commands::prepare_audio_tracks,
            commands::export_clip,
            commands::process_icon,
            commands::add_custom_game,
            commands::list_encoders,
            commands::reprobe_encoders,
            commands::list_monitors,
            commands::list_audio_devices,
            commands::get_audio_levels,
            commands::set_track_volume,
            commands::validate_hotkey,
            commands::hotkey_errors,
            commands::action,
            commands::system_info,
            commands::overlay_ready,
            commands::preview_overlay,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        .run(|app, event| match event {
            // Closing the window only hides it; the app lives in the tray.
            RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                let close_to_tray = app
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.read().system.close_to_tray)
                    .unwrap_or(true);
                if close_to_tray {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => {
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    // Stops capture and finalises an active recording.
                    drop(state.engine.lock().take());
                    drop(state.hook.lock().take());
                    drop(state.watcher.lock().take());
                }
                log::logger().flush();
            }
            _ => {}
        });
}
