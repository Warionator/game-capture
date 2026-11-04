//! Native tweaks for the in-game popup window: it must never take focus
//! (that would minimise exclusive-fullscreen games and trigger the focus
//! screen), stay above the game, ignore the mouse, and never appear in
//! captures or screenshots.

/// Marks `hwnd` as a non-activating, click-through tool window that is
/// excluded from screen capture (Windows 10 2004+).
pub fn prepare(hwnd: isize) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, GWL_EXSTYLE,
            WDA_EXCLUDEFROMCAPTURE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        };
        let h = HWND(hwnd as *mut _);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let add = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) as isize;
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | add);
        if let Err(e) = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) {
            log::debug!("overlay capture exclusion unavailable: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = hwnd;
}

/// Shows the window at a physical-pixel rectangle without activating it.
pub fn show(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        };
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOPMOST),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    #[cfg(not(windows))]
    let _ = (hwnd, x, y, w, h);
}

pub fn hide(hwnd: isize) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
    }
    #[cfg(not(windows))]
    let _ = hwnd;
}

/// Top-left corner for a `w`×`h` popup in a monitor work area
/// `(x, y, mw, mh)` at the given corner, with `margin` pixels of spacing.
pub fn corner_position(
    monitor: (i32, i32, i32, i32),
    size: (i32, i32),
    margin: i32,
    corner: gc_core::config::OverlayPosition,
) -> (i32, i32) {
    use gc_core::config::OverlayPosition::*;
    let (mx, my, mw, mh) = monitor;
    let (w, h) = size;
    match corner {
        TopLeft => (mx + margin, my + margin),
        TopRight => (mx + mw - w - margin, my + margin),
        BottomLeft => (mx + margin, my + mh - h - margin),
        BottomRight => (mx + mw - w - margin, my + mh - h - margin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gc_core::config::OverlayPosition;

    #[test]
    fn corners() {
        let m = (1920, 0, 2560, 1440);
        assert_eq!(
            corner_position(m, (380, 90), 24, OverlayPosition::TopLeft),
            (1944, 24)
        );
        assert_eq!(
            corner_position(m, (380, 90), 24, OverlayPosition::BottomRight),
            (1920 + 2560 - 404, 1440 - 114)
        );
    }
}
