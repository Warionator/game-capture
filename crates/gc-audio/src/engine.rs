use crate::buffer::{apply_gain, peak, SampleRing, TrackControl};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use gc_core::config::TrackKind;
use gc_core::ffmpeg::{CHANNELS, SAMPLE_RATE};
use gc_platform::process::{root_pids_by_exe, snapshot};
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(10);
/// Jitter cushion a source must accumulate before it is consumed.
const PRIME_MS: usize = 30;
/// Per-source ring capacity.
const RING_MS: usize = 500;
const RESOLVE_EVERY: Duration = Duration::from_secs(2);
/// Silence written to each sink before all sinks are connected.
const LEAD_IN_MS: usize = 200;

fn samples_for_ms(ms: usize) -> usize {
    SAMPLE_RATE as usize * CHANNELS as usize * ms / 1000
}

/// What a capture source records.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CaptureTarget {
    /// A process and all of its children (process loopback).
    ProcessTree(u32),
    /// Everything the default render device plays.
    SystemLoopback,
    /// A capture endpoint; `None` = default communications microphone.
    Microphone(Option<String>),
}

/// A running capture; dropping it stops the capture.
pub trait CaptureHandle: Send {
    fn is_alive(&self) -> bool;
}

/// Creates captures that push 48 kHz stereo f32 into a ring.
pub trait SourceFactory: Send + Sync {
    fn open(
        &self,
        target: &CaptureTarget,
        ring: SampleRing,
    ) -> anyhow::Result<Box<dyn CaptureHandle>>;
    /// Whether per-process loopback is available on this system.
    fn supports_process_loopback(&self) -> bool;
}

/// Destination of one track's PCM stream.
pub trait AudioSink: Send {
    /// Blocks until the consumer is attached (e.g. FFmpeg opened the pipe).
    fn connect(&mut self) -> io::Result<()>;
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// A path that, when opened by us, unblocks a pending [`connect`]
    /// (used on shutdown if FFmpeg never attached).
    ///
    /// [`connect`]: AudioSink::connect
    fn unblock_path(&self) -> Option<String> {
        None
    }
}

/// Writes PCM to a file (tests, debugging).
pub struct FileSink(pub std::fs::File);

impl AudioSink for FileSink {
    fn connect(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }
}

/// Factory that never produces audio (non-Windows hosts).
#[allow(dead_code)]
pub(crate) struct NullFactory;

