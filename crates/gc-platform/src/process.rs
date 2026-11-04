//! Process enumeration and metadata: executable path, version-info display
//! name, Microsoft Store package display name, loaded graphics modules,
//! foreground/fullscreen state and icon extraction (→ PNG).

use gc_core::detect::ProcessFacts;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub exe_name: String,
}

/// A process that owns a visible top-level window (for the manual
/// "pick a running game" UI).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowedProcess {
    pub pid: u32,
    pub exe_name: String,
    pub exe_path: Option<String>,
    pub window_title: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundWindow {
    pub pid: u32,
    pub hwnd: isize,
    pub fullscreen: bool,
}

/// PIDs of processes whose executable matches one of `names`
/// (case-insensitive). Only *root* instances are returned — a match whose
/// parent is also a match is skipped, because process-loopback capture
/// already includes the whole process tree.
pub fn root_pids_by_exe(procs: &[ProcessEntry], names: &[String]) -> Vec<u32> {
    let matches = |p: &ProcessEntry| names.iter().any(|n| n.eq_ignore_ascii_case(&p.exe_name));
    let matched: Vec<&ProcessEntry> = procs.iter().filter(|p| matches(p)).collect();
    matched
        .iter()
        .filter(|p| {
            !matched
                .iter()
                .any(|q| q.pid == p.parent_pid && q.pid != p.pid)
        })
        .map(|p| p.pid)
        .collect()
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod stub {
    use super::*;
    pub fn snapshot() -> Vec<ProcessEntry> {
        Vec::new()
    }
    pub fn exe_path(_pid: u32) -> Option<String> {
        None
    }
    pub fn foreground_window() -> Option<ForegroundWindow> {
        None
    }
    pub fn gather_facts(pid: u32, exe_name: &str, _fg: Option<ForegroundWindow>) -> ProcessFacts {
        ProcessFacts {
            pid,
            exe_name: exe_name.to_string(),
            ..Default::default()
        }
    }
    pub fn version_display_name(_path: &str) -> Option<String> {
        None
    }
    pub fn icon_png(_path: &str, _size: u32) -> Option<Vec<u8>> {
        None
    }
    pub fn windowed_processes() -> Vec<WindowedProcess> {
        Vec::new()
    }
    pub fn main_window(_pid: u32) -> Option<isize> {
        None
    }
}
#[cfg(not(windows))]
pub use stub::*;

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::wstr::{from_wide, hstring, wide};
    use gc_core::detect::is_graphics_module;
    use std::collections::HashMap;
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, HWND, LPARAM, RECT,
    };
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetMonitorInfoW, GetObjectW,
        MonitorFromWindow, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
        MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::Win32::Storage::Packaging::Appx::{GetPackageFullName, GetPackagePathByFullName};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
        MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Shell::{ExtractIconExW, SHDefExtractIconW};
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, EnumWindows, GetForegroundWindow, GetIconInfo, GetShellWindow, GetWindow,
        GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, GW_OWNER, HICON, ICONINFO,
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub fn snapshot() -> Vec<ProcessEntry> {
        let mut out = Vec::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let snap = Handle(snap);
            let mut e = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap.0, &mut e).is_ok() {
                loop {
                    out.push(ProcessEntry {
                        pid: e.th32ProcessID,
                        parent_pid: e.th32ParentProcessID,
                        exe_name: from_wide(&e.szExeFile),
                    });
                    if Process32NextW(snap.0, &mut e).is_err() {
                        break;
                    }
                }
            }
        }
        out
    }

    fn open_limited(pid: u32) -> Option<Handle> {
        unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .ok()
                .map(Handle)
        }
    }

    pub fn exe_path(pid: u32) -> Option<String> {
        let h = open_limited(pid)?;
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .ok()?;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// Graphics DLLs loaded in `pid`, or `None` if the module list is not
    /// readable (protected / anti-cheat processes, or 32↔64-bit edge cases).
    fn graphics_modules(pid: u32) -> Option<Vec<String>> {
        unsafe {
            let snap =
                CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid).ok()?;
            let snap = Handle(snap);
            let mut e = MODULEENTRY32W {
                dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = Vec::new();
            Module32FirstW(snap.0, &mut e).ok()?;
            loop {
                let name = from_wide(&e.szModule);
                if is_graphics_module(&name) {
                    found.push(name);
                }
                if Module32NextW(snap.0, &mut e).is_err() {
                    break;
                }
            }
            Some(found)
        }
    }

    fn window_pid(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        pid
    }

    fn is_fullscreen(hwnd: HWND) -> bool {
        unsafe {
            if hwnd == GetShellWindow() || IsIconic(hwnd).as_bool() {
                return false;
            }
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_err() {
                return false;
            }
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(mon, &mut mi).as_bool() {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }

    pub fn foreground_window() -> Option<ForegroundWindow> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            Some(ForegroundWindow {
                pid: window_pid(hwnd),
                hwnd: hwnd.0 as isize,
                fullscreen: is_fullscreen(hwnd),
            })
        }
    }

    /// Microsoft Store / Game Pass display name from the package manifest.
    fn package_display_name(pid: u32) -> Option<(bool, Option<String>)> {
        let h = open_limited(pid)?;
        unsafe {
            let mut len = 0u32;
            let rc = GetPackageFullName(h.0, &mut len, None);
            if rc != ERROR_INSUFFICIENT_BUFFER {
                return Some((false, None));
            }
            let mut name = vec![0u16; len as usize];
            if GetPackageFullName(h.0, &mut len, Some(PWSTR(name.as_mut_ptr()))).is_err() {
                return Some((true, None));
            }
            let mut plen = 0u32;
            let _ = GetPackagePathByFullName(PCWSTR(name.as_ptr()), &mut plen, None);
            let mut path = vec![0u16; plen as usize];
            if GetPackagePathByFullName(
                PCWSTR(name.as_ptr()),
                &mut plen,
                Some(PWSTR(path.as_mut_ptr())),
            )
            .is_err()
            {
                return Some((true, None));
            }
            let manifest = std::path::Path::new(&from_wide(&path)).join("AppxManifest.xml");
            let xml = std::fs::read_to_string(manifest).ok();
            Some((true, xml.as_deref().and_then(manifest_display_name)))
        }
    }

    pub fn version_display_name(path: &str) -> Option<String> {
        let wpath = hstring(path);
        unsafe {
            let size = GetFileVersionInfoSizeW(&wpath, None);
            if size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            GetFileVersionInfoW(&wpath, None, size, buf.as_mut_ptr().cast()).ok()?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            let translation = wide("\\VarFileInfo\\Translation");
            let mut langs: Vec<(u16, u16)> = Vec::new();
            if VerQueryValueW(
                buf.as_ptr().cast(),
                PCWSTR(translation.as_ptr()),
                &mut ptr,
                &mut len,
            )
            .as_bool()
                && len >= 4
            {
                let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
                langs.extend(pairs.chunks(2).map(|c| (c[0], c[1])));
            }
            langs.push((0x0409, 0x04B0)); // en-US, Unicode
            for key in ["FileDescription", "ProductName"] {
                for (lang, cp) in &langs {
                    let q = wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"));
                    let mut p = std::ptr::null_mut();
                    let mut l = 0u32;
                    if VerQueryValueW(buf.as_ptr().cast(), PCWSTR(q.as_ptr()), &mut p, &mut l)
                        .as_bool()
                        && l > 1
                    {
                        let s = from_wide(std::slice::from_raw_parts(p as *const u16, l as usize));
                        let s = s.trim().to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
            None
        }
    }

    pub fn gather_facts(pid: u32, exe_name: &str, fg: Option<ForegroundWindow>) -> ProcessFacts {
        let exe_path = exe_path(pid);
        let (is_packaged, pkg_name) = package_display_name(pid).unwrap_or((false, None));
        let display_name = pkg_name.or_else(|| exe_path.as_deref().and_then(version_display_name));
        let fg = fg.filter(|f| f.pid == pid);
        ProcessFacts {
            pid,
            exe_name: exe_name.to_string(),
            exe_path,
            display_name,
            graphics_modules: graphics_modules(pid),
            is_foreground: fg.is_some(),
            is_fullscreen: fg.is_some_and(|f| f.fullscreen),
            is_packaged,
        }
    }

    /// Converts an HICON into RGBA PNG bytes.
    unsafe fn hicon_to_png(icon: HICON) -> Option<Vec<u8>> {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let color = info.hbmColor;
        let mask = info.hbmMask;
        let cleanup = || {
            let _ = DeleteObject(HGDIOBJ(color.0));
            let _ = DeleteObject(HGDIOBJ(mask.0));
        };
        if color.0.is_null() {
            cleanup();
            return None;
        }
        let mut bmp = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(color.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut bmp as *mut BITMAP).cast()),
        ) == 0
        {
            cleanup();
            return None;
        }
        let (w, h) = (bmp.bmWidth, bmp.bmHeight);
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            color,
            0,
            h as u32,
            Some(px.as_mut_ptr().cast()),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        cleanup();
        if lines == 0 {
            return None;
        }
        let has_alpha = px.chunks(4).any(|p| p[3] != 0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2); // BGRA → RGBA
            if !has_alpha {
                p[3] = 255;
            }
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, px)?;
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }

    /// Extracts the executable's main icon at roughly `size` pixels.
    pub fn icon_png(path: &str, size: u32) -> Option<Vec<u8>> {
        let wpath = hstring(path);
        unsafe {
            let mut large = HICON::default();
            // SHDefExtractIconW can return sizes up to 256 px.
            let ok = SHDefExtractIconW(&wpath, 0, 0, Some(&mut large), None, size & 0xFFFF).is_ok();
            if !ok || large.is_invalid() {
                let mut l = HICON::default();
                if ExtractIconExW(&wpath, 0, Some(&mut l), None, 1) == 0 || l.is_invalid() {
                    return None;
                }
                large = l;
            }
            let png = hicon_to_png(large);
            let _ = DestroyIcon(large);
            png
        }
    }

    struct EnumState {
        windows: Vec<(HWND, u32)>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER)
                .map(|o| o.0.is_null())
                .unwrap_or(true)
            && GetWindowTextLengthW(hwnd) > 0
        {
            state.windows.push((hwnd, window_pid(hwnd)));
        }
        BOOL(1)
    }

    fn top_level_windows() -> Vec<(HWND, u32)> {
        let mut state = EnumState {
            windows: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(enum_proc),
                LPARAM(&mut state as *mut EnumState as isize),
            );
        }
        state.windows
    }

    fn window_title(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            String::from_utf16_lossy(&buf[..n as usize])
        }
    }

    /// Largest visible top-level window of a process.
    pub fn main_window(pid: u32) -> Option<isize> {
        top_level_windows()
            .into_iter()
            .filter(|(_, p)| *p == pid)
            .max_by_key(|(h, _)| unsafe {
                let mut r = RECT::default();
                let _ = GetWindowRect(*h, &mut r);
                ((r.right - r.left) as i64) * ((r.bottom - r.top) as i64)
            })
            .map(|(h, _)| h.0 as isize)
    }

    pub fn windowed_processes() -> Vec<WindowedProcess> {
        let names: HashMap<u32, String> = snapshot()
            .into_iter()
            .map(|p| (p.pid, p.exe_name))
            .collect();
        let own = std::process::id();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (hwnd, pid) in top_level_windows() {
            if pid == own || !seen.insert(pid) {
                continue;
            }
            let Some(exe_name) = names.get(&pid).cloned() else {
                continue;
            };
            if exe_name.eq_ignore_ascii_case("explorer.exe")
                || exe_name.eq_ignore_ascii_case("ApplicationFrameHost.exe")
            {
                continue;
            }
            let exe_path = exe_path(pid);
            out.push(WindowedProcess {
                pid,
                display_name: exe_path.as_deref().and_then(version_display_name),
                exe_name,
                exe_path,
                window_title: window_title(hwnd),
            });
        }
        out.sort_by_key(|p| p.exe_name.to_lowercase());
        out
    }
}

