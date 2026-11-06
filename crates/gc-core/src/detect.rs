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
    /// Loaded modules that matter for detection: graphics APIs
    /// (`d3d11.dll`, `vulkan-1.dll`…) and game engines/SDKs
    /// (`UnityPlayer.dll`, `steam_api64.dll`…), see [`is_tracked_module`].
    /// `None` when the module list could not be read (protected process).
    pub modules: Option<Vec<String>>,
    /// Windows (Game Bar) has recognised this executable as a game.
    pub known_game: bool,
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
    "ffplay.exe",
    // Capture, streaming and screenshot tools: they render with D3D and
    // often show fullscreen overlays (region pickers), but are never games.
    "sharex.exe",
    "obs32.exe",
    "streamlabs obs.exe",
    "streamlabs desktop.exe",
    "xsplit.core.exe",
    "xsplitbroadcaster.exe",
    "medal.exe",
    "medalencoder.exe",
    "nvidia share.exe",
    "nvsphelper64.exe",
    "nvidia overlay.exe",
    "gamebarftserver.exe",
    "amdow.exe",
    "amdrsserv.exe",
    "action.exe",
    "bandicam.exe",
    "bdcam.exe",
    "fraps.exe",
    "dxtory.exe",
    "camtasia.exe",
    "camrecorder.exe",
    "snagit32.exe",
    "snagiteditor.exe",
    "lightshot.exe",
    "greenshot.exe",
    "flameshot.exe",
    "picpick.exe",
    "screenclippinghost.exe",
    "snippingtool.exe",
    "screensketch.exe",
    "loom.exe",
    "overwolf.exe",
    "outplayed.exe",
    "steelseriesgg.exe",
    "steelseriesengine.exe",
    "rtss.exe",
    "msiafterburner.exe",
    // Video players and streaming apps: fullscreen D3D video looks a lot
    // like a game.
    "stremio.exe",
    "stremio-shell-ng.exe",
    "stremio-runtime.exe",
    "mpv.exe",
    "mpc-hc.exe",
    "mpc-be64.exe",
    "mpc-be.exe",
    "potplayer.exe",
    "potplayer64.exe",
    "potplayermini.exe",
    "potplayermini64.exe",
    "kmplayer.exe",
    "kmplayer64x.exe",
    "wmplayer.exe",
    "microsoft.media.player.exe",
    "video.ui.exe",
    "kodi.exe",
    "plex.exe",
    "plex htpc.exe",
    "jellyfin media player.exe",
    "jellyfinmediaplayer.exe",
    "smplayer.exe",
    "netflix.exe",
    // Video editors.
    "resolve.exe",
    "adobe premiere pro.exe",
    "afterfx.exe",
    "capcut.exe",
    "vegas.exe",
    "vegas200.exe",
    "vegas210.exe",
    "vegas220.exe",
    "handbrake.exe",
    "shotcut.exe",
    "kdenlive.exe",
    "clipchamp.exe",
    // Engine editors and creative tools (several are sold on Steam, so they
    // sit in `steamapps\common` like games do).
    "ue4editor.exe",
    "unity hub.exe",
    "gamemaker.exe",
    "rpgmv.exe",
    "rpgmz.exe",
    "aseprite.exe",
    "krita.exe",
    "substance painter.exe",
    "adobe substance 3d painter.exe",
    "wallpaper32.exe",
    "wallpaper64.exe",
    "lossless scaling.exe",
    "losslessscaling.exe",
    "vrserver.exe",
    "vrmonitor.exe",
    "vrcompositor.exe",
    "vrdashboard.exe",
    "soundpad.exe",
    // Launchers that Windows sometimes lists as games.
    "leagueclient.exe",
    "leagueclientux.exe",
];

/// Executable name prefixes that are never games. Godot ships its editor
/// (which also runs projects) as `godot*.exe`, e.g. the Steam build's
/// `godot.windows.opt.tools.64.exe`; exported games are named after the game.
const NOT_GAME_PREFIXES: &[&str] = &["godot", "unrealeditor"];

/// Words in a program's description that mark it as a capture/screenshot
/// tool rather than a game.
const CAPTURE_TOOL_WORDS: &[&str] = &[
    "screen capture",
    "screen recorder",
    "screen recording",
    "screenshot",
    "screen shot",
    "snipping",
    "game recorder",
    "gameplay recorder",
    "broadcaster",
    "streaming software",
    "clip recorder",
];

/// Whether a program describes itself as capture/streaming software.
pub fn is_capture_tool(display_name: Option<&str>) -> bool {
    display_name.is_some_and(|n| {
        let n = n.to_ascii_lowercase();
        CAPTURE_TOOL_WORDS.iter().any(|w| n.contains(w))
    })
}

/// Game launcher library folders: anything installed here is (almost always)
/// a game.
const LAUNCHER_PATHS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\xboxgames\\",
    "\\riot games\\",
    "\\gog galaxy\\games\\",
    "\\gog games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\origin games\\",
];

