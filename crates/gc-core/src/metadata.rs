//! Metadata embedded into every capture and mirrored in the SQLite index.
//!
//! Each file carries one JSON tag (`GC_METADATA`) holding [`ClipMetadata`],
//! plus a few plain tags (`game_title`, `executable_name`…) so other tools
//! can read them. MP4 uses `-movflags use_metadata_tags` (mdta atoms in
//! `moov/udta/meta`); Matroska stores them as global tags.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const METADATA_TAG: &str = "GC_METADATA";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Clip,
    Session,
    Screenshot,
}

impl MediaKind {
    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Clip => "Clip",
            MediaKind::Session => "Session",
            MediaKind::Screenshot => "Screenshot",
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "clip" => Some(MediaKind::Clip),
            "session" => Some(MediaKind::Session),
            "screenshot" => Some(MediaKind::Screenshot),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Clip => "clip",
            MediaKind::Session => "session",
            MediaKind::Screenshot => "screenshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// Offset from the start of the file, in milliseconds.
    pub offset_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipMetadata {
    #[serde(default = "schema_version")]
    pub version: u32,
    pub kind: MediaKind,
    pub game_title: Option<String>,
    pub executable_name: Option<String>,
    /// RFC 3339 capture start time.
    pub timestamp: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    /// e.g. "1920x1080".
    pub resolution: Option<String>,
    #[serde(default)]
    pub encoder: Option<String>,
    #[serde(default)]
    pub audio_tracks: Vec<String>,
    /// Titles of the tracks the first ("Master") track is a mix of. Used to
    /// rebuild that mix when track volumes are edited.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mix_tracks: Vec<String>,
}

fn schema_version() -> u32 {
    SCHEMA_VERSION
}

impl ClipMetadata {
    /// Tags to embed, as `(key, value)` pairs for `-metadata key=value`.
    pub fn to_tags(&self) -> Vec<(String, String)> {
        let mut tags = vec![
            (
                METADATA_TAG.to_string(),
                serde_json::to_string(self).expect("serializable"),
            ),
            ("creation_time".into(), self.timestamp.clone()),
            ("timestamp".into(), self.timestamp.clone()),
            (
                "duration".into(),
                format!("{:.3}", self.duration_ms as f64 / 1000.0),
            ),
            (
                "bookmarks".into(),
                serde_json::to_string(&self.bookmarks).expect("serializable"),
            ),
            ("comment".into(), "Captured with game-capture".into()),
        ];
        if let Some(g) = &self.game_title {
            tags.push(("game_title".into(), g.clone()));
            tags.push(("title".into(), g.clone()));
        }
        if let Some(e) = &self.executable_name {
            tags.push(("executable_name".into(), e.clone()));
        }
        if let Some(r) = &self.resolution {
            tags.push(("resolution".into(), r.clone()));
        }
        tags
    }

    /// Recovers metadata from container tags (keys compared case-insensitively
    /// because Matroska demuxers commonly upper-case tag names).
    pub fn from_tags(tags: &BTreeMap<String, String>) -> Option<Self> {
        let get = |k: &str| {
            tags.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.as_str())
        };
        if let Some(meta) =
            get(METADATA_TAG).and_then(|j| serde_json::from_str::<ClipMetadata>(j).ok())
        {
            return Some(meta);
        }
        // Partial recovery from the plain tags written alongside the JSON.
        let game = get("game_title").map(str::to_string);
        let exe = get("executable_name").map(str::to_string);
        if game.is_none() && exe.is_none() {
            return None;
        }
        Some(ClipMetadata {
            version: SCHEMA_VERSION,
            kind: MediaKind::Clip,
            game_title: game,
            executable_name: exe,
            timestamp: get("timestamp")
                .or(get("creation_time"))
                .unwrap_or_default()
                .to_string(),
            duration_ms: get("duration")
                .and_then(|d| d.parse::<f64>().ok())
                .map(|d| (d * 1000.0) as u64)
                .unwrap_or(0),
            bookmarks: get("bookmarks")
                .and_then(|b| serde_json::from_str(b).ok())
                .unwrap_or_default(),
            resolution: get("resolution").map(str::to_string),
            encoder: None,
            audio_tracks: Vec::new(),
            mix_tracks: Vec::new(),
        })
    }

