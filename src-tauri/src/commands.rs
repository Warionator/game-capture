//! `#[tauri::command]` handlers invoked by the React UI.

use crate::state::{allow_asset_dirs, apply_system_settings, run_scan, AppState};
use base64::Engine as _;
use gc_capture::{EncoderInfo, EngineStatus};
use gc_core::config::{CustomGame, Settings};
use gc_core::hotkey::{self, Hotkey};
use gc_library::{GameSummary, MediaFilter, MediaItem, ScanReport};
use gc_platform::{display, process, shell, system};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

type St<'a> = State<'a, Arc<AppState>>;
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_settings(state: St) -> Settings {
    state.settings.read().clone()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: St, settings: Settings) -> CmdResult<Settings> {
    let mut new = settings;
    new.sanitize();
    let prev = state.settings.read().clone();
    if new == prev {
        return Ok(new);
    }
    new.save(&state.settings_path).map_err(err)?;
    *state.settings.write() = new.clone();

    if new.hotkeys != prev.hotkeys {
        let (b, errs) = hotkey::bindings(&new.hotkeys);
        *state.bindings.write() = b;
        *state.hotkey_errors.write() = errs;
    }
    if new.games != prev.games {
        *state.game_cfg.write() = new.games.clone();
        if let Some(w) = state.watcher.lock().as_ref() {
            w.rescan();
        }
    }
    apply_system_settings(&new, Some(&prev));
    state.engine(|e| e.update_settings(new.clone()));
    if new.storage_root != prev.storage_root {
        let _ = std::fs::create_dir_all(&new.storage_root);
        allow_asset_dirs(&app, &new.storage_root, &state.thumbs_dir);
        let (app, state) = (app.clone(), state.inner().clone());
        std::thread::spawn(move || run_scan(&app, &state));
    }
    Ok(new)
}

#[tauri::command]
pub fn get_status(state: St) -> Option<EngineStatus> {
    state.engine(|e| e.status())
}

#[tauri::command]
pub fn list_media(state: St, filter: MediaFilter) -> CmdResult<Vec<MediaItem>> {
    state.library.lock().list(&filter).map_err(err)
}

#[tauri::command]
pub fn get_media(state: St, id: i64) -> CmdResult<Option<MediaItem>> {
    state.library.lock().get(id).map_err(err)
}

#[tauri::command]
pub fn list_games(state: St) -> CmdResult<Vec<GameSummary>> {
    state.library.lock().games().map_err(err)
}

/// Game icon as a `data:image/png;base64,…` URL.
#[tauri::command]
pub fn game_icon(state: St, title: String) -> Option<String> {
    let png = state.library.lock().game_icon(&title).ok().flatten()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

fn media_path(state: &AppState, id: i64) -> CmdResult<(MediaItem, PathBuf)> {
    let item = state
        .library
        .lock()
        .get(id)
        .map_err(err)?
        .ok_or_else(|| "file is no longer in the library".to_string())?;
    let p = item.path.clone();
    Ok((item, p))
}

#[tauri::command]
pub fn delete_media(state: St, id: i64) -> CmdResult<()> {
    let (item, path) = media_path(&state, id)?;
    shell::delete_file(&path).map_err(err)?;
    state.library.lock().remove(id).map_err(err)?;
    if let Some(t) = item.thumbnail {
        let _ = std::fs::remove_file(t);
    }
    Ok(())
}

#[tauri::command]
pub fn reveal_media(state: St, id: i64) -> CmdResult<()> {
    let (_, path) = media_path(&state, id)?;
    shell::reveal_in_explorer(&path).map_err(err)
}

#[tauri::command]
pub fn open_media_external(state: St, id: i64) -> CmdResult<()> {
    let (_, path) = media_path(&state, id)?;
    shell::open_default(&path).map_err(err)
}

#[tauri::command]
pub fn copy_media(state: St, id: i64) -> CmdResult<()> {
    let (_, path) = media_path(&state, id)?;
    shell::copy_files_to_clipboard(&[path.as_path()]).map_err(err)
}

#[tauri::command]
pub fn open_storage_folder(state: St) -> CmdResult<()> {
    let root = state.settings.read().storage_root.clone();
    std::fs::create_dir_all(&root).map_err(err)?;
    shell::open_default(&root).map_err(err)
}

#[tauri::command]
pub async fn rescan_library(app: AppHandle, state: St<'_>) -> CmdResult<Option<ScanReport>> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || run_scan(&app, &state))
        .await
        .map_err(err)
}

#[tauri::command]
pub fn list_processes() -> Vec<process::WindowedProcess> {
    process::windowed_processes()
}

#[tauri::command]
pub fn process_icon(path: String) -> Option<String> {
    let png = process::icon_png(&path, 64)?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// Registers an executable as a game (manual override).
#[tauri::command]
pub fn add_custom_game(
    app: AppHandle,
    state: St,
    exe_path: String,
    display_name: Option<String>,
) -> CmdResult<Settings> {
    let path = Path::new(&exe_path);
    let executable = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "not a file path".to_string())?
        .to_string();
    let name = display_name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| process::version_display_name(&exe_path))
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let mut s = state.settings.read().clone();
    s.games
        .custom
        .retain(|g| !g.executable.eq_ignore_ascii_case(&executable));
    s.games.custom.push(CustomGame {
        display_name: name.clone(),
        executable,
        path: Some(PathBuf::from(&exe_path)),
    });
    if let Some(png) = process::icon_png(&exe_path, 128) {
        let _ = state.library.lock().set_game_icon(&name, &png);
    }
    save_settings(app, state, s)
}

