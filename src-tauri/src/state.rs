//! Application state and the background threads that glue the engine,
//! process watcher, hotkeys and library together.

use crate::tray;
use crossbeam_channel::unbounded;
use gc_capture::{CaptureEngine, EngineEvent, EnginePaths, Ffmpeg, GameContext};
use gc_core::config::{GameSettings, Settings};
use gc_core::hotkey::{self, HotkeyAction};
use gc_library::{FfmpegInspector, Library};
use gc_platform::hotkeys::{Bindings, HotkeyHook};
use gc_platform::watcher::{ProcessWatcher, WatchEvent};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub library: Arc<Mutex<Library>>,
    pub thumbs_dir: PathBuf,
    pub engine: Mutex<Option<CaptureEngine>>,
    pub watcher: Mutex<Option<ProcessWatcher>>,
    pub game_cfg: Arc<RwLock<GameSettings>>,
    pub bindings: Bindings,
    pub hook: Mutex<Option<HotkeyHook>>,
    pub ffmpeg: Option<Ffmpeg>,
    pub inspector: Option<Arc<FfmpegInspector>>,
    pub scanning: AtomicBool,
    pub hotkey_errors: RwLock<Vec<String>>,
}

impl AppState {
    pub fn engine<R>(&self, f: impl FnOnce(&CaptureEngine) -> R) -> Option<R> {
        self.engine.lock().as_ref().map(f)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub error: bool,
    pub message: String,
}

fn ffmpeg_search_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        dirs.push(res.join("binaries"));
        dirs.push(res);
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        dirs.push(exe_dir.join("binaries"));
        dirs.push(exe_dir);
    }
    // `cargo tauri dev` runs from src-tauri/.
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    dirs
}

pub fn init(app: &AppHandle, minimized: bool) -> anyhow::Result<()> {
    let config_dir = app.path().app_config_dir()?;
    let data_dir = app.path().app_local_data_dir()?;
    crate::logger::init(app.path().app_log_dir()?.join("game-capture.log"));
    log::info!("game-capture {} starting", env!("CARGO_PKG_VERSION"));

    let settings_path = config_dir.join("settings.json");
    let first_run = !settings_path.exists();
    let settings = Settings::load_or_default(&settings_path);
    if first_run {
        settings.save(&settings_path)?;
    }
    let _ = std::fs::create_dir_all(&settings.storage_root);

    let library = Arc::new(Mutex::new(Library::open(&data_dir.join("library.sqlite"))?));
    let thumbs_dir = data_dir.join("thumbnails");
    let temp_dir = data_dir.join("temp");
    std::fs::create_dir_all(&thumbs_dir)?;
    std::fs::create_dir_all(&temp_dir)?;

    let ffmpeg = match Ffmpeg::locate(&ffmpeg_search_dirs(app)) {
        Ok(f) => {
            log::info!("using {}", f.ffmpeg.display());
            Some(f)
        }
        Err(e) => {
            log::error!("{e}");
            None
        }
    };
    let inspector = ffmpeg.as_ref().map(|f| {
        Arc::new(FfmpegInspector {
            ffmpeg: f.ffmpeg.clone(),
            ffprobe: f.ffprobe.clone(),
        })
    });

    allow_asset_dirs(app, &settings.library_roots(), &thumbs_dir);

    // --- capture engine --------------------------------------------------
    let (ev_tx, ev_rx) = unbounded::<EngineEvent>();
    let engine = CaptureEngine::new(
        EnginePaths {
            ffmpeg: ffmpeg.clone(),
            temp_dir,
        },
        settings.clone(),
        ev_tx,
    );

    let game_cfg = Arc::new(RwLock::new(settings.games.clone()));
    let (bind, errs) = hotkey::bindings(&settings.hotkeys);
    let bindings: Bindings = Arc::new(RwLock::new(bind));

    let state = Arc::new(AppState {
        settings: RwLock::new(settings.clone()),
        settings_path,
        library,
        thumbs_dir,
        engine: Mutex::new(Some(engine)),
        watcher: Mutex::new(None),
        game_cfg: game_cfg.clone(),
        bindings: bindings.clone(),
        hook: Mutex::new(None),
        ffmpeg,
        inspector,
        scanning: AtomicBool::new(false),
        hotkey_errors: RwLock::new(errs),
    });
    app.manage(state.clone());

    // Engine events → UI + library.
    {
        let app = app.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name("gc-engine-events".into())
            .spawn(move || {
                for ev in ev_rx {
                    match ev {
                        EngineEvent::Status(st) => {
                            tray::update_status(&app, &st);
                            let _ = app.emit("status", st);
                        }
                        EngineEvent::Notice { error, message } => {
                            let _ = app.emit("notice", Notice { error, message });
                        }
                        EngineEvent::Saved(saved) => {
                            let label = saved.kind.label();
                            let _ = app.emit(
                                "notice",
                                Notice {
                                    error: false,
                                    message: format!(
                                        "{label} saved: {}",
                                        saved
                                            .path
                                            .file_name()
                                            .map(|n| n.to_string_lossy())
                                            .unwrap_or_default()
                                    ),
                                },
                            );
                            index_new_file(&app, &state, saved.path);
                        }
                    }
                }
            })?;
    }

    // Process watcher → engine.
    {
        let (w_tx, w_rx) = unbounded::<WatchEvent>();
        *state.watcher.lock() = Some(ProcessWatcher::spawn(game_cfg, w_tx));
        let app = app.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name("gc-watch-events".into())
            .spawn(move || {
                for ev in w_rx {
                    match ev {
                        WatchEvent::Started(g) => {
                            if let Some(png) = &g.icon_png {
                                let _ = state.library.lock().set_game_icon(&g.display_name, png);
                            }
                            let ctx = GameContext {
                                pid: g.pid,
                                exe_name: g.exe_name.clone(),
                                display_name: g.display_name.clone(),
                                hwnd: gc_platform::process::main_window(g.pid),
                            };
                            state.engine(|e| e.set_game(Some(ctx)));
                            let _ = app.emit("game", Some(g));
                        }
                        WatchEvent::Stopped(_) => {
                            state.engine(|e| e.set_game(None));
                            let _ = app.emit("game", Option::<()>::None);
                        }
                    }
                }
            })?;
    }

    // Global hotkeys → engine.
    {
        let (h_tx, h_rx) = crossbeam_channel::bounded::<HotkeyAction>(16);
        match HotkeyHook::install(bindings, h_tx) {
            Ok(h) => *state.hook.lock() = Some(h),
            Err(e) => log::error!("installing keyboard hook failed: {e}"),
        }
        let state = state.clone();
        std::thread::Builder::new()
            .name("gc-hotkey-events".into())
            .spawn(move || {
                for action in h_rx {
                    log::debug!("hotkey: {action:?}");
                    state.engine(|e| match action {
                        HotkeyAction::SaveReplay => e.save_replay(),
                        HotkeyAction::ToggleRecording => e.toggle_recording(),
                        HotkeyAction::AddBookmark => e.add_bookmark(),
                        HotkeyAction::Screenshot => e.screenshot(),
                    });
                }
            })?;
    }

    apply_system_settings(&settings, None);

    // Crash recovery, then the self-healing library scan.
    {
        let app = app.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name("gc-startup-scan".into())
            .spawn(move || {
                if let Some(ff) = &state.ffmpeg {
                    let root = state.settings.read().storage_root.clone();
                    let recovered = gc_capture::recover_sessions(&root, ff);
                    if !recovered.is_empty() {
                        let _ = app.emit(
                            "notice",
                            Notice {
                                error: false,
                                message: format!(
                                    "Recovered {} interrupted recording(s)",
                                    recovered.len()
                                ),
                            },
                        );
                    }
                }
                run_scan(&app, &state);
            })?;
    }

    if !minimized {
        crate::show_main(app);
    }
    Ok(())
}

