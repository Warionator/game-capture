//! Game detection heuristics.
//!
//! `gc-platform` gathers facts about each running process (path, loaded
//! graphics modules, whether it owns a fullscreen foreground window…) and
//! this module decides whether it is a game. Keeping the decision pure makes
//! it testable and easy to tune.

use crate::config::GameSettings;
use serde::{Deserialize, Serialize};

/// Facts collected about one process.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessFacts {
    pub pid: u32,
    pub exe_name: String,
    pub exe_path: Option<String>,
    /// File description / product name / package display name.
    pub display_name: Option<String>,
    /// Graphics API DLLs loaded (`d3d11.dll`, `d3d12.dll`, `vulkan-1.dll`…).
    /// `None` when the module list could not be read (protected process).
    pub graphics_modules: Option<Vec<String>>,
    /// Owns the foreground window.
    pub is_foreground: bool,
    /// Foreground window covers its whole monitor (exclusive or borderless).
    pub is_fullscreen: bool,
    /// Packaged (Microsoft Store / Game Pass) application.
    pub is_packaged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// User-registered game.
    Custom,
    /// Detected automatically.
    Detected,
    NotAGame,
}

/// Executables that render with D3D/Vulkan but are not games.
const NOT_GAMES: &[&str] = &[
    "explorer.exe",
    "dwm.exe",
    "searchhost.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
    "applicationframehost.exe",
    "systemsettings.exe",
    "taskmgr.exe",
    "chrome.exe",
    "msedge.exe",
    "msedgewebview2.exe",
    "firefox.exe",
    "opera.exe",
    "brave.exe",
    "vivaldi.exe",
    "discord.exe",
    "slack.exe",
    "teams.exe",
    "ms-teams.exe",
    "zoom.exe",
    "spotify.exe",
    "code.exe",
    "devenv.exe",
    "obs64.exe",
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "battle.net.exe",
    "eadesktop.exe",
    "galaxyclient.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "xboxpcapp.exe",
    "gamebar.exe",
    "nvidia app.exe",
    "nvcontainer.exe",
    "radeonsoftware.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "photoshop.exe",
    "blender.exe",
    "unity.exe",
    "unrealeditor.exe",
    "game-capture.exe",
    "ffmpeg.exe",
];

/// Path fragments that strongly suggest a game install.
const GAME_PATHS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\xboxgames\\",
    "\\riot games\\",
    "\\gog galaxy\\games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\battle.net\\",
    "\\origin games\\",
    "\\games\\",
];

const GRAPHICS_DLLS: &[&str] = &[
    "d3d9.dll",
    "d3d11.dll",
    "d3d12.dll",
    "vulkan-1.dll",
    "opengl32.dll",
];

pub fn is_graphics_module(name: &str) -> bool {
    GRAPHICS_DLLS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

fn path_looks_like_game(path: &str) -> bool {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    GAME_PATHS.iter().any(|g| p.contains(g))
}

pub fn classify(facts: &ProcessFacts, cfg: &GameSettings) -> Verdict {
    let exe = facts.exe_name.to_ascii_lowercase();
    if cfg.custom.iter().any(|g| {
        g.executable.eq_ignore_ascii_case(&facts.exe_name)
            && match (&g.path, &facts.exe_path) {
                (Some(want), Some(have)) => want.to_string_lossy().eq_ignore_ascii_case(have),
                _ => true,
            }
    }) {
        return Verdict::Custom;
    }
    if !cfg.auto_detect
        || cfg.excluded.iter().any(|e| e.eq_ignore_ascii_case(&exe))
        || NOT_GAMES.contains(&exe.as_str())
    {
        return Verdict::NotAGame;
    }
    if let Some(path) = &facts.exe_path {
        let p = path.to_ascii_lowercase();
        if p.contains("\\windows\\system32\\") || p.contains("\\windows\\systemapps\\") {
            return Verdict::NotAGame;
        }
    }

    let in_game_dir = facts.exe_path.as_deref().is_some_and(path_looks_like_game);
    let renders = facts
        .graphics_modules
        .as_ref()
        .map(|m| m.iter().any(|d| is_graphics_module(d)));
    let fullscreen_fg = facts.is_foreground && facts.is_fullscreen;

    let score = match renders {
        // Module list readable: require a graphics API.
        Some(true) => {
            if fullscreen_fg || in_game_dir {
                2
            } else {
                0
            }
        }
        Some(false) => 0,
        // Unreadable (often anti-cheat protected) – rely on other signals.
        None => {
            u8::from(in_game_dir)
                + u8::from(fullscreen_fg)
                + u8::from(facts.is_packaged && fullscreen_fg)
        }
    };
    if score >= 2 {
        Verdict::Detected
    } else {
        Verdict::NotAGame
    }
}

/// Human-friendly name for a process: explicit custom name, version-info
/// name, the install folder under a launcher library, or the exe stem.
pub fn display_name(facts: &ProcessFacts, cfg: &GameSettings) -> String {
    if let Some(g) = cfg.custom.iter().find(|g| {
        g.executable.eq_ignore_ascii_case(&facts.exe_name) && !g.display_name.trim().is_empty()
    }) {
        return g.display_name.clone();
    }
    if let Some(folder) = facts.exe_path.as_deref().and_then(launcher_folder_name) {
        return folder;
    }
    if let Some(n) = facts
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        // Generic engine descriptions are worse than the exe name.
        let generic = [
            "unreal",
            "unity",
            "game",
            "launcher",
            "bootstrapper",
            "shipping",
        ];
        if !generic.iter().any(|g| n.eq_ignore_ascii_case(g)) && n.len() > 2 {
            return n.to_string();
        }
    }
    let stem = facts
        .exe_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&facts.exe_name);
    // UE builds are named like "FortniteClient-Win64-Shipping".
    stem.split("-Win64").next().unwrap_or(stem).to_string()
}