const GRAPHICS_DLLS: &[&str] = &[
    "d3d9.dll",
    "d3d11.dll",
    "d3d12.dll",
    "vulkan-1.dll",
    "opengl32.dll",
];

/// Modules only games (and emulators) load: engine runtimes, store SDKs,
/// anti-cheat, game middleware and upscalers. Browsers, video players and
/// editors load the graphics APIs above but none of these.
const GAME_DLLS: &[&str] = &[
    // Engines
    "unityplayer.dll",
    "gameassembly.dll",
    // Store / platform SDKs
    "steam_api.dll",
    "steam_api64.dll",
    "eossdk-win32-shipping.dll",
    "eossdk-win64-shipping.dll",
    "galaxy.dll",
    "galaxy64.dll",
    "discord_game_sdk.dll",
    // Anti-cheat
    "easyanticheat.dll",
    "easyanticheat_x64.dll",
    "easyanticheat_eos.dll",
    "beclient.dll",
    "beclient_x64.dll",
    // Middleware
    "fmod.dll",
    "fmod64.dll",
    "fmodstudio.dll",
    "fmodstudio64.dll",
    "binkw32.dll",
    "binkw64.dll",
    "bink2w32.dll",
    "bink2w64.dll",
    "physx3_x64.dll",
    "physx3_x86.dll",
    "physx_64.dll",
    "physxcommon_64.dll",
    "sdl2.dll",
    "sdl3.dll",
    // Upscalers / frame generation
    "nvngx_dlss.dll",
    "nvngx_dlssg.dll",
    "sl.interposer.dll",
    "amd_fidelityfx_dx12.dll",
    "amd_fidelityfx_vk.dll",
    "libxess.dll",
];

/// Chromium-based browsers that host installed web apps (PWAs), with the
/// name shown to users.
const BROWSERS: &[(&str, &str)] = &[
    ("msedge.exe", "Microsoft Edge"),
    ("chrome.exe", "Google Chrome"),
    ("brave.exe", "Brave"),
    ("vivaldi.exe", "Vivaldi"),
    ("opera.exe", "Opera"),
    ("chromium.exe", "Chromium"),
];

/// Display name of a PWA-capable browser executable.
pub fn browser_name(exe: &str) -> Option<&'static str> {
    BROWSERS
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(exe))
        .map(|(_, n)| *n)
}

