//! End-to-end check of the pipeline pieces against a real FFmpeg binary:
//! encode → MPEG-TS on stdout → chunker → ring buffer → remux with embedded
//! metadata and bookmark chapters → ffprobe → metadata recovery.
//!
//! Skipped (passes trivially) when `ffmpeg`/`ffprobe` are not on PATH.

use gc_core::config::{Codec, FrameRate, RateControl, Resolution};
use gc_core::ffmpeg::*;
use gc_core::metadata::{Bookmark, ClipMetadata, MediaKind};
use gc_core::ring::{ReplayRing, Store};
use gc_core::ts::TsChunker;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn encode_chunk_ring_remux_probe() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();

    // Two raw f32le audio "pipes" (files here) standing in for WASAPI.
    let mut urls = Vec::new();
    for (i, freq) in [440, 660].iter().enumerate() {
        let p = dir.path().join(format!("a{i}.f32"));
        let st = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={freq}:duration=20:sample_rate=48000"
            ))
            .args(["-ac", "2", "-f", "f32le", "-y"])
            .arg(&p)
            .status()
            .unwrap();
        assert!(st.success());
        urls.push(p.to_string_lossy().into_owned());
    }

    let spec = PipelineSpec {
        video: VideoSource::TestPattern,
        source_size: (640, 360),
        fps: FrameRate(30),
        resolution: Resolution::Native,
        backend: Backend::Software,
        codec: Codec::H264,
        rate: RateControl::Cbr {
            bitrate_kbps: 2_000,
        },
        zero_copy: false,
        gpu_scaler: None,
        has_scale_d3d11: false,
        focus_screen: None,
        audio: vec![
            AudioInput {
                url: urls[0].clone(),
                in_mix: true,
                denoise: None,
            },
            AudioInput {
                url: urls[1].clone(),
                in_mix: true,
                denoise: Some(12),
            },
        ],
        mixdown: true,
        audio_bitrate_kbps: 128,
        output: Output::StdoutTs,
    };
    let mut child = Command::new("ffmpeg")
        .args(spec.build_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut out = child.stdout.take().unwrap();
    let mut chunker = TsChunker::new();
    let mut ring = ReplayRing::new(
        Duration::from_secs(3),
        Store::Disk(dir.path().into()),
        u64::MAX,
    )
    .unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    let mut chunks = 0;
    while chunks < 6 {
        let n = out.read(&mut buf).unwrap();
        assert!(n > 0, "ffmpeg ended early");
        for c in chunker.push(&buf[..n]) {
            ring.push(c).unwrap();
            chunks += 1;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(chunker.stats.keyframes >= 6);
    let stats = ring.stats();
    assert!(
        stats.chunks <= 5,
        "ring evicts beyond its 3 s window: {stats:?}"
    );

    // Save the last 2 seconds as a clip.
    let snap = ring.snapshot(2.0);
    assert!(!snap.is_empty());
    let meta = ClipMetadata {
        version: 1,
        kind: MediaKind::Clip,
        game_title: Some("Test Game".into()),
        executable_name: Some("test.exe".into()),
        timestamp: "2026-11-04T13:37:00+00:00".into(),
        duration_ms: 2_000,
        bookmarks: vec![Bookmark {
            offset_ms: 500,
            label: Some("nice".into()),
        }],
        resolution: Some("640x360".into()),
        encoder: Some("libx264".into()),
        audio_tracks: vec!["Master".into(), "Game".into(), "Microphone".into()],
    };
    let chapters = dir.path().join("chapters.txt");
    std::fs::write(&chapters, meta.ffmetadata_chapters()).unwrap();
    for ext in ["mp4", "mkv"] {
        let output = dir.path().join(format!("clip.{ext}"));
        let remux = RemuxSpec {
            tags: meta.to_tags(),
            audio_titles: meta.audio_tracks.clone(),
            chapters_file: Some(chapters.to_string_lossy().into_owned()),
        };
        let mut ff = Command::new("ffmpeg")
            .args(remux_args("pipe:0", &output, &remux))
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut stdin = ff.stdin.take().unwrap();
            for c in &snap {
                c.write_to(&mut stdin).unwrap();
            }
            stdin.flush().unwrap();
        }
        assert!(ff.wait().unwrap().success(), "remux to {ext} failed");

        let probe = Command::new("ffprobe")
            .args(probe_args(&output))
            .output()
            .unwrap();
        assert!(probe.status.success());
        let info = parse_probe(&String::from_utf8_lossy(&probe.stdout)).unwrap();
        assert_eq!((info.width, info.height), (Some(640), Some(360)), "{ext}");
        assert_eq!(info.audio_tracks, ["Master", "Game", "Microphone"], "{ext}");
        assert!(
            info.duration_ms >= 1_500 && info.duration_ms <= 4_500,
            "{ext}: {}",
            info.duration_ms
        );
        assert_eq!(info.chapters_ms, [0, 500], "{ext}");
        let recovered =
            ClipMetadata::from_tags(&info.tags).unwrap_or_else(|| panic!("{ext}: {:?}", info.tags));
        assert_eq!(recovered, meta, "{ext}");

        // Thumbnail generation.
        let thumb = dir.path().join(format!("thumb-{ext}.jpg"));
        let st = Command::new("ffmpeg")
            .args(thumbnail_args(&output, &thumb, 0.5, false))
            .status()
            .unwrap();
        assert!(st.success() && thumb.metadata().unwrap().len() > 0);
    }
}
