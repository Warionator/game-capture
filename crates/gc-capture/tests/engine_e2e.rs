//! Drives the whole capture engine with a real FFmpeg on a development host
//! (synthetic video source, software encoder): start capture, bookmark,
//! save a replay, record a session, and check the exported files carry the
//! embedded metadata the library scanner relies on.
//!
//! Skipped when `ffmpeg`/`ffprobe` are not on PATH. On Windows the engine
//! captures a real monitor through Desktop Duplication, which headless CI
//! runners may not provide, so it only runs there with `GC_E2E_WINDOWS=1`.

use crossbeam_channel::Receiver;
use gc_capture::{CaptureEngine, CaptureState, EngineEvent, EnginePaths, Ffmpeg, SavedMedia};
use gc_core::config::{Container, EncoderPreference, RateControl, Settings};
use gc_core::ffmpeg::{parse_probe, probe_args};
use gc_core::metadata::{ClipMetadata, MediaKind};
use std::process::Command;
use std::time::{Duration, Instant};

fn wait_for<T>(
    rx: &Receiver<EngineEvent>,
    secs: u64,
    mut f: impl FnMut(&EngineEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
            if let EngineEvent::Notice { message, .. } = &ev {
                eprintln!("notice: {message}");
            }
            if let Some(v) = f(&ev) {
                return v;
            }
        }
    }
    panic!("timed out waiting for engine event");
}

fn saved(kind: MediaKind) -> impl FnMut(&EngineEvent) -> Option<SavedMedia> {
    move |ev| match ev {
        EngineEvent::Saved(s) if s.kind == kind => Some(s.clone()),
        EngineEvent::Notice {
            error: true,
            message,
        } if message.contains("failed") => panic!("{message}"),
        _ => None,
    }
}

fn probe(ff: &Ffmpeg, path: &std::path::Path) -> gc_core::ffmpeg::ProbeInfo {
    let out = Command::new(&ff.ffprobe)
        .args(probe_args(path))
        .output()
        .unwrap();
    assert!(out.status.success());
    parse_probe(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

#[test]
fn replay_and_session_roundtrip() {
    if cfg!(windows) && std::env::var_os("GC_E2E_WINDOWS").is_none() {
        eprintln!("set GC_E2E_WINDOWS=1 to run against a real display; skipping");
        return;
    }
    let Ok(ff) = Ffmpeg::locate(&[]) else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut s = Settings {
        storage_root: root.path().to_path_buf(),
        ..Default::default()
    };
    s.video.encoder = EncoderPreference::Software;
    s.video.rate_control = RateControl::Cbr {
        bitrate_kbps: 2_000,
    };
    s.video.container = Container::Mkv;
    s.replay.duration_secs = 15;

    let (tx, rx) = crossbeam_channel::unbounded();
    let engine = CaptureEngine::new(
        EnginePaths {
            ffmpeg: Some(ff.clone()),
            temp_dir: temp.path().to_path_buf(),
        },
        s,
        tx,
    );
    engine.start_manual();
    wait_for(&rx, 60, |ev| match ev {
        EngineEvent::Status(st) if st.state == CaptureState::Buffering => Some(()),
        _ => None,
    });
    std::thread::sleep(Duration::from_millis(2500));
    engine.add_bookmark();
    std::thread::sleep(Duration::from_millis(1500));

    engine.save_replay();
    let clip = wait_for(&rx, 30, saved(MediaKind::Clip));
    assert!(clip.path.starts_with(root.path().join("Uncategorized")));
    assert_eq!(clip.path.extension().unwrap(), "mkv");
    let info = probe(&ff, &clip.path);
    assert!(
        info.duration_ms >= 2_500,
        "clip duration {}",
        info.duration_ms
    );
    let meta = ClipMetadata::from_tags(&info.tags).expect("embedded metadata");
    assert_eq!(meta.kind, MediaKind::Clip);
    assert_eq!(meta.bookmarks.len(), 1);
    assert_eq!(info.chapters_ms.len(), 2, "Start + bookmark");

    engine.toggle_recording();
    wait_for(&rx, 10, |ev| match ev {
        EngineEvent::Status(st) if st.state == CaptureState::Recording => Some(()),
        _ => None,
    });
    std::thread::sleep(Duration::from_millis(2500));
    engine.add_bookmark();
    std::thread::sleep(Duration::from_millis(1000));
    engine.toggle_recording();
    let session = wait_for(&rx, 30, saved(MediaKind::Session));
    assert!(session
        .path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .ends_with("_Session.mkv"));
    let info = probe(&ff, &session.path);
    assert!(
        info.duration_ms >= 2_500,
        "session duration {}",
        info.duration_ms
    );
    let meta = ClipMetadata::from_tags(&info.tags).expect("embedded metadata");
    assert_eq!(meta.kind, MediaKind::Session);
    assert_eq!(meta.bookmarks.len(), 1);
    assert!(
        (2_000..=4_500).contains(&meta.bookmarks[0].offset_ms),
        "bookmark offset {}",
        meta.bookmarks[0].offset_ms
    );
    // No temp files left next to the output.
    let leftovers: Vec<_> = std::fs::read_dir(session.path.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    engine.stop_capture();
    wait_for(&rx, 15, |ev| match ev {
        EngineEvent::Status(st) if st.state == CaptureState::Idle => Some(()),
        _ => None,
    });
}
