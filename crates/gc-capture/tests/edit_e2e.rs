//! Clip editor against a real FFmpeg: trim, mute one isolated track, rebuild
//! the Master mix, overwrite vs. copy, metadata/bookmarks follow the trim,
//! and per-track extraction for the player. Skipped without ffmpeg.

use gc_capture::edit::{export_clip, extract_tracks, probe, ExportRequest};
use gc_capture::Ffmpeg;
use gc_core::ffmpeg::TrackGain;
use gc_core::metadata::{Bookmark, ClipMetadata, MediaKind};
use std::path::Path;
use std::process::Command;

fn max_volume_db(ff: &Ffmpeg, file: &Path, track: usize) -> f64 {
    let out = Command::new(&ff.ffmpeg)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args([
            "-map",
            &format!("0:a:{track}"),
            "-af",
            "volumedetect",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    log.lines()
        .find_map(|l| l.split("max_volume:").nth(1))
        .and_then(|v| v.trim().trim_end_matches(" dB").parse().ok())
        .unwrap_or_else(|| panic!("no volumedetect output: {log}"))
}

#[test]
fn trim_mute_overwrite_and_copy() {
    let Ok(ff) = Ffmpeg::locate(&[]) else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("2026-11-04_13-37-00_Clip.mp4");
    let meta = ClipMetadata {
        version: 1,
        kind: MediaKind::Clip,
        game_title: Some("Test Game".into()),
        executable_name: Some("test.exe".into()),
        timestamp: "2026-11-04T13:37:00+00:00".into(),
        duration_ms: 8_000,
        bookmarks: vec![
            Bookmark {
                offset_ms: 1_000,
                label: None,
            },
            Bookmark {
                offset_ms: 4_000,
                label: Some("keep".into()),
            },
        ],
        resolution: Some("320x180".into()),
        encoder: Some("libx264".into()),
        audio_tracks: vec![],
    };
    // 8 s clip: Master = game+mic, Game (440 Hz), Microphone (880 Hz).
    let mut cmd = Command::new(&ff.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=8",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=8"])
        .args(["-f", "lavfi", "-i", "sine=frequency=880:duration=8"])
        .args([
            "-filter_complex",
            "[1:a]asplit[g1][g2];[2:a]asplit[m1][m2];[g1][m1]amix=inputs=2:normalize=0[mix]",
        ])
        .args([
            "-map", "0:v", "-map", "[mix]", "-map", "[g2]", "-map", "[m2]",
        ])
        .args([
            "-c:v",
            "libx264",
            "-g",
            "30",
            "-c:a",
            "aac",
            "-movflags",
            "+use_metadata_tags",
        ]);
    for (k, v) in meta.to_tags() {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    for (i, t) in ["Master", "Game", "Microphone"].iter().enumerate() {
        cmd.arg(format!("-metadata:s:a:{i}"))
            .arg(format!("handler_name={t}"));
    }
    assert!(cmd.arg(&clip).status().unwrap().success());

    // Copy: trim to 2.0–6.0 s and mute the microphone.
    let req = ExportRequest {
        start_ms: 2_000,
        end_ms: 6_000,
        gains: Some(vec![
            TrackGain {
                index: 1,
                volume: 1.0,
                muted: false,
            },
            TrackGain {
                index: 2,
                volume: 1.0,
                muted: true,
            },
        ]),
        overwrite: false,
    };
    let copy = export_clip(&ff, &clip, &req, meta.clone(), dir.path()).unwrap();
    assert!(copy.ends_with("2026-11-04_13-37-00_Clip_edit.mp4"));
    let info = probe(&ff, &copy).unwrap();
    assert!(
        (3_800..=5_200).contains(&info.duration_ms),
        "duration {}",
        info.duration_ms
    );
    assert_eq!(info.audio_tracks, ["Master", "Game", "Microphone"]);
    assert!(max_volume_db(&ff, &copy, 2) < -80.0, "mic muted");
    assert!(max_volume_db(&ff, &copy, 1) > -20.0, "game kept");
    assert!(
        max_volume_db(&ff, &copy, 0) > -20.0,
        "master rebuilt from game"
    );
    let m = ClipMetadata::from_tags(&info.tags).unwrap();
    assert_eq!(m.bookmarks.len(), 1);
    assert_eq!(m.bookmarks[0].offset_ms, 2_000);
    assert_eq!(m.duration_ms, 4_000);
    assert_eq!(info.chapters_ms, [0, 2_000]);

    // Overwrite: trim only, audio untouched.
    let before = std::fs::metadata(&clip).unwrap().len();
    let out = export_clip(
        &ff,
        &clip,
        &ExportRequest {
            start_ms: 0,
            end_ms: 3_000,
            gains: None,
            overwrite: true,
        },
        meta,
        dir.path(),
    )
    .unwrap();
    assert_eq!(out, clip);
    assert!(std::fs::metadata(&clip).unwrap().len() < before);
    let info = probe(&ff, &clip).unwrap();
    assert!(info.duration_ms <= 3_300, "duration {}", info.duration_ms);
    assert!(!dir
        .path()
        .join(".2026-11-04_13-37-00_Clip.editing.mp4")
        .exists());

    // Track extraction for the player.
    let cache = dir.path().join("cache");
    let tracks = extract_tracks(&ff, &clip, "k", &info.audio_tracks, &[1, 2], &cache).unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[1].title, "Microphone");
    for t in &tracks {
        let p = probe(&ff, &t.path).unwrap();
        assert_eq!(p.audio_tracks.len(), 1);
        assert!(p.width.is_none(), "audio only");
    }
}