/// Extracts the web-app id from a Chromium app window's AppUserModelID,
/// e.g. `MSEdge._crx_cinhimbnkkaeohfgghhklpknlkffjgod` →
/// `cinhimbnkkaeohfgghhklpknlkffjgod`.
pub fn web_app_id(app_user_model_id: &str) -> Option<String> {
    let i = app_user_model_id.find("_crx_")?;
    let id: String = app_user_model_id[i + 5..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (id.len() >= 16).then_some(id)
}

/// Cleans a web app window title such as `Song • Artist - YouTube Music`
/// down to the app name when the relaunch name is unavailable.
pub fn web_app_name_from_title(title: &str) -> String {
    let t = title.trim();
    t.rsplit_once(" - ")
        .map(|(_, app)| app.trim())
        .filter(|a| !a.is_empty())
        .unwrap_or(t)
        .to_string()
}

pub fn is_graphics_module(name: &str) -> bool {
    GRAPHICS_DLLS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

pub fn is_game_module(name: &str) -> bool {
    GAME_DLLS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Modules worth recording in [`ProcessFacts::modules`].
pub fn is_tracked_module(name: &str) -> bool {
    is_graphics_module(name) || is_game_module(name)
}

fn normalized_path(path: &str) -> String {
    path.to_ascii_lowercase().replace('/', "\\")
}

fn in_launcher_library(path: &str) -> bool {
    let p = normalized_path(path);
    LAUNCHER_PATHS.iter().any(|g| p.contains(g))
}

fn is_known_non_game(exe_lower: &str) -> bool {
    NOT_GAMES.contains(&exe_lower) || NOT_GAME_PREFIXES.iter().any(|p| exe_lower.starts_with(p))
}

/// Decides whether a process is a game.
///
/// Rendering with D3D/Vulkan/OpenGL in a fullscreen window is *not* enough
/// on its own: browsers, video players and editors all do that. An
/// auto-detected game needs positive evidence:
///
/// * **Strong** – Windows lists it as a game, or it is installed in a game
///   launcher's library. Enough while it is in the foreground.
/// * **Weak** – it loads a game engine/SDK/anti-cheat module, is an Unreal
///   `-Shipping` build, lives under a `Games` folder, or is a packaged app
///   whose modules can't be read. Needs a fullscreen foreground window too.
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
        || is_known_non_game(&exe)
        || is_capture_tool(facts.display_name.as_deref())
    {
        return Verdict::NotAGame;
    }
    let path = facts.exe_path.as_deref().map(normalized_path);
    if path.as_deref().is_some_and(|p| {
        p.contains("\\windows\\system32\\") || p.contains("\\windows\\systemapps\\")
    }) {
        return Verdict::NotAGame;
    }
    if !facts.is_foreground {
        return Verdict::NotAGame;
    }
    let modules = facts.modules.as_deref();
    // Module list readable: a game must load a graphics API. Unreadable
    // (often anti-cheat protected): rely on the other signals.
    if modules.is_some_and(|m| !m.iter().any(|d| is_graphics_module(d))) {
        return Verdict::NotAGame;
    }

    let strong = facts.known_game || path.as_deref().is_some_and(in_launcher_library);
    if strong {
        return Verdict::Detected;
    }
    let weak = modules.is_some_and(|m| m.iter().any(|d| is_game_module(d)))
        || exe.contains("-shipping")
        || path.as_deref().is_some_and(|p| p.contains("\\games\\"))
        || (facts.is_packaged && modules.is_none());
    if weak && facts.is_fullscreen {
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
            modules: Some(vec!["d3d12.dll".into()]),
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
        f.modules = Some(vec![]);
        f.is_fullscreen = true;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn fullscreen_rendering_alone_is_not_a_game() {
        let cfg = GameSettings::default();
        // A video player / streaming app showing fullscreen video.
        let f = facts(
            "SomeVideoApp.exe",
            r"C:\Users\me\AppData\Local\Programs\SomeVideoApp\SomeVideoApp.exe",
        );
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        let f = facts(
            "stremio-shell-ng.exe",
            r"C:\Users\me\AppData\Local\Programs\LNV\Stremio-4\stremio-shell-ng.exe",
        );
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn steam_software_is_not_a_game() {
        let cfg = GameSettings::default();
        for (exe, dir) in [
            ("godot.windows.opt.tools.64.exe", "Godot Engine"),
            ("Godot_v4.3-stable_win64.exe", "Godot Engine"),
            ("wallpaper64.exe", "wallpaper_engine"),
        ] {
            let f = facts(exe, &format!(r"C:\SteamLibrary\steamapps\common\{dir}\{exe}"));
            assert_eq!(classify(&f, &cfg), Verdict::NotAGame, "{exe}");
        }
    }

    #[test]
    fn windows_known_games_detected_even_windowed() {
        let cfg = GameSettings::default();
        let mut f = facts("Dolphin.exe", r"F:\Dolphin-x64\Dolphin.exe");
        f.is_fullscreen = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        f.known_game = true;
        assert_eq!(classify(&f, &cfg), Verdict::Detected);
        // Still needs to be in the foreground.
        f.is_foreground = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn engine_modules_need_fullscreen() {
        let cfg = GameSettings::default();
        let mut f = facts("MyIndieGame.exe", r"C:\itch\MyIndieGame\MyIndieGame.exe");
        f.modules = Some(vec!["d3d11.dll".into(), "UnityPlayer.dll".into()]);
        assert_eq!(classify(&f, &cfg), Verdict::Detected);
        f.is_fullscreen = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        // An engine module without any graphics API isn't rendering.
        f.is_fullscreen = true;
        f.modules = Some(vec!["steam_api64.dll".into()]);
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn protected_processes_use_path_and_fullscreen() {
        let cfg = GameSettings::default();
        let mut f = facts(
            "FortniteClient-Win64-Shipping.exe",
            r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe",
        );
        f.modules = None;
        assert_eq!(classify(&f, &cfg), Verdict::Detected);
        assert_eq!(display_name(&f, &cfg), "Fortnite");
        f.is_foreground = false;
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
    }

    #[test]
    fn web_apps() {
        assert_eq!(
            web_app_id("MSEdge._crx_cinhimbnkkaeohfgghhklpknlkffjgod").as_deref(),
            Some("cinhimbnkkaeohfgghhklpknlkffjgod")
        );
        assert_eq!(
            web_app_id("Chrome._crx_cinhimbnkkaeohfgghhklpknlkffjgod.UserData.Default").as_deref(),
            Some("cinhimbnkkaeohfgghhklpknlkffjgod")
        );
        assert_eq!(web_app_id("Microsoft.Windows.Explorer"), None);
        assert_eq!(browser_name("MSEDGE.EXE"), Some("Microsoft Edge"));
        assert_eq!(
            web_app_name_from_title("Song • Artist - YouTube Music"),
            "YouTube Music"
        );
        assert_eq!(web_app_name_from_title("YouTube Music"), "YouTube Music");
    }

    #[test]
    fn capture_tools_are_never_games() {
        let cfg = GameSettings::default();
        // ShareX's region picker is a fullscreen D3D window in the foreground.
        let f = facts("ShareX.exe", r"C:\Program Files\ShareX\ShareX.exe");
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        let mut f = facts("recorder.exe", r"D:\Games\tools\recorder.exe");
        f.display_name = Some("Awesome Screen Recorder Pro".into());
        assert_eq!(classify(&f, &cfg), Verdict::NotAGame);
        assert!(is_capture_tool(Some(
            "ShareX - Screen capture, file sharing"
        )));
        assert!(!is_capture_tool(Some("Counter-Strike 2")));
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
