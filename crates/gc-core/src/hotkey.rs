//! Hotkey chords: parsing (`"Ctrl+Shift+F10"`), canonical formatting and
//! Windows virtual-key mapping. The low-level keyboard hook in `gc-platform`
//! reports raw virtual-key codes and uses [`Hotkey::matches`] to dispatch.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// A key chord: zero or more modifiers plus one non-modifier key, identified
/// by its Windows virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub vk: u16,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("hotkey is empty")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("hotkey needs a non-modifier key")]
    MissingKey,
    #[error("hotkey has more than one non-modifier key")]
    MultipleKeys,
}

// Virtual-key codes of the modifier keys (both generic and left/right).
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;

/// Named keys and their virtual-key codes. Letters and digits are handled
/// separately (their VK equals the ASCII upper-case code).
const NAMED: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Escape", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Num0", 0x60),
    ("Num1", 0x61),
    ("Num2", 0x62),
    ("Num3", 0x63),
    ("Num4", 0x64),
    ("Num5", 0x65),
    ("Num6", 0x66),
    ("Num7", 0x67),
    ("Num8", 0x68),
    ("Num9", 0x69),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    ("ScrollLock", 0x91),
    ("Semicolon", 0xBA),
    ("Equal", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("BracketLeft", 0xDB),
    ("Backslash", 0xDC),
    ("BracketRight", 0xDD),
    ("Quote", 0xDE),
];

const ALIASES: &[(&str, &str)] = &[
    ("esc", "Escape"),
    ("return", "Enter"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("prtsc", "PrintScreen"),
    ("printscr", "PrintScreen"),
    ("print", "PrintScreen"),
    ("arrowleft", "Left"),
    ("arrowright", "Right"),
    ("arrowup", "Up"),
    ("arrowdown", "Down"),
    ("grave", "Backquote"),
    ("`", "Backquote"),
    ("-", "Minus"),
    ("=", "Equal"),
    (",", "Comma"),
    (".", "Period"),
    ("/", "Slash"),
    (";", "Semicolon"),
    ("'", "Quote"),
    ("[", "BracketLeft"),
    ("]", "BracketRight"),
    ("\\", "Backslash"),
];

/// Returns `true` for any modifier virtual-key code.
pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        VK_SHIFT
            | VK_CONTROL
            | VK_MENU
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_LMENU
            | VK_RMENU
            | VK_LWIN
            | VK_RWIN
    )
}

/// Maps a key name (case-insensitive) to a virtual-key code.
pub fn vk_from_name(name: &str) -> Option<u16> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let canonical = ALIASES
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(name))
        .map(|(_, c)| *c)
        .unwrap_or(name);

    if canonical.len() == 1 {
        let c = canonical.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u16);
        }
    }
    // F1..F24 = 0x70..0x87
    if let Some(n) = canonical
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<u16>().ok())
    {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    NAMED
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(canonical))
        .map(|(_, vk)| *vk)
}

/// Canonical name for a virtual-key code.
pub fn name_from_vk(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => NAMED
            .iter()
            .find(|(_, v)| *v == vk)
            .map(|(n, _)| n.to_string()),
    }
}

impl Hotkey {
    pub fn parse(s: &str) -> Result<Self, HotkeyError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut mods = Modifiers::default();
        let mut vk = None;
        // Support "Ctrl++" (plus as key) by splitting carefully.
        let parts: Vec<&str> = if let Some(head) = s.strip_suffix("++") {
            let mut p: Vec<&str> = head.split('+').collect();
            p.push("NumAdd");
            p
        } else {
            s.split('+').collect()
        };
        for raw in parts {
            let part = raw.trim();
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "menu" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                "win" | "super" | "meta" | "cmd" => mods.win = true,
                _ => {
                    let code = vk_from_name(part)
                        .ok_or_else(|| HotkeyError::UnknownKey(part.to_string()))?;
                    if vk.replace(code).is_some() {
                        return Err(HotkeyError::MultipleKeys);
                    }
                }
            }
        }
        Ok(Hotkey {
            mods,
            vk: vk.ok_or(HotkeyError::MissingKey)?,
        })
    }

    /// Exact match: the same key with exactly the same modifiers held.
    pub fn matches(&self, vk: u16, mods: Modifiers) -> bool {
        self.vk == vk && self.mods == mods
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.ctrl {
            parts.push("Ctrl".into());
        }
        if self.mods.alt {
            parts.push("Alt".into());
        }
        if self.mods.shift {
            parts.push("Shift".into());
        }
        if self.mods.win {
            parts.push("Win".into());
        }
        parts.push(name_from_vk(self.vk).unwrap_or_else(|| format!("VK{:#04X}", self.vk)));
        f.write_str(&parts.join("+"))
    }
}

