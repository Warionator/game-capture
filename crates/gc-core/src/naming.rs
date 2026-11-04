//! Library file layout: `<root>/<Game>/<YYYY-MM-DD_HH-MM-SS>_<Kind>.<ext>`.

use crate::metadata::MediaKind;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::path::{Path, PathBuf};

pub const UNCATEGORIZED: &str = "Uncategorized";
const TIMESTAMP_FMT: &str = "%Y-%m-%d_%H-%M-%S";

/// Makes a string safe to use as a single Windows path component.
pub fn sanitize_component(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            // Strip ™ ® © which games love to put in their product names.
            '\u{2122}' | '\u{00AE}' | '\u{00A9}' => ' ',
            c => c,
        })
        .collect();
    // Collapse whitespace runs.
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // Windows silently drops trailing dots/spaces.
    let trimmed = out.trim_end_matches(['.', ' ']).to_string();
    out = trimmed;
    if out.chars().count() > 80 {
        out = out
            .chars()
            .take(80)
            .collect::<String>()
            .trim_end_matches(['.', ' '])
            .to_string();
    }
    if out.is_empty() {
        return UNCATEGORIZED.to_string();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
}

/// Folder name used for a game; `None` → `Uncategorized`.
pub fn game_folder(game: Option<&str>) -> String {
    match game {
        Some(g) if !g.trim().is_empty() => sanitize_component(g),
        _ => UNCATEGORIZED.to_string(),
    }
}

/// File name for a capture taken at `when`.
pub fn file_name(when: DateTime<Local>, kind: MediaKind, ext: &str) -> String {
    format!("{}_{}.{}", when.format(TIMESTAMP_FMT), kind.label(), ext)
}

/// Full output path, de-duplicated with `_2`, `_3`… if the file exists.
pub fn output_path(
    root: &Path,
    game: Option<&str>,
    when: DateTime<Local>,
    kind: MediaKind,
    ext: &str,
) -> PathBuf {
    let dir = root.join(game_folder(game));
    let base = file_name(when, kind, ext);
    let first = dir.join(&base);
    if !first.exists() {
        return first;
    }
    let stem = base.trim_end_matches(&format!(".{ext}")).to_string();
    (2..)
        .map(|n| dir.join(format!("{stem}_{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded iterator")
}

/// Recovers capture time and kind from a library file name. Used by the
/// self-healing scanner when a file carries no embedded metadata.
pub fn parse_file_name(name: &str) -> Option<(DateTime<Local>, Option<MediaKind>)> {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    if stem.len() < 19 || !stem.is_char_boundary(19) {
        return None;
    }
    let (ts, rest) = stem.split_at(19);
    let naive = NaiveDateTime::parse_from_str(ts, TIMESTAMP_FMT).ok()?;
    let when = Local.from_local_datetime(&naive).earliest()?;
    let kind = rest
        .strip_prefix('_')
        .and_then(|r| r.split('_').next())
        .and_then(MediaKind::from_label);
    Some((when, kind))
}

/// Media kind implied by a file extension.
pub fn kind_from_extension(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mkv" | "mov" | "webm" => Some(MediaKind::Clip),
        "png" | "jpg" | "jpeg" => Some(MediaKind::Screenshot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_windows_names() {
        assert_eq!(
            sanitize_component("Tom Clancy's Rainbow Six® Siege"),
            "Tom Clancy's Rainbow Six Siege"
        );
        assert_eq!(sanitize_component("Half-Life: Alyx"), "Half-Life Alyx");
        assert_eq!(sanitize_component("What?  Ok..."), "What Ok");
        assert_eq!(sanitize_component("con"), "_con");
        assert_eq!(sanitize_component("NUL.txt"), "_NUL.txt");
        assert_eq!(sanitize_component("  ** "), UNCATEGORIZED);
        assert_eq!(sanitize_component(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn builds_and_parses_names() {
        let when = Local.with_ymd_and_hms(2026, 11, 4, 13, 37, 0).unwrap();
        assert_eq!(
            file_name(when, MediaKind::Clip, "mp4"),
            "2026-11-04_13-37-00_Clip.mp4"
        );
        let (t, k) = parse_file_name("2026-11-04_13-37-00_Session.mkv").unwrap();
        assert_eq!(t, when);
        assert_eq!(k, Some(MediaKind::Session));
        let (_, k) = parse_file_name("2026-11-04_13-37-00_Clip_2.mp4").unwrap();
        assert_eq!(k, Some(MediaKind::Clip));
        assert!(parse_file_name("holiday.mp4").is_none());
        assert!(parse_file_name("ééééééééééééééééééééé.mp4").is_none());
    }

    #[test]
    fn output_path_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let when = Local.with_ymd_and_hms(2026, 11, 4, 13, 37, 0).unwrap();
        let p1 = output_path(dir.path(), Some("Fortnite"), when, MediaKind::Clip, "mp4");
        assert_eq!(
            p1,
            dir.path()
                .join("Fortnite")
                .join("2026-11-04_13-37-00_Clip.mp4")
        );
        std::fs::create_dir_all(p1.parent().unwrap()).unwrap();
        std::fs::write(&p1, b"x").unwrap();
        let p2 = output_path(dir.path(), Some("Fortnite"), when, MediaKind::Clip, "mp4");
        assert_eq!(p2.file_name().unwrap(), "2026-11-04_13-37-00_Clip_2.mp4");
        let p3 = output_path(dir.path(), None, when, MediaKind::Screenshot, "png");
        assert!(p3.starts_with(dir.path().join(UNCATEGORIZED)));
    }
}
