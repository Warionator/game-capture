//! Notification-area icon with quick actions.

use crate::state::AppState;
use gc_capture::{CaptureState, EngineStatus};
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

pub struct TrayHandles {
    tray: TrayIcon,
    status: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    last: Mutex<Option<(CaptureState, Option<String>)>>,
}

const ICON: &[u8] = include_bytes!("../icons/tray.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray-paused.png");

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(
        app,
        "status",
        "Idle – waiting for a game",
        false,
        None::<&str>,
    )?;
    let save = MenuItem::with_id(app, "save_replay", "Save Replay", true, None::<&str>)?;
    let record = MenuItem::with_id(
        app,
        "toggle_recording",
        "Start / Stop Recording",
        true,
        None::<&str>,
    )?;
    let pause = MenuItem::with_id(app, "toggle_pause", "Pause Capture", true, None::<&str>)?;
    let dashboard = MenuItem::with_id(app, "dashboard", "Open Dashboard", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &save,
            &record,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &dashboard,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON)?)
        .tooltip("game-capture")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let state = app.state::<Arc<AppState>>();
            match event.id.as_ref() {
                "save_replay" => {
                    state.engine(|e| e.save_replay());
                }
                "toggle_recording" => {
                    state.engine(|e| e.toggle_recording());
                }
                "toggle_pause" => {
                    let paused = state
                        .engine(|e| e.status().state == CaptureState::Paused)
                        .unwrap_or(false);
                    state.engine(|e| e.set_paused(!paused));
                }
                "dashboard" => crate::show_main(app),
                "settings" => {
                    crate::show_main(app);
                    let _ = app.emit("navigate", "settings");
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayHandles {
        tray,
        status,
        pause,
        last: Mutex::new(None),
    });
    Ok(())
}

fn describe(st: &EngineStatus) -> String {
    let game = st.game.as_ref().map(|g| g.display_name.as_str());
    match (st.state, game) {
        (CaptureState::Recording, g) => {
            let s = st.recording_secs as u64;
            format!(
                "● Recording {}:{:02}:{:02}{}",
                s / 3600,
                s / 60 % 60,
                s % 60,
                g.map(|g| format!(" – {g}")).unwrap_or_default()
            )
        }
        (CaptureState::Buffering, Some(g)) => format!("Replay buffer on – {g}"),
        (CaptureState::Buffering, None) => "Replay buffer on – desktop".into(),
        (CaptureState::Starting, _) => "Starting capture…".into(),
        (CaptureState::Paused, _) => "Paused".into(),
        (CaptureState::Error, _) => "Capture error – open dashboard".into(),
        (CaptureState::Idle, _) => "Idle – waiting for a game".into(),
    }
}

pub fn update_status(app: &AppHandle, st: &EngineStatus) {
    let Some(h) = app.try_state::<TrayHandles>() else {
        return;
    };
    let text = describe(st);
    let key = (st.state, Some(text.clone()));
    {
        let mut last = h.last.lock();
        if last.as_ref() == Some(&key) {
            return;
        }
        let was_paused = last.as_ref().map(|l| l.0 == CaptureState::Paused);
        *last = Some(key);
        let paused = st.state == CaptureState::Paused;
        if was_paused != Some(paused) {
            let _ = h.pause.set_text(if paused {
                "Resume Capture"
            } else {
                "Pause Capture"
            });
            if let Ok(img) = Image::from_bytes(if paused { ICON_PAUSED } else { ICON }) {
                let _ = h.tray.set_icon(Some(img));
            }
        }
    }
    let _ = h.status.set_text(&text);
    let _ = h.tray.set_tooltip(Some(format!("game-capture – {text}")));
}