/// Actions a hotkey can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyAction {
    SaveReplay,
    ToggleRecording,
    AddBookmark,
    Screenshot,
}

/// Parses all configured bindings. Invalid or empty entries are reported
/// and skipped; duplicate chords keep the first action.
pub fn bindings(cfg: &crate::config::HotkeySettings) -> (Vec<(Hotkey, HotkeyAction)>, Vec<String>) {
    let mut out: Vec<(Hotkey, HotkeyAction)> = Vec::new();
    let mut errors = Vec::new();
    for (text, action) in [
        (&cfg.save_replay, HotkeyAction::SaveReplay),
        (&cfg.toggle_recording, HotkeyAction::ToggleRecording),
        (&cfg.add_bookmark, HotkeyAction::AddBookmark),
        (&cfg.screenshot, HotkeyAction::Screenshot),
    ] {
        if text.trim().is_empty() {
            continue;
        }
        match Hotkey::parse(text) {
            Ok(hk) if out.iter().any(|(h, _)| *h == hk) => {
                errors.push(format!("{action:?}: `{text}` is already bound"));
            }
            Ok(hk) => out.push((hk, action)),
            Err(e) => errors.push(format!("{action:?}: {e}")),
        }
    }
    (out, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let hk = Hotkey::parse("ctrl + shift + f10").unwrap();
        assert!(hk.mods.ctrl && hk.mods.shift && !hk.mods.alt);
        assert_eq!(hk.vk, 0x79);
        assert_eq!(hk.to_string(), "Ctrl+Shift+F10");
        assert_eq!(Hotkey::parse("Alt+F9").unwrap().to_string(), "Alt+F9");
        assert_eq!(
            Hotkey::parse("Win+Alt+PrtSc").unwrap().to_string(),
            "Alt+Win+PrintScreen"
        );
        assert_eq!(Hotkey::parse("Ctrl+1").unwrap().vk, b'1' as u16);
        assert_eq!(Hotkey::parse("Ctrl+m").unwrap().vk, b'M' as u16);
        assert_eq!(Hotkey::parse("Ctrl++").unwrap().vk, 0x6B);
        assert_eq!(Hotkey::parse("F24").unwrap().vk, 0x87);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Hotkey::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("Ctrl+Shift"), Err(HotkeyError::MissingKey));
        assert_eq!(Hotkey::parse("A+B"), Err(HotkeyError::MultipleKeys));
        assert!(matches!(
            Hotkey::parse("Ctrl+Banana"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(Hotkey::parse("F25").is_err());
    }

    #[test]
    fn matching_requires_exact_modifiers() {
        let hk = Hotkey::parse("Alt+F10").unwrap();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert!(hk.matches(0x79, alt));
        assert!(!hk.matches(0x79, Modifiers { shift: true, ..alt }));
        assert!(!hk.matches(0x78, alt));
    }

    #[test]
    fn bindings_report_duplicates() {
        let cfg = crate::config::HotkeySettings {
            save_replay: "Alt+F10".into(),
            toggle_recording: "alt+f10".into(),
            add_bookmark: "".into(),
            screenshot: "Nope+X".into(),
        };
        let (b, errs) = bindings(&cfg);
        assert_eq!(b.len(), 1);
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn every_named_key_roundtrips() {
        for (name, vk) in NAMED {
            assert_eq!(vk_from_name(name), Some(*vk), "{name}");
            assert_eq!(name_from_vk(*vk).as_deref(), Some(*name));
        }
    }
}
