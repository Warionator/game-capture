//! Windows settings integration: Game Mode toggle, autostart (`Run` key)
//! and OS capability checks.

pub const APP_RUN_VALUE: &str = "game-capture";
const GAMEBAR_KEY: &str = r"Software\Microsoft\GameBar";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Process loopback capture (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`)
/// is available from Windows 10 2004 (build 19041).
pub const PROCESS_LOOPBACK_MIN_BUILD: u32 = 19041;

/// Command line registered for autostart.
pub fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\" --minimized", exe.display())
}

#[cfg(windows)]
pub(crate) mod reg {
    use crate::wstr::{from_wide, wide};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteKeyValueW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
        RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD,
        RRF_RT_REG_SZ,
    };

    /// Names of the direct subkeys of `root\key` (empty if it can't be opened).
    pub fn subkeys(root: HKEY, key: &str) -> Vec<String> {
        let k = wide(key);
        let mut h = HKEY::default();
        let mut out = Vec::new();
        unsafe {
            if RegOpenKeyExW(root, PCWSTR(k.as_ptr()), None, KEY_READ, &mut h) != ERROR_SUCCESS {
                return out;
            }
            let mut i = 0u32;
            loop {
                // Key names are limited to 255 characters.
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                let rc = RegEnumKeyExW(
                    h,
                    i,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                );
                if rc != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&name[..len as usize]));
                i += 1;
            }
            let _ = RegCloseKey(h);
        }
        out
    }

    pub fn get_dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
        let (k, n) = (wide(key), wide(name));
        let mut v = 0u32;
        let mut len = 4u32;
        let rc = unsafe {
            RegGetValueW(
                root,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut v as *mut u32).cast()),
                Some(&mut len),
            )
        };
        (rc == ERROR_SUCCESS).then_some(v)
    }

    pub fn get_string(root: HKEY, key: &str, name: &str) -> Option<String> {
        let (k, n) = (wide(key), wide(name));
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                root,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        (rc == ERROR_SUCCESS).then(|| from_wide(&buf))
    }

    pub fn set_dword(key: &str, name: &str, value: u32) -> anyhow::Result<()> {
        let (k, n) = (wide(key), wide(name));
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                REG_DWORD.0,
                Some((&value as *const u32).cast()),
                4,
            )
        };
        anyhow::ensure!(
            rc == ERROR_SUCCESS,
            "RegSetKeyValueW({key}\\{name}) failed: {rc:?}"
        );
        Ok(())
    }

    pub fn set_string(key: &str, name: &str, value: &str) -> anyhow::Result<()> {
        let (k, n, v) = (wide(key), wide(name), wide(value));
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(k.as_ptr()),
                PCWSTR(n.as_ptr()),
                REG_SZ.0,
                Some(v.as_ptr().cast()),
                (v.len() * 2) as u32,
            )
        };
        anyhow::ensure!(
            rc == ERROR_SUCCESS,
            "RegSetKeyValueW({key}\\{name}) failed: {rc:?}"
        );
        Ok(())
    }

    pub fn delete(key: &str, name: &str) -> anyhow::Result<()> {
        let (k, n) = (wide(key), wide(name));
        let rc = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()))
        };
        anyhow::ensure!(
            rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND,
            "RegDeleteKeyValueW({key}\\{name}) failed: {rc:?}"
        );
        Ok(())
    }
}

/// `Some(true)` if Windows Game Mode is enabled, `None` if unknown.
pub fn game_mode_enabled() -> Option<bool> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        // Newer builds use AutoGameModeEnabled; older ones AllowAutoGameMode.
        // Both default to enabled when absent.
        let a = reg::get_dword(HKEY_CURRENT_USER, GAMEBAR_KEY, "AutoGameModeEnabled");
        let b = reg::get_dword(HKEY_CURRENT_USER, GAMEBAR_KEY, "AllowAutoGameMode");
        Some(a.or(b).map(|v| v != 0).unwrap_or(true))
    }
    #[cfg(not(windows))]
    {
        let _ = GAMEBAR_KEY;
        None
    }
}

/// Enables or disables Windows Game Mode for the current user.
pub fn set_game_mode(enabled: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let v = u32::from(enabled);
        reg::set_dword(GAMEBAR_KEY, "AllowAutoGameMode", v)?;
        reg::set_dword(GAMEBAR_KEY, "AutoGameModeEnabled", v)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}

pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        reg::get_string(HKEY_CURRENT_USER, RUN_KEY, APP_RUN_VALUE).is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = RUN_KEY;
        false
    }
}

/// Adds/removes `"<exe>" --minimized` under `HKCU\...\CurrentVersion\Run`.
pub fn set_autostart(enabled: bool, exe: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        if enabled {
            reg::set_string(RUN_KEY, APP_RUN_VALUE, &autostart_command(exe))
        } else {
            reg::delete(RUN_KEY, APP_RUN_VALUE)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, exe);
        Ok(())
    }
}

/// Windows build number (e.g. 22631), `None` off Windows.
pub fn windows_build() -> Option<u32> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
        reg::get_string(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuildNumber",
        )
        .and_then(|s| s.trim().parse().ok())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Executables Windows itself has recognised as games, lower-cased full
/// paths. Game Bar matches launched programs against Microsoft's catalogue
/// of known games (and games the user ticked "Remember this is a game" for)
/// and records them under `HKCU\System\GameConfigStore\Children`.
pub fn windows_known_games() -> std::collections::HashSet<String> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        const KEY: &str = r"System\GameConfigStore\Children";
        reg::subkeys(HKEY_CURRENT_USER, KEY)
            .into_iter()
            .filter_map(|id| {
                reg::get_string(
                    HKEY_CURRENT_USER,
                    &format!(r"{KEY}\{id}"),
                    "MatchedExeFullPath",
                )
            })
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect()
    }
    #[cfg(not(windows))]
    {
        std::collections::HashSet::new()
    }
}

pub fn supports_process_loopback() -> bool {
    windows_build().is_some_and(|b| b >= PROCESS_LOOPBACK_MIN_BUILD)
}

#[cfg(test)]
mod tests {
    #[test]
    fn autostart_command_quotes_path() {
        let cmd = super::autostart_command(std::path::Path::new(
            r"C:\Program Files\game-capture\game-capture.exe",
        ));
        assert_eq!(
            cmd,
            r#""C:\Program Files\game-capture\game-capture.exe" --minimized"#
        );
    }
}
