//! Exercises the audio → FFmpeg handshake with real pipes: several raw PCM
//! inputs that FFmpeg opens one after another while the engine streams into
//! them. On Windows these are named pipes; here POSIX FIFOs stand in, with
//! the same blocking-open semantics. Guards against start-up deadlocks and
//! checks every track ends up in the output.
#![cfg(unix)]

use gc_audio::{
    AudioEngine, AudioSink, CaptureHandle, CaptureTarget, EngineTrack, SampleRing, SourceFactory,
};
use gc_core::config::{Codec, FrameRate, RateControl, Resolution, TrackKind};
use gc_core::ffmpeg::{
    parse_probe, probe_args, AudioInput, Backend, Output, PipelineSpec, VideoSource,
};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Fifo {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
}

impl AudioSink for Fifo {
    fn connect(&mut self) -> std::io::Result<()> {
        // Blocks until FFmpeg opens the FIFO for reading.
        self.file = Some(std::fs::OpenOptions::new().write(true).open(&self.path)?);
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.as_mut().expect("connected").write_all(bytes)
    }
}

struct Tone(Arc<AtomicBool>);
impl CaptureHandle for Tone {
    fn is_alive(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
impl Drop for Tone {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct ToneFactory;
impl SourceFactory for ToneFactory {
    fn open(&self, _: &CaptureTarget, ring: SampleRing) -> anyhow::Result<Box<dyn CaptureHandle>> {
        let alive = Arc::new(AtomicBool::new(true));
        let a = alive.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut sent = 0u64;
            let mut phase = 0f32;
            while a.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                let due = (start.elapsed().as_secs_f64() * 48_000.0) as u64;
                let mut buf = Vec::with_capacity(((due - sent) * 2) as usize);
                for _ in sent..due {
                    let s = (phase * std::f32::consts::TAU).sin() * 0.3;
                    phase = (phase + 440.0 / 48_000.0) % 1.0;
                    buf.extend([s, s]);
                }
                ring.push(&buf);
                sent = due;
            }
        });
        Ok(Box::new(Tone(alive)))
    }
    fn supports_process_loopback(&self) -> bool {
        true
    }
}

fn have(bin: &str, flag: &str) -> bool {
    Command::new(bin)
        .arg(flag)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn ffmpeg_reads_all_tracks_from_pipes() {
    if !have("ffmpeg", "-version") || !have("ffprobe", "-version") || !have("mkfifo", "--version") {
        eprintln!("ffmpeg/mkfifo not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let kinds = [TrackKind::Game, TrackKind::Mic, TrackKind::Game];
    let mut sinks: Vec<Box<dyn AudioSink>> = Vec::new();
    let mut inputs = Vec::new();
    for i in 0..kinds.len() {
        let path = dir.path().join(format!("track{i}.pcm"));
        assert!(Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        inputs.push(AudioInput {
            url: path.to_string_lossy().into_owned(),
            in_mix: i != 2,
            denoise: i == 1,
        });
        sinks.push(Box::new(Fifo { path, file: None }));
    }
    let out = dir.path().join("out.mkv");
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
        audio: inputs,
        mixdown: true,
        audio_bitrate_kbps: 128,
        output: Output::File(out.to_string_lossy().into_owned()),
    };
    let mut ff = Command::new("ffmpeg")
        .args(spec.build_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let tracks = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| EngineTrack {
            title: format!("t{i}"),
            kind: k.clone(),
            volume: 1.0,
            muted: false,
        })
        .collect();
    let engine = AudioEngine::start(
        tracks,
        Some(1),
        None,
        false,
        Arc::new(ToneFactory),
        Some(sinks),
    )
    .unwrap();

    // Let it record ~3 s; a deadlock would leave the output empty.
    std::thread::sleep(Duration::from_secs(4));
    ff.stdin.take().unwrap().write_all(b"q").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = ff.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() > deadline {
            let _ = ff.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    engine.stop();
    assert!(
        status.is_some_and(|s| s.success()),
        "ffmpeg did not finish cleanly"
    );

    let probe = Command::new("ffprobe")
        .args(probe_args(&out))
        .output()
        .unwrap();
    let info = parse_probe(&String::from_utf8_lossy(&probe.stdout)).unwrap();
    assert_eq!(info.audio_tracks.len(), 4, "master + 3 isolated tracks");
    assert!(info.duration_ms >= 2_000, "duration {}", info.duration_ms);

    // The master track must carry signal (not just the lead-in silence).
    let vol = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(&out)
        .args(["-map", "0:a:0", "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&vol.stderr);
    let max_db: f64 = log
        .lines()
        .find_map(|l| l.split("max_volume:").nth(1))
        .and_then(|v| v.trim().trim_end_matches(" dB").parse().ok())
        .expect("volumedetect output");
    assert!(max_db > -20.0, "master track looks silent: {max_db} dB");
}