impl SourceFactory for NullFactory {
    fn open(&self, _: &CaptureTarget, _: SampleRing) -> anyhow::Result<Box<dyn CaptureHandle>> {
        anyhow::bail!("audio capture is only available on Windows")
    }
    fn supports_process_loopback(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct EngineTrack {
    pub title: String,
    pub kind: TrackKind,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackLevel {
    pub title: String,
    /// Peak since the last call, linear 0..1.
    pub peak: f32,
    pub volume: f32,
    pub muted: bool,
    /// Number of live capture sources feeding the track.
    pub sources: usize,
}

struct Source {
    target: CaptureTarget,
    ring: SampleRing,
    handle: Box<dyn CaptureHandle>,
    primed: bool,
}

struct TrackState {
    title: String,
    kind: TrackKind,
    control: Arc<TrackControl>,
    sources: Mutex<Vec<Source>>,
}

struct Shared {
    tracks: Vec<Arc<TrackState>>,
    game_pid: Mutex<Option<u32>>,
    mic_device: Option<String>,
    system_fallback: bool,
    factory: Arc<dyn SourceFactory>,
    stop: AtomicBool,
    resolve_now: AtomicBool,
}

pub struct AudioEngine {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    unblock: Vec<String>,
}

impl AudioEngine {
    /// Starts capturing. `sinks` (one per track, in order) receive the PCM
    /// streams; pass `None` for a meter-only "monitor" engine.
    pub fn start(
        tracks: Vec<EngineTrack>,
        game_pid: Option<u32>,
        mic_device: Option<String>,
        system_fallback: bool,
        factory: Arc<dyn SourceFactory>,
        sinks: Option<Vec<Box<dyn AudioSink>>>,
    ) -> anyhow::Result<Self> {
        if let Some(s) = &sinks {
            anyhow::ensure!(s.len() == tracks.len(), "one sink per track required");
        }
        let shared = Arc::new(Shared {
            tracks: tracks
                .into_iter()
                .filter(|t| t.kind != TrackKind::Mix)
                .map(|t| {
                    Arc::new(TrackState {
                        control: TrackControl::new(t.volume, t.muted),
                        title: t.title,
                        kind: t.kind,
                        sources: Mutex::new(Vec::new()),
                    })
                })
                .collect(),
            game_pid: Mutex::new(game_pid),
            mic_device,
            system_fallback,
            factory,
            stop: AtomicBool::new(false),
            resolve_now: AtomicBool::new(true),
        });

        resolve_sources(&shared);

        let mut threads = Vec::new();
        let mut senders: Vec<Option<Sender<Vec<f32>>>> = vec![None; shared.tracks.len()];
        let mut unblock = Vec::new();
        if let Some(sinks) = sinks {
            unblock = sinks.iter().filter_map(|s| s.unblock_path()).collect();
            let connected = Arc::new(AtomicUsize::new(0));
            let total = sinks.len();
            for (i, sink) in sinks.into_iter().enumerate() {
                let (tx, rx) = bounded::<Vec<f32>>(100);
                senders[i] = Some(tx);
                let (shared, connected) = (shared.clone(), connected.clone());
                threads.push(
                    std::thread::Builder::new()
                        .name(format!("gc-audio-writer-{i}"))
                        .spawn(move || writer(sink, rx, connected, total, shared))?,
                );
            }
        }
        {
            let shared = shared.clone();
            threads.push(
                std::thread::Builder::new()
                    .name("gc-audio-pump".into())
                    .spawn(move || pump(shared, senders))?,
            );
        }
        {
            let shared = shared.clone();
            threads.push(
                std::thread::Builder::new()
                    .name("gc-audio-resolver".into())
                    .spawn(move || {
                        let mut last = Instant::now();
                        while !shared.stop.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(100));
                            if last.elapsed() >= RESOLVE_EVERY
                                || shared.resolve_now.swap(false, Ordering::SeqCst)
                            {
                                resolve_sources(&shared);
                                last = Instant::now();
                            }
                        }
                    })?,
            );
        }
        Ok(Self {
            shared,
            threads,
            unblock,
        })
    }

    /// Live volume/mute change for track `idx` (index among input tracks).
    pub fn set_track(&self, idx: usize, volume: f32, muted: bool) {
        if let Some(t) = self.shared.tracks.get(idx) {
            t.control.set_volume(volume);
            t.control.set_muted(muted);
        }
    }

    /// Live volume/mute change by track title.
    pub fn set_track_by_title(&self, title: &str, volume: f32, muted: bool) {
        for t in &self.shared.tracks {
            if t.title == title {
                t.control.set_volume(volume);
                t.control.set_muted(muted);
            }
        }
    }

    pub fn set_game_pid(&self, pid: Option<u32>) {
        *self.shared.game_pid.lock() = pid;
        self.shared.resolve_now.store(true, Ordering::SeqCst);
    }

    pub fn levels(&self) -> Vec<TrackLevel> {
        self.shared
            .tracks
            .iter()
            .map(|t| TrackLevel {
                title: t.title.clone(),
                peak: t.control.take_peak(),
                volume: t.control.volume(),
                muted: t.control.muted(),
                sources: t
                    .sources
                    .lock()
                    .iter()
                    .filter(|s| s.handle.is_alive())
                    .count(),
            })
            .collect()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for path in self.unblock.drain(..) {
            // Satisfies a ConnectNamedPipe that is still waiting; fails
            // harmlessly if FFmpeg is already attached.
            let _ = std::fs::OpenOptions::new().read(true).open(path);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        for t in &self.shared.tracks {
            t.sources.lock().clear();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Opens/closes capture sources so they match what each track wants.
fn resolve_sources(shared: &Shared) {
    let needs_procs = shared
        .tracks
        .iter()
        .any(|t| matches!(t.kind, TrackKind::Apps { .. }));
    let procs = if needs_procs { snapshot() } else { Vec::new() };
    let loopback = shared.factory.supports_process_loopback();
    for track in &shared.tracks {
        let desired: Vec<CaptureTarget> = match &track.kind {
            TrackKind::Mix => continue,
            TrackKind::Game => match *shared.game_pid.lock() {
                Some(pid) if loopback => vec![CaptureTarget::ProcessTree(pid)],
                Some(_) | None if !loopback && shared.system_fallback => {
                    vec![CaptureTarget::SystemLoopback]
                }
                _ => vec![],
            },
            TrackKind::Mic => vec![CaptureTarget::Microphone(shared.mic_device.clone())],
            TrackKind::Apps { executables } if loopback => root_pids_by_exe(&procs, executables)
                .into_iter()
                .map(CaptureTarget::ProcessTree)
                .collect(),
            TrackKind::Apps { .. } => vec![],
        };
        let mut sources = track.sources.lock();
        sources.retain(|s| desired.contains(&s.target) && s.handle.is_alive());
        for target in desired {
            if sources.iter().any(|s| s.target == target) {
                continue;
            }
            let ring = SampleRing::new(samples_for_ms(RING_MS));
            match shared.factory.open(&target, ring.clone()) {
                Ok(handle) => {
                    log::info!("audio track {}: capturing {target:?}", track.title);
                    sources.push(Source {
                        target,
                        ring,
                        handle,
                        primed: false,
                    });
                }
                Err(e) => log::debug!("audio track {}: cannot open {target:?}: {e}", track.title),
            }
        }
    }
}

/// Clock-driven mixer: emits exactly 48 000 frames per second per track.
fn pump(shared: Arc<Shared>, senders: Vec<Option<Sender<Vec<f32>>>>) {
    let start = Instant::now();
    let mut frames_sent: u64 = 0;
    let prime = samples_for_ms(PRIME_MS);
    while !shared.stop.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
        let due_total = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
        // Cap catch-up after a stall at one second.
        let frames = (due_total - frames_sent).min(SAMPLE_RATE as u64) as usize;
        if frames == 0 {
            continue;
        }
        frames_sent = due_total;
        let n = frames * CHANNELS as usize;
        for (i, track) in shared.tracks.iter().enumerate() {
            let mut block = vec![0f32; n];
            for src in track.sources.lock().iter_mut() {
                if !src.primed {
                    if src.ring.len() >= prime {
                        src.primed = true;
                    } else {
                        continue;
                    }
                }
                if src.ring.mix_into(&mut block) < n {
                    // Underrun (source went quiet): rebuild the cushion.
                    src.primed = false;
                }
            }
            apply_gain(&mut block, track.control.gain());
            track.control.update_peak(peak(&block));
            if let Some(Some(tx)) = senders.get(i) {
                match tx.try_send(block) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {}
                }
            }
        }
    }
}

fn as_bytes(samples: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte pattern is a valid u8.
    unsafe {
        std::slice::from_raw_parts(
            samples.as_ptr().cast::<u8>(),
            std::mem::size_of_val(samples),
        )
    }
}

fn writer(
    mut sink: Box<dyn AudioSink>,
    rx: Receiver<Vec<f32>>,
    connected: Arc<AtomicUsize>,
    total: usize,
    shared: Arc<Shared>,
) {
    if let Err(e) = sink.connect() {
        log::warn!("audio sink connect failed: {e}");
        connected.fetch_add(1, Ordering::SeqCst);
        return;
    }
    // Lead-in silence: FFmpeg reads a little from each raw input while it
    // opens them one after another, so every pipe must have data before the
    // others are connected. The same amount of live audio is skipped below,
    // keeping sample time aligned with wall-clock (and therefore video).
    let lead_in = samples_for_ms(LEAD_IN_MS);
    let _ = sink.write(as_bytes(&vec![0f32; lead_in]));
    connected.fetch_add(1, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(10);
    while connected.load(Ordering::SeqCst) < total && Instant::now() < deadline {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Drop audio queued before every track was attached so tracks start
    // aligned with each other and with the video.
    while rx.try_recv().is_ok() {}
    let mut skip = lead_in;
    while !shared.stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(block) => {
                let start = skip.min(block.len());
                skip -= start;
                if start == block.len() {
                    continue;
                }
                if let Err(e) = sink.write(as_bytes(&block[start..])) {
                    log::debug!("audio sink closed: {e}");
                    return;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct Tone {
        alive: Arc<AtomicBool>,
    }
    impl CaptureHandle for Tone {
        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }
    impl Drop for Tone {
        fn drop(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    /// Produces a constant 0.25 signal in 10 ms packets.
    struct ToneFactory;
    impl SourceFactory for ToneFactory {
        fn open(
            &self,
            target: &CaptureTarget,
            ring: SampleRing,
        ) -> anyhow::Result<Box<dyn CaptureHandle>> {
            anyhow::ensure!(!matches!(target, CaptureTarget::SystemLoopback));
            let alive = Arc::new(AtomicBool::new(true));
            let a = alive.clone();
            std::thread::spawn(move || {
                let start = Instant::now();
                let mut sent = 0u64;
                while a.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                    let due = (start.elapsed().as_secs_f64() * 48_000.0) as u64;
                    ring.push(&vec![0.25f32; ((due - sent) * 2) as usize]);
                    sent = due;
                }
            });
            Ok(Box::new(Tone { alive }))
        }
        fn supports_process_loopback(&self) -> bool {
            true
        }
    }

    #[test]
    fn continuous_streams_with_live_gain() {
        let dir = std::env::temp_dir().join(format!("gc-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tracks = vec![
            EngineTrack {
                title: "Game".into(),
                kind: TrackKind::Game,
                volume: 1.0,
                muted: false,
            },
            EngineTrack {
                title: "Microphone".into(),
                kind: TrackKind::Mic,
                volume: 2.0,
                muted: false,
            },
        ];
        let paths: Vec<_> = (0..2).map(|i| dir.join(format!("t{i}.f32"))).collect();
        let sinks: Vec<Box<dyn AudioSink>> = paths
            .iter()
            .map(|p| Box::new(FileSink(std::fs::File::create(p).unwrap())) as Box<dyn AudioSink>)
            .collect();
        let engine = AudioEngine::start(
            tracks,
            Some(42),
            None,
            true,
            Arc::new(ToneFactory),
            Some(sinks),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let levels = engine.levels();
        assert_eq!(levels.len(), 2);
        assert!((levels[0].peak - 0.25).abs() < 1e-6, "{levels:?}");
        assert!((levels[1].peak - 0.5).abs() < 1e-6, "{levels:?}");
        assert_eq!(levels[0].sources, 1);
        engine.set_track(1, 1.0, true);
        std::thread::sleep(Duration::from_millis(100));
        let _ = engine.levels();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(engine.levels()[1].peak, 0.0, "muted");
        engine.set_game_pid(None);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(engine.levels()[0].sources, 0, "game source closed");
        engine.stop();

        for p in &paths {
            let bytes = std::fs::metadata(p).unwrap().len();
            // ~0.9 s of 48 kHz stereo f32 = 345 600 bytes; allow scheduling slack.
            assert!(
                (200_000..=500_000).contains(&bytes),
                "{}: {bytes}",
                p.display()
            );
            assert_eq!(bytes % 8, 0, "frame aligned");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
