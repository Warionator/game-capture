//! DXGI output enumeration and window → output mapping, used to point the
//! Desktop Duplication source at the monitor the game is on.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    /// Index into `IDXGIFactory1::EnumAdapters1`.
    pub adapter_idx: u32,
    /// Index into `IDXGIAdapter::EnumOutputs` for that adapter.
    pub output_idx: u32,
    /// Global index across all adapters (what the settings UI stores).
    pub global_idx: u32,
    pub device_name: String,
    pub adapter_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    #[serde(skip)]
    pub hmonitor: isize,
}

/// Where a game window is, relative to its monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPlacement {
    pub monitor: MonitorInfo,
    /// Window rectangle relative to the monitor origin, clamped to it.
    pub rect: (i32, i32, u32, u32),
    /// The window covers the whole monitor.
    pub fullscreen: bool,
}

/// Clamps a window rectangle (screen coordinates) to a monitor and returns
/// it relative to the monitor origin, with even dimensions.
pub fn clamp_to_monitor(
    win: (i32, i32, i32, i32),
    m: &MonitorInfo,
) -> Option<(i32, i32, u32, u32)> {
    let (l, t, r, b) = win;
    let ml = m.x;
    let mt = m.y;
    let mr = m.x + m.width as i32;
    let mb = m.y + m.height as i32;
    let (l, t, r, b) = (l.max(ml), t.max(mt), r.min(mr), b.min(mb));
    if r - l < 16 || b - t < 16 {
        return None;
    }
    Some((l - ml, t - mt, ((r - l) as u32) & !1, ((b - t) as u32) & !1))
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub fn list_monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn window_placement(_hwnd: isize) -> Option<WindowPlacement> {
    None
}

#[cfg(not(windows))]
pub fn window_monitor_rect(_hwnd: isize) -> Option<(i32, i32, u32, u32)> {
    None
}

#[cfg(not(windows))]
pub fn window_client_size(_hwnd: isize) -> Option<(u32, u32)> {
    None
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::wstr::from_wide;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

    /// Size of a window's client area (what Windows Graphics Capture
    /// records, without borders and title bar).
    pub fn window_client_size(hwnd: isize) -> Option<(u32, u32)> {
        let mut r = RECT::default();
        unsafe { GetClientRect(HWND(hwnd as *mut _), &mut r).ok()? };
        let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        (w >= 16 && h >= 16).then_some((w, h))
    }

    pub fn list_monitors() -> Vec<MonitorInfo> {
        let mut out = Vec::new();
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
                return out;
            };
            let mut global = 0;
            let mut a = 0;
            while let Ok(adapter) = factory.EnumAdapters1(a) {
                let adapter_name = adapter
                    .GetDesc1()
                    .map(|d| from_wide(&d.Description))
                    .unwrap_or_default();
                let mut o = 0;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    if let Ok(d) = output.GetDesc() {
                        let r = d.DesktopCoordinates;
                        out.push(MonitorInfo {
                            adapter_idx: a,
                            output_idx: o,
                            global_idx: global,
                            device_name: from_wide(&d.DeviceName),
                            adapter_name: adapter_name.clone(),
                            x: r.left,
                            y: r.top,
                            width: (r.right - r.left) as u32,
                            height: (r.bottom - r.top) as u32,
                            primary: r.left == 0 && r.top == 0,
                            hmonitor: d.Monitor.0 as isize,
                        });
                        global += 1;
                    }
                    o += 1;
                }
                a += 1;
            }
        }
        out
    }

    /// The window's rectangle relative to its monitor, as in
    /// [`window_placement`], from user32 alone. Cheap enough to poll while
    /// capturing: creating a DXGI factory and enumerating outputs (as
    /// `list_monitors` does) stalls Desktop Duplication for 100-250 ms on
    /// some drivers, which shows up as a hitch in the recording.
    pub fn window_monitor_rect(hwnd: isize) -> Option<(i32, i32, u32, u32)> {
        unsafe {
            let hwnd = HWND(hwnd as *mut _);
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(mon, &mut mi).ok().ok()?;
            let m = mi.rcMonitor;
            let monitor = MonitorInfo {
                adapter_idx: 0,
                output_idx: 0,
                global_idx: 0,
                device_name: String::new(),
                adapter_name: String::new(),
                x: m.left,
                y: m.top,
                width: (m.right - m.left) as u32,
                height: (m.bottom - m.top) as u32,
                primary: false,
                hmonitor: mon.0 as isize,
            };
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).ok()?;
            clamp_to_monitor((r.left, r.top, r.right, r.bottom), &monitor)
        }
    }

    pub fn window_placement(hwnd: isize) -> Option<WindowPlacement> {
        unsafe {
            let hwnd = HWND(hwnd as *mut _);
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let monitor = list_monitors()
                .into_iter()
                .find(|m| m.hmonitor == mon.0 as isize)?;
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).ok()?;
            let rect = clamp_to_monitor((r.left, r.top, r.right, r.bottom), &monitor)?;
            let fullscreen = rect.0 == 0
                && rect.1 == 0
                && rect.2 >= monitor.width & !1
                && rect.3 >= monitor.height & !1;
            Some(WindowPlacement {
                monitor,
                rect,
                fullscreen,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_window_rects() {
        let m = MonitorInfo {
            adapter_idx: 0,
            output_idx: 1,
            global_idx: 1,
            device_name: String::new(),
            adapter_name: String::new(),
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            primary: false,
            hmonitor: 0,
        };
        assert_eq!(
            clamp_to_monitor((2000, 100, 3281, 821), &m),
            Some((80, 100, 1280, 720))
        );
        assert_eq!(
            clamp_to_monitor((1900, -8, 4500, 1450), &m),
            Some((0, 0, 2560, 1440))
        );
        assert_eq!(clamp_to_monitor((0, 0, 100, 100), &m), None);
    }
}