#[tauri::command]
pub fn list_encoders(state: St) -> Vec<EncoderInfo> {
    state.engine(|e| e.encoders()).unwrap_or_default()
}

#[tauri::command]
pub fn reprobe_encoders(state: St) {
    state.engine(|e| e.reprobe_encoders());
}

#[tauri::command]
pub fn list_monitors() -> Vec<display::MonitorInfo> {
    display::list_monitors()
}

#[tauri::command]
pub fn list_audio_devices() -> Vec<gc_audio::AudioDevice> {
    gc_audio::list_input_devices()
}

#[tauri::command]
pub fn get_audio_levels(state: St) -> Vec<gc_audio::TrackLevel> {
    state.engine(|e| e.audio_levels()).unwrap_or_default()
}

/// Live gain change (does not persist; the UI saves settings separately).
#[tauri::command]
pub fn set_track_volume(state: St, title: String, volume: f32, muted: bool) {
    state.engine(|e| e.set_volume(&title, volume, muted));
}

#[tauri::command]
pub fn validate_hotkey(text: String) -> CmdResult<String> {
    Hotkey::parse(&text).map(|h| h.to_string()).map_err(err)
}

#[tauri::command]
pub fn hotkey_errors(state: St) -> Vec<String> {
    state.hotkey_errors.read().clone()
}

#[tauri::command]
pub fn action(state: St, name: String) -> CmdResult<()> {
    state
        .engine(|e| {
            match name.as_str() {
                "saveReplay" => e.save_replay(),
                "toggleRecording" => e.toggle_recording(),
                "bookmark" => e.add_bookmark(),
                "screenshot" => e.screenshot(),
                "startCapture" => e.start_manual(),
                "stopCapture" => e.stop_capture(),
                "pause" => e.set_paused(true),
                "resume" => e.set_paused(false),
                other => return Err(format!("unknown action {other}")),
            }
            Ok(())
        })
        .unwrap_or_else(|| Err("engine stopped".into()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub version: &'static str,
    pub windows_build: Option<u32>,
    pub process_loopback: bool,
    pub game_mode_enabled: Option<bool>,
    pub autostart_enabled: bool,
    pub ffmpeg_path: Option<String>,
    pub settings_path: String,
    pub library_items: u64,
    pub platform: &'static str,
}

#[tauri::command]
pub fn system_info(state: St) -> SystemInfo {
    SystemInfo {
        version: env!("CARGO_PKG_VERSION"),
        windows_build: system::windows_build(),
        process_loopback: system::supports_process_loopback(),
        game_mode_enabled: system::game_mode_enabled(),
        autostart_enabled: system::autostart_enabled(),
        ffmpeg_path: state
            .ffmpeg
            .as_ref()
            .map(|f| f.ffmpeg.display().to_string()),
        settings_path: state.settings_path.display().to_string(),
        library_items: state.library.lock().count().unwrap_or(0),
        platform: std::env::consts::OS,
    }
}
