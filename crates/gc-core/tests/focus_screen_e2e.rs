//! The "Game not in focus" screen is switched at runtime by writing
//! `focus_command` to FFmpeg's stdin. This drives a real FFmpeg (synthetic
//! source, software encoder) through capture → focus screen → capture and
//! checks the recorded frames actually change. Skipped without ffmpeg.

use gc_core::config::{Codec, FrameRate, RateControl, Resolution};
use gc_core::ffmpeg::*;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Average RGB of the frame at `t` seconds.
fn avg_rgb(file: &Path, t: f64) -> [u8; 3] {
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{t:.2}"),
            "-i",
        ])
        .arg(file)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=1:1:flags=area",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.stdout.len(),
        3,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    [out.stdout[0], out.stdout[1], out.stdout[2]]
}

#[test]
fn focus_screen_switches_live() {
    if !have("ffmpeg") {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("focus.mkv");
    let font = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "C:/Windows/Fonts/segoeui.ttf",
    ]
    .into_iter()
    .find(|f| Path::new(f).exists())
    .map(String::from);
    let spec = PipelineSpec {
        video: VideoSource::TestPattern,
        source_size: (320, 180),
        fps: FrameRate(30),
        resolution: Resolution::Native,
        backend: Backend::Software,
        codec: Codec::H264,
        rate: RateControl::Cbr {
            bitrate_kbps: 1_000,
        },
        zero_copy: false,
        gpu_scaler: None,
        has_scale_d3d11: false,
        focus_screen: Some(FocusScreen { font_file: font }),
        audio: vec![],
        mixdown: false,
        audio_bitrate_kbps: 128,
        output: Output::File(out.to_string_lossy().into_owned()),
        ts_offset_secs: 0.0,
    };
    let mut ff = Command::new("ffmpeg")
        .args(spec.build_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = ff.stdin.take().unwrap();
    let t0 = Instant::now();
    let at = |secs: f64| {
        let d = Duration::from_secs_f64(secs).saturating_sub(t0.elapsed());
        std::thread::sleep(d);
    };
    at(2.0);
    stdin.write_all(focus_command(true).as_bytes()).unwrap();
    stdin.flush().unwrap();
    at(4.5);
    stdin.write_all(focus_command(false).as_bytes()).unwrap();
    stdin.flush().unwrap();
    at(7.0);
    stdin.write_all(b"q").unwrap();
    drop(stdin);
    assert!(ff.wait().unwrap().success());

    let bg: [u8; 3] = [0x0b, 0x0d, 0x12];
    let near_bg = |c: [u8; 3]| {
        c.iter()
            .zip(bg)
            .all(|(a, b)| (*a as i32 - b as i32).abs() <= 12)
    };
    let before = avg_rgb(&out, 1.0);
    let during = avg_rgb(&out, 3.5);
    let after = avg_rgb(&out, 6.2);
    assert!(!near_bg(before), "capture visible before: {before:?}");
    assert!(near_bg(during), "focus screen while unfocused: {during:?}");
    assert!(!near_bg(after), "capture back after refocus: {after:?}");
}