/// `...\steamapps\common\Elden Ring\Game\eldenring.exe` → `Elden Ring`.
fn launcher_folder_name(path: &str) -> Option<String> {
    let norm = path.replace('/', "\\");
    let lower = norm.to_ascii_lowercase();
    for marker in [
        "\\steamapps\\common\\",
        "\\epic games\\",
        "\\xboxgames\\",
        "\\riot games\\",
    ] {
        if let Some(i) = lower.find(marker) {
            let rest = &norm[i + marker.len()..];
            let name = rest.split('\\').next()?.trim();
            if !name.is_empty() && !name.contains('.') {
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomGame;

    fn facts(exe: &str, path: &str) -> ProcessFacts {
        ProcessFacts {
            pid: 1,
            exe_name: exe.into(),
            exe_path: Some(path.into()),
            graphics_modules: Some(vec!["d3d12.dll".into()]),
            is_foreground: true,
            is_fullscreen: true,
            ..Default::default()
        }
    }

    #[test]
    fn detects_fullscreen_renderer() {
        let cfg = GameSettings::default();
        let f = facts(
            "eldenring.exe",
            r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe",
        );
        assert_eq!(classify(&f, &cfg), Verdict::Detected);
        assert_eq!(display_name(&f, &cfg), "ELDEN RING");
    }

    #[test]
    fn ignores_browsers_and_windowed_apps() {
        let cfg = GameSettings::default();
        assert_eq!(
            classify(&facts("chrome.exe", r"C:\x\chrome.exe"), &cfg),
            Verdict::NotAGame
        );
        let mut f = facts("tool.exe", r"C:\Tools\tool.exe");
        f.is_fullscreen = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        f.graphics_modules = Some(vec![]);
        f.is_fullscreen = true;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn protected_processes_use_path_and_fullscreen() {
        let cfg = GameSettings::default();
        let mut f = facts(
            "FortniteClient-Win64-Shipping.exe",
            r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe",
        );
        f.graphics_modules = None;
        assert_eq!(classify(&f, &cfg), Verdict::Detected);
        assert_eq!(display_name(&f, &cfg), "Fortnite");
        f.is_foreground = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn custom_and_excluded_override() {
        let mut cfg = GameSettings::default();
        cfg.custom.push(CustomGame {
            display_name: "My Emulator".into(),
            executable: "EMU.exe".into(),
            path: None,
        });
        cfg.excluded.push("eldenring.exe".into());
        let mut f = facts("emu.exe", r"C:\emu\emu.exe");
        f.is_fullscreen = false;
        assert_eq!(classify(&f, &cfg), Verdict::Custom);
        assert_eq!(display_name(&f, &cfg), "My Emulator");
        let f = facts(
            "eldenring.exe",
            r"D:\steamapps\common\ELDEN RING\eldenring.exe",
        );
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn display_name_fallbacks() {
        let cfg = GameSettings::default();
        let mut f = facts(
            "Cyberpunk2077.exe",
            r"C:\GOG Galaxy\Games\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe",
        );
        f.display_name = Some("Cyberpunk 2077".into());
        assert_eq!(display_name(&f, &cfg), "Cyberpunk 2077");
        f.display_name = Some("Game".into());
        assert_eq!(display_name(&f, &cfg), "Cyberpunk2077");
    }
}
