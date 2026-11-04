//! In-game popups: a small transparent, click-through, always-on-top window
//! that never takes focus and is excluded from capture. Created lazily on the
//! first popup so idle memory stays low.

use crate::state::AppState;
use gc_capture::{Toast, ToastKind};
use gc_platform::sound::{self, Chime};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "overlay";
/// Logical size of the popup.
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 92.0;
const MARGIN: f64 = 24.0;
const VISIBLE_FOR: Duration = Duration::from_millis(3600);

#[derive(Default)]
pub struct OverlayState {
    ready: AtomicBool,
    creating: AtomicBool,
    pending: Mutex<Option<Toast>>,
    generation: AtomicU64,
}

fn chime_for(kind: ToastKind) -> Chime {
    match kind {
        ToastKind::Ready => Chime::Ready,
        ToastKind::Saved => Chime::Saved,
        ToastKind::RecordStart => Chime::RecordStart,
        ToastKind::RecordStop => Chime::RecordStop,
        ToastKind::Bookmark | ToastKind::Screenshot => Chime::Tick,
        ToastKind::Error => Chime::Error,
    }
}

/// Plays the chime and shows the popup (if enabled in settings).
pub fn show_toast(app: &AppHandle, toast: Toast) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    let cfg = state.settings.read().overlay.clone();
    if !cfg.enabled {
        return;
    }
    if cfg.sound {
        sound::play(chime_for(toast.kind), cfg.volume);
    }
    let ov = app.state::<OverlayState>();
    if !ov.ready.load(Ordering::SeqCst) {
        *ov.pending.lock() = Some(toast);
        ensure_window(app);
        return;
    }
    present(app, toast);
}

/// Called by the overlay page once it is listening for events.
pub fn on_ready(app: &AppHandle) {
    let ov = app.state::<OverlayState>();
    ov.ready.store(true, Ordering::SeqCst);
    let pending = ov.pending.lock().take();
    if let Some(t) = pending {
        present(app, t);
    }
}

fn ensure_window(app: &AppHandle) {
    let ov = app.state::<OverlayState>();
    if app.get_webview_window(LABEL).is_some() || ov.creating.swap(true, Ordering::SeqCst) {
        return;
    }
    let app2 = app.clone();
    std::thread::spawn(move || {
        let built =
            WebviewWindowBuilder::new(&app2, LABEL, WebviewUrl::App("index.html#overlay".into()))
                .title("game-capture overlay")
                .inner_size(WIDTH, HEIGHT)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .resizable(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .visible(false)
                .build();
        match built {
            Ok(w) => {
                let _ = w.set_ignore_cursor_events(true);
                #[cfg(windows)]
                if let Ok(h) = w.hwnd() {
                    gc_platform::overlay::prepare(h.0 as isize);
                }
            }
            Err(e) => log::warn!("overlay window: {e}"),
        }
        app2.state::<OverlayState>()
            .creating
            .store(false, Ordering::SeqCst);
    });
}

/// Physical rectangle of the monitor the game is on (else the primary).
fn target_monitor(app: &AppHandle) -> Option<(i32, i32, i32, i32, f64)> {
    let monitors = app.available_monitors().ok()?;
    let state = app.state::<Arc<AppState>>();
    let game_hwnd = state
        .engine(|e| e.status().game.map(|g| g.pid))
        .flatten()
        .and_then(gc_platform::process::main_window);
    let centre = game_hwnd
        .and_then(gc_platform::display::window_placement)
        .map(|pl| {
            (
                pl.monitor.x + pl.monitor.width as i32 / 2,
                pl.monitor.y + pl.monitor.height as i32 / 2,
            )
        });
    let pick = centre
        .and_then(|(cx, cy)| {
            monitors.iter().find(|m| {
                let (p, s) = (m.position(), m.size());
                cx >= p.x && cy >= p.y && cx < p.x + s.width as i32 && cy < p.y + s.height as i32
            })
        })
        .cloned()
        .or_else(|| app.primary_monitor().ok().flatten())
        .or_else(|| monitors.first().cloned())?;
    let (p, s) = (pick.position(), pick.size());
    Some((
        p.x,
        p.y,
        s.width as i32,
        s.height as i32,
        pick.scale_factor(),
    ))
}

fn present(app: &AppHandle, toast: Toast) {
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    let corner = app
        .state::<Arc<AppState>>()
        .settings
        .read()
        .overlay
        .position;
    let (mx, my, mw, mh, scale) = target_monitor(app).unwrap_or((0, 0, 1920, 1080, 1.0));
    let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
    let (x, y) = gc_platform::overlay::corner_position(
        (mx, my, mw, mh),
        (w, h),
        (MARGIN * scale) as i32,
        corner,
    );
    let _ = app.emit_to(
        LABEL,
        "overlay-toast",
        serde_json::json!({ "toast": toast, "corner": corner }),
    );

    #[cfg(windows)]
    let hwnd = win.hwnd().ok().map(|h| h.0 as isize);
    #[cfg(not(windows))]
    let hwnd: Option<isize> = None;
    match hwnd {
        Some(native) => gc_platform::overlay::show(native, x, y, w, h),
        None => {
            let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            let _ = win.show();
        }
    }

    let ov = app.state::<OverlayState>();
    let gen = ov.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(VISIBLE_FOR);
        if app2
            .state::<OverlayState>()
            .generation
            .load(Ordering::SeqCst)
            != gen
        {
            return; // a newer popup is showing
        }
        if let Some(win) = app2.get_webview_window(LABEL) {
            #[cfg(windows)]
            if let Ok(h) = win.hwnd() {
                gc_platform::overlay::hide(h.0 as isize);
                return;
            }
            let _ = win.hide();
        }
    });
}

/// "Preview" button in the settings.
pub fn preview(app: &AppHandle) {
    show_toast(
        app,
        Toast {
            kind: ToastKind::Saved,
            title: "Saved last 2:00".into(),
            detail: Some("This is how popups look in game".into()),
        },
    );
}