/// Extracts `<DisplayName>` from an AppxManifest (ignores `ms-resource:` refs).
#[cfg_attr(not(windows), allow(dead_code))]
fn manifest_display_name(xml: &str) -> Option<String> {
    let props = xml.find("<Properties>").map(|i| &xml[i..]).unwrap_or(xml);
    let start = props.find("<DisplayName>")? + "<DisplayName>".len();
    let end = props[start..].find("</DisplayName>")? + start;
    let name = props[start..end].trim();
    if name.is_empty() || name.starts_with("ms-resource:") {
        None
    } else {
        Some(
            name.replace("&amp;", "&")
                .replace("&apos;", "'")
                .replace("&quot;", "\"")
                .replace("&lt;", "<")
                .replace("&gt;", ">"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_pids_skip_children_of_same_exe() {
        let p = |pid, parent, exe: &str| ProcessEntry {
            pid,
            parent_pid: parent,
            exe_name: exe.into(),
        };
        let procs = vec![
            p(10, 1, "Discord.exe"),
            p(11, 10, "Discord.exe"),
            p(12, 10, "Discord.exe"),
            p(20, 1, "spotify.exe"),
            p(30, 1, "game.exe"),
        ];
        assert_eq!(root_pids_by_exe(&procs, &["discord.exe".into()]), [10]);
        assert_eq!(
            root_pids_by_exe(&procs, &["DISCORD.EXE".into(), "Spotify.exe".into()]),
            [10, 20]
        );
        assert!(root_pids_by_exe(&procs, &["x.exe".into()]).is_empty());
    }

    #[test]
    fn manifest_names() {
        let xml = r#"<Package><Properties><DisplayName>Forza Horizon 5 &amp; Co</DisplayName></Properties>
            <Applications><Application><uap:VisualElements DisplayName="x"/></Application></Applications></Package>"#;
        assert_eq!(
            manifest_display_name(xml).as_deref(),
            Some("Forza Horizon 5 & Co")
        );
        let xml = "<Properties><DisplayName>ms-resource:AppName</DisplayName></Properties>";
        assert_eq!(manifest_display_name(xml), None);
    }
}