/// Lets the webview load thumbnails and videos from the library folders.
pub fn allow_asset_dirs(app: &AppHandle, roots: &[PathBuf], thumbs: &std::path::Path) {
    let scope = app.asset_protocol_scope();
    for dir in roots.iter().map(PathBuf::as_path).chain([thumbs]) {
        if let Err(e) = scope.allow_directory(dir, true) {
            log::warn!("asset scope for {}: {e}", dir.display());
        }
    }
}

/// Writes Windows integration settings (Game Mode, autostart). `prev` is the
/// previous settings; only changed values are written.
pub fn apply_system_settings(s: &Settings, prev: Option<&Settings>) {
    use gc_platform::system;
    let changed = |f: fn(&Settings) -> bool| prev.is_none_or(|p| f(p) != f(s));
    if changed(|s| s.system.disable_game_mode) {
        let want_enabled = !s.system.disable_game_mode;
        if system::game_mode_enabled() != Some(want_enabled) {
            if let Err(e) = system::set_game_mode(want_enabled) {
                log::warn!("updating Game Mode failed: {e}");
            }
        }
    }
    // Development builds never register themselves for autostart.
    if !cfg!(debug_assertions) && changed(|s| s.system.autostart) {
        if let Ok(exe) = std::env::current_exe() {
            if let Err(e) = system::set_autostart(s.system.autostart, &exe) {
                log::warn!("updating autostart failed: {e}");
            }
        }
    }
}

/// Indexes a freshly saved file and tells the UI.
pub fn index_new_file(app: &AppHandle, state: &AppState, path: PathBuf) {
    let Some(inspector) = state.inspector.clone() else {
        return;
    };
    let root = state.settings.read().storage_root.clone();
    match gc_library::index_file(
        &state.library,
        &root,
        &state.thumbs_dir,
        inspector.as_ref(),
        &path,
    ) {
        Ok(item) => {
            let _ = app.emit("media-added", item);
        }
        Err(e) => log::warn!("indexing {} failed: {e}", path.display()),
    }
}

pub fn run_scan(app: &AppHandle, state: &AppState) -> Option<gc_library::ScanReport> {
    let inspector = state.inspector.clone()?;
    if state.scanning.swap(true, Ordering::SeqCst) {
        return None;
    }
    let _ = app.emit("scan-progress", true);
    let roots = state.settings.read().library_roots();
    let report = gc_library::scan(
        &state.library,
        &roots,
        &state.thumbs_dir,
        inspector.as_ref(),
        |_| {},
    );
    state.scanning.store(false, Ordering::SeqCst);
    let _ = app.emit("library-changed", report.clone());
    let _ = app.emit("scan-progress", false);
    Some(report)
}
