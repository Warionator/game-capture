//! Platform-independent pieces of the self-updater: how the app was
//! installed, and where a downloaded update package is kept.

use std::path::{Path, PathBuf};

/// How this copy of game-capture was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallKind {
    /// Installed by the NSIS installer: updates are downloaded and applied.
    Installer,
    /// Extracted from the portable zip (or a dev build): updates are only
    /// announced, the user downloads them.
    Portable,
}

/// The NSIS installer writes `uninstall.exe` next to the app; the portable
/// zip never contains one.
pub fn detect_install_kind(exe_dir: &Path) -> InstallKind {
    if exe_dir.join("uninstall.exe").is_file() {
        InstallKind::Installer
    } else {
        InstallKind::Portable
    }
}

/// File name a downloaded update package is stored under.
pub fn package_file_name(version: &str) -> String {
    format!("game-capture_{version}_x64-setup.exe")
}

/// Cached update packages in `dir` other than `keep` (older or superseded
/// downloads, partial files), to be deleted.
pub fn stale_packages(dir: &Path, keep: Option<&str>) -> Vec<PathBuf> {
    let keep = keep.map(package_file_name);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            keep.as_deref() != Some(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_kind_follows_uninstaller() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_install_kind(dir.path()), InstallKind::Portable);
        std::fs::write(dir.path().join("uninstall.exe"), b"").unwrap();
        assert_eq!(detect_install_kind(dir.path()), InstallKind::Installer);
    }

    #[test]
    fn stale_packages_keep_only_current() {
        let dir = tempfile::tempdir().unwrap();
        for v in ["1.1.0", "1.1.1"] {
            std::fs::write(dir.path().join(package_file_name(v)), b"x").unwrap();
        }
        std::fs::write(dir.path().join("download.part"), b"x").unwrap();
        let mut stale: Vec<String> = stale_packages(dir.path(), Some("1.1.1"))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        stale.sort();
        assert_eq!(stale, ["download.part", "game-capture_1.1.0_x64-setup.exe"]);
        assert_eq!(stale_packages(dir.path(), None).len(), 3);
        assert!(stale_packages(&dir.path().join("missing"), None).is_empty());
    }
}