    /// FFmpeg `ffmetadata` document with one chapter per bookmark, so players
    /// show bookmarks as chapter marks. MP4 chapters are contiguous from 0, so
    /// a "Start" chapter covers the part before the first bookmark.
    pub fn ffmetadata_chapters(&self) -> String {
        let mut out = String::from(";FFMETADATA1\n");
        let escape = |s: &str| {
            s.chars()
                .flat_map(|c| match c {
                    '=' | ';' | '#' | '\\' | '\n' => vec!['\\', c],
                    c => vec![c],
                })
                .collect::<String>()
        };
        let mut marks = self.bookmarks.clone();
        marks.sort_by_key(|b| b.offset_ms);
        if let Some(first) = marks.first().filter(|b| b.offset_ms > 0) {
            out.push_str(&format!(
                "[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND={}\ntitle=Start\n",
                first.offset_ms
            ));
        }
        for (i, b) in marks.iter().enumerate() {
            let end = marks
                .get(i + 1)
                .map(|n| n.offset_ms)
                .unwrap_or(self.duration_ms.max(b.offset_ms + 1));
            out.push_str("[CHAPTER]\nTIMEBASE=1/1000\n");
            out.push_str(&format!(
                "START={}\nEND={}\n",
                b.offset_ms,
                end.max(b.offset_ms + 1)
            ));
            let title = b
                .label
                .clone()
                .unwrap_or_else(|| format!("Bookmark {}", i + 1));
            out.push_str(&format!("title={}\n", escape(&title)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ClipMetadata {
        ClipMetadata {
            version: SCHEMA_VERSION,
            kind: MediaKind::Session,
            game_title: Some("Elden Ring".into()),
            executable_name: Some("eldenring.exe".into()),
            timestamp: "2026-11-04T15:10:22+01:00".into(),
            duration_ms: 90_500,
            bookmarks: vec![
                Bookmark {
                    offset_ms: 60_000,
                    label: None,
                },
                Bookmark {
                    offset_ms: 12_000,
                    label: Some("boss = down; gg".into()),
                },
            ],
            resolution: Some("2560x1440".into()),
            encoder: Some("hevc_nvenc".into()),
            audio_tracks: vec!["Master".into(), "Game".into()],
            mix_tracks: vec!["Game".into()],
        }
    }

    #[test]
    fn json_tag_roundtrip_case_insensitive() {
        let m = sample();
        let tags: BTreeMap<_, _> = m
            .to_tags()
            .into_iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), v))
            .collect();
        assert_eq!(ClipMetadata::from_tags(&tags), Some(m));
    }

    #[test]
    fn partial_recovery_from_plain_tags() {
        let mut tags = BTreeMap::new();
        tags.insert("game_title".to_string(), "Fortnite".to_string());
        tags.insert("duration".to_string(), "12.5".to_string());
        let m = ClipMetadata::from_tags(&tags).unwrap();
        assert_eq!(m.game_title.as_deref(), Some("Fortnite"));
        assert_eq!(m.duration_ms, 12_500);
        assert!(ClipMetadata::from_tags(&BTreeMap::new()).is_none());
    }

    #[test]
    fn chapters_are_sorted_and_escaped() {
        let doc = sample().ffmetadata_chapters();
        assert!(doc.contains("START=0\nEND=12000\ntitle=Start"));
        let first = doc.find("START=12000").unwrap();
        let second = doc.find("START=60000").unwrap();
        assert!(first < second);
        assert!(doc.contains("END=60000"));
        assert!(doc.contains("END=90500"));
        assert!(doc.contains(r"title=boss \= down\; gg"));
    }
}
