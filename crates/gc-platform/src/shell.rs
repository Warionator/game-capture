//! Explorer / shell helpers used by the gallery context menu.

use std::path::Path;

/// Opens Explorer with `path` selected.
pub fn reveal_in_explorer(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `/select,"<path>"` must reach explorer verbatim; std's quoting
        // would wrap the whole argument in quotes and break it.
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let dir = path.parent().unwrap_or(path);
        std::process::Command::new("xdg-open").arg(dir).spawn()?;
        Ok(())
    }
}

/// Opens a file with its default application (external player).
pub fn open_default(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    unsafe {
        use crate::wstr::hstring;
        use windows::core::w;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let file = hstring(&path.to_string_lossy());
        let r = ShellExecuteW(None, w!("open"), &file, None, None, SW_SHOWNORMAL);
        // Values <= 32 are errors.
        anyhow::ensure!(r.0 as isize > 32, "ShellExecuteW failed ({})", r.0 as isize);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
        Ok(())
    }
}

/// Builds the `CF_HDROP` payload: a `DROPFILES` header followed by a
/// double-NUL-terminated list of wide paths.
pub fn hdrop_payload(paths: &[&Path]) -> Vec<u8> {
    const HEADER: u32 = 20; // sizeof(DROPFILES)
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for p in paths {
        for u in p.to_string_lossy().encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Puts files on the clipboard so they can be pasted into Explorer, Discord…
pub fn copy_files_to_clipboard(paths: &[&Path]) -> anyhow::Result<()> {
    #[cfg(windows)]
    unsafe {
        use windows::core::w;
        use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
            SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
        };
        use windows::Win32::System::Ole::CF_HDROP;

        unsafe fn global(bytes: &[u8]) -> anyhow::Result<HGLOBAL> {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
            let p = GlobalLock(h);
            if p.is_null() {
                let _ = GlobalFree(Some(h));
                anyhow::bail!("GlobalLock failed");
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(h);
            Ok(h)
        }

        let drop_files = global(&hdrop_payload(paths))?;
        // DROPEFFECT_COPY so Explorer copies rather than moves on paste.
        let effect = global(&1u32.to_le_bytes())?;
        OpenClipboard(None)?;
        let result = (|| -> anyhow::Result<()> {
            EmptyClipboard()?;
            SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(drop_files.0)))?;
            let fmt = RegisterClipboardFormatW(w!("Preferred DropEffect"));
            if fmt != 0 {
                SetClipboardData(fmt, Some(HANDLE(effect.0)))?;
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        anyhow::bail!("copying files to the clipboard is only supported on Windows")
    }
}

/// Permanently deletes a library file (a missing file is not an error).
pub fn delete_file(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// Sets or clears the Windows "hidden" attribute (no-op elsewhere, where the
/// leading dot already hides the file).
pub fn set_hidden(path: &Path, hidden: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    unsafe {
        use crate::wstr::hstring;
        use windows::Win32::Storage::FileSystem::{
            GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN,
            FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES,
        };
        let p = hstring(&path.to_string_lossy());
        let cur = GetFileAttributesW(&p);
        anyhow::ensure!(
            cur != INVALID_FILE_ATTRIBUTES,
            "cannot read attributes of {}",
            path.display()
        );
        let new = if hidden {
            cur | FILE_ATTRIBUTE_HIDDEN.0
        } else {
            cur & !FILE_ATTRIBUTE_HIDDEN.0
        };
        if new != cur {
            SetFileAttributesW(&p, FILE_FLAGS_AND_ATTRIBUTES(new))?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, hidden);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hdrop_layout() {
        let p =
            super::hdrop_payload(&[std::path::Path::new("C:\\a.mp4"), std::path::Path::new("b")]);
        assert_eq!(&p[0..4], &20u32.to_le_bytes());
        assert_eq!(&p[16..20], &1u32.to_le_bytes());
        // "C:\a.mp4\0" (9 u16) + "b\0" (2) + final "\0" (1) = 12 u16
        assert_eq!(p.len(), 20 + 12 * 2);
        assert_eq!(&p[p.len() - 4..], &[0, 0, 0, 0]);
    }
}
