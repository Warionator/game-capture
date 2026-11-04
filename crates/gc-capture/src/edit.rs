//! Clip editing: trim + per-track volume, saved over the original or as a
//! copy; and extraction of individual audio tracks for the player.
//!
//! Video is always stream-copied (lossless and near-instant), so a trim
//! starts on the keyframe at or before the chosen start — at most one second
//! early for game-capture recordings. Audio is only re-encoded when track
//! volumes are changed; the Master track is then rebuilt from the adjusted
//! tracks so it matches what the user heard.

use crate::ffmpeg::{hidden, Ffmpeg};
use gc_core::ffmpeg::{
    export_args, extract_tracks_args, parse_probe, probe_args, ExportSpec, ProbeInfo, TrackGain,
};
use gc_core::metadata::{Bookmark, ClipMetadata};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Track volumes to bake in; `None` keeps the audio untouched.
    pub gains: Option<Vec<TrackGain>>,
    /// Replace the original file instead of writing `<name>_edit.<ext>`.
    pub overwrite: bool,
}

pub fn probe(ff: &Ffmpeg, path: &Path) -> anyhow::Result<ProbeInfo> {
    let out = hidden(&ff.ffprobe).args(probe_args(path)).output()?;
    anyhow::ensure!(
        out.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

fn run(ff: &Ffmpeg, args: Vec<String>) -> anyhow::Result<()> {
    let out = ff.command().args(args).stdout(Stdio::null()).output()?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Path for a copy: `<stem>_edit.<ext>`, `<stem>_edit_2.<ext>`…
pub fn copy_path(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let ext = input.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
    let first = dir.join(format!("{stem}_edit.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}_edit_{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// Shifts bookmarks into the trimmed range.
pub fn trim_bookmarks(marks: &[Bookmark], start_ms: u64, end_ms: u64) -> Vec<Bookmark> {
    marks
        .iter()
        .filter(|b| b.offset_ms >= start_ms && b.offset_ms < end_ms)
        .map(|b| Bookmark {
            offset_ms: b.offset_ms - start_ms,
            label: b.label.clone(),
        })
        .collect()
}

/// Writes the edited clip and returns its path. `fallback` supplies
/// metadata for files that carry none (e.g. clips from other recorders).
pub fn export_clip(
    ff: &Ffmpeg,
    input: &Path,
    req: &ExportRequest,
    fallback: ClipMetadata,
    temp_dir: &Path,
) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(req.end_ms > req.start_ms, "the selection is empty");
    let info = probe(ff, input)?;
    let end_ms = if info.duration_ms > 0 {
        req.end_ms.min(info.duration_ms)
    } else {
        req.end_ms
    };
    let mut meta = ClipMetadata::from_tags(&info.tags).unwrap_or(fallback);
    meta.duration_ms = end_ms.saturating_sub(req.start_ms);
    meta.bookmarks = trim_bookmarks(&meta.bookmarks, req.start_ms, end_ms);
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&meta.timestamp) {
        meta.timestamp = (t + chrono::Duration::milliseconds(req.start_ms as i64)).to_rfc3339();
    }
    meta.audio_tracks = info.audio_tracks.clone();

    std::fs::create_dir_all(temp_dir)?;
    let chapters = temp_dir.join(format!("edit-chapters-{}.txt", std::process::id()));
    let chapters_file = if meta.bookmarks.is_empty() {
        None
    } else {
        std::fs::write(&chapters, meta.ffmetadata_chapters())?;
        Some(chapters.to_string_lossy().into_owned())
    };
    let n_audio = info.audio_tracks.len();
    let spec = ExportSpec {
        start_ms: req.start_ms,
        end_ms,
        audio_streams: n_audio,
        gains: req.gains.clone().filter(|_| n_audio > 0),
        rebuild_master: n_audio > 1 && info.audio_tracks[0].eq_ignore_ascii_case("Master"),
        tags: meta.to_tags(),
        audio_titles: info.audio_tracks.clone(),
        chapters_file,
        audio_bitrate_kbps: 160,
    };

    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let out = if req.overwrite {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
        input.with_file_name(format!(".{stem}.editing.{ext}"))
    } else {
        copy_path(input)
    };
    let result = run(ff, export_args(input, &out, &spec));
    let _ = std::fs::remove_file(&chapters);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(e.context("saving the edited clip failed"));
    }
    if req.overwrite {
        // Replaces the original atomically (MoveFileEx REPLACE_EXISTING).
        if let Err(e) = std::fs::rename(&out, input) {
            let _ = std::fs::remove_file(&out);
            anyhow::bail!("could not replace the original (is it open in another program?): {e}");
        }
        Ok(input.to_path_buf())
    } else {
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFile {
    /// Audio stream index in the clip.
    pub index: usize,
    pub title: String,
    pub path: PathBuf,
}

/// Extracts audio streams `indices` of `input` into `cache_dir` (reused if
/// already extracted for this file version).
pub fn extract_tracks(
    ff: &Ffmpeg,
    input: &Path,
    cache_key: &str,
    titles: &[String],
    indices: &[usize],
    cache_dir: &Path,
) -> anyhow::Result<Vec<TrackFile>> {
    std::fs::create_dir_all(cache_dir)?;
    let files: Vec<(usize, PathBuf)> = indices
        .iter()
        .map(|&i| (i, cache_dir.join(format!("{cache_key}-a{i}.m4a"))))
        .collect();
    let missing: Vec<(usize, PathBuf)> =
        files.iter().filter(|(_, p)| !p.exists()).cloned().collect();
    if !missing.is_empty() {
        // Stream copy first (instant); AAC re-encode for other codecs.
        if run(ff, extract_tracks_args(input, &missing, true)).is_err() {
            run(ff, extract_tracks_args(input, &missing, false))?;
        }
    }
    Ok(files
        .into_iter()
        .map(|(index, path)| TrackFile {
            index,
            title: titles
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("Track {}", index + 1)),
            path,
        })
        .collect())
}

/// Removes cached track files older than `max_age`.
pub fn prune_track_cache(cache_dir: &Path, max_age: std::time::Duration) {
    let Ok(rd) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_follow_the_trim() {
        let marks = vec![
            Bookmark {
                offset_ms: 500,
                label: None,
            },
            Bookmark {
                offset_ms: 2_500,
                label: Some("x".into()),
            },
            Bookmark {
                offset_ms: 9_000,
                label: None,
            },
        ];
        let t = trim_bookmarks(&marks, 1_000, 5_000);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].offset_ms, 1_500);
        assert_eq!(t[0].label.as_deref(), Some("x"));
    }

    #[test]
    fn copy_names_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("2026-11-04_13-37-00_Clip.mp4");
        assert!(copy_path(&f).ends_with("2026-11-04_13-37-00_Clip_edit.mp4"));
        std::fs::write(dir.path().join("2026-11-04_13-37-00_Clip_edit.mp4"), b"").unwrap();
        assert!(copy_path(&f).ends_with("2026-11-04_13-37-00_Clip_edit_2.mp4"));
    }
}
