//! One running capture: FFmpeg encoder process + audio engine + the reader
//! thread that splits FFmpeg's MPEG-TS output into the replay ring and the
//! session file.

use crate::ffmpeg::Ffmpeg;
use crate::session::SessionWriter;
use gc_audio::{AudioEngine, AudioSink, EngineTrack, TrackLevel};
use gc_core::config::{TrackKind, TrackSpec};
use gc_core::ffmpeg::PipelineSpec;
use gc_core::ring::{ChunkRef, ReplayRing, Store};
use gc_core::ts::TsChunker;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct AudioPlan {
    /// Container track layout (including the Master mix if any).
    pub layout: Vec<TrackSpec>,
    pub game_pid: Option<u32>,
    pub mic_device: Option<String>,
    pub system_fallback: bool,
}

pub struct RingPlan {
    pub window: Duration,
    pub store: Store,
    pub max_bytes: u64,
}

pub struct Shared {
    pub ring: Mutex<ReplayRing>,
    pub chunker: Mutex<TsChunker>,
    pub session: Mutex<Option<SessionWriter>>,
    /// Bookmarks in ring media time (seconds).
    pub bookmarks: Mutex<Vec<f64>>,
    pub chunks: AtomicU64,
    pub bytes: AtomicU64,
    pub exited: AtomicBool,
    pub stderr_tail: Mutex<VecDeque<String>>,
}

impl Shared {
    /// Current media time (ring time base).
    pub fn now_media_time(&self) -> f64 {
        self.ring.lock().media_now()
    }

    /// Ring snapshot plus the in-progress chunk, covering `secs`.
    pub fn snapshot(&self, secs: f64) -> Vec<ChunkRef> {
        let mut snap = self.ring.lock().snapshot(secs);
        if let Some(c) = self.chunker.lock().in_progress() {
            let t = snap.last().map(|l| l.t + 1.0).unwrap_or(0.0);
            snap.push(ChunkRef::owned(c.data, t));
        }
        snap
    }

    pub fn stderr_summary(&self) -> String {
        let tail = self.stderr_tail.lock();
        tail.iter()
            .rev()
            .take(6)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct Pipeline {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    audio: Option<AudioEngine>,
    pub shared: Arc<Shared>,
    pub spec: PipelineSpec,
    pub audio_titles: Vec<String>,
}

impl Pipeline {
    pub fn start(
        ff: &Ffmpeg,
        mut spec: PipelineSpec,
        audio: &AudioPlan,
        ring: RingPlan,
    ) -> anyhow::Result<Self> {
        let inputs: Vec<&TrackSpec> = audio
            .layout
            .iter()
            .filter(|t| t.kind != TrackKind::Mix)
            .collect();
        let audio_titles: Vec<String> = audio.layout.iter().map(|t| t.title.clone()).collect();

        // Named pipes must exist before FFmpeg tries to open them.
        #[allow(unused_mut)]
        let mut sinks: Vec<Box<dyn AudioSink>> = Vec::new();
        spec.audio.clear();
        #[cfg(windows)]
        {
            use gc_core::ffmpeg::AudioInput;
            use std::sync::atomic::AtomicU32;
            static NONCE: AtomicU32 = AtomicU32::new(0);
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            for (i, t) in inputs.iter().enumerate() {
                let name = format!("game-capture-{}-{nonce}-{i}", std::process::id());
                let sink = gc_audio::NamedPipeSink::create(&name)?;
                spec.audio.push(AudioInput {
                    url: sink.path.clone(),
                    in_mix: t.in_mix,
                    denoise: t.denoise,
                });
                sinks.push(Box::new(sink));
            }
        }
        let has_audio = !spec.audio.is_empty();
        let audio_titles = if has_audio { audio_titles } else { Vec::new() };
        spec.mixdown = has_audio && audio.layout.iter().any(|t| t.kind == TrackKind::Mix);

        let args = spec.build_args();
        log::info!("starting ffmpeg: {}", args.join(" "));
        let mut child = ff
            .command()
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();

        let audio_engine = if has_audio {
            let tracks = inputs
                .iter()
                .map(|t| EngineTrack {
                    title: t.title.clone(),
                    kind: t.kind.clone(),
                    volume: t.volume,
                    muted: t.muted,
                })
                .collect();
            match AudioEngine::start(
                tracks,
                audio.game_pid,
                audio.mic_device.clone(),
                audio.system_fallback,
                gc_audio::default_factory(),
                Some(sinks),
            ) {
                Ok(e) => Some(e),
                Err(e) => {
                    let _ = child.kill();
                    return Err(e);
                }
            }
        } else {
            None
        };

        let shared = Arc::new(Shared {
            ring: Mutex::new(ReplayRing::new(ring.window, ring.store, ring.max_bytes)?),
            chunker: Mutex::new(TsChunker::new()),
            session: Mutex::new(None),
            bookmarks: Mutex::new(Vec::new()),
            chunks: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            exited: AtomicBool::new(false),
            stderr_tail: Mutex::new(VecDeque::new()),
        });

        {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("gc-ffmpeg-log".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        log::warn!("ffmpeg: {line}");
                        let mut tail = shared.stderr_tail.lock();
                        tail.push_back(line);
                        if tail.len() > 50 {
                            tail.pop_front();
                        }
                    }
                })?;
        }
        let reader = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("gc-ffmpeg-reader".into())
                .spawn(move || read_loop(stdout, &shared))?
        };

        Ok(Self {
            child,
            stdin,
            reader: Some(reader),
            audio: audio_engine,
            shared,
            spec,
            audio_titles,
        })
    }

    /// Waits until the first keyframe chunk arrives (pipeline healthy) or
    /// FFmpeg exits (configuration not supported).
    pub fn wait_healthy(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.chunks.load(Ordering::SeqCst) > 0 {
                return Ok(());
            }
            if self.shared.exited.load(Ordering::SeqCst)
                || matches!(self.child.try_wait(), Ok(Some(_)))
            {
                std::thread::sleep(Duration::from_millis(100)); // let stderr drain
                anyhow::bail!("encoder exited: {}", self.shared.stderr_summary());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        anyhow::bail!(
            "no video produced within {:?}: {}",
            timeout,
            self.shared.stderr_summary()
        )
    }

    pub fn is_running(&mut self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst) && matches!(self.child.try_wait(), Ok(None))
    }

    pub fn audio_levels(&self) -> Vec<TrackLevel> {
        self.audio.as_ref().map(|a| a.levels()).unwrap_or_default()
    }

    pub fn set_volume(&self, title: &str, volume: f32, muted: bool) {
        if let Some(a) = &self.audio {
            a.set_track_by_title(title, volume, muted);
        }
    }

    /// Graceful stop: ask FFmpeg to quit (flushes the encoder), then kill
    /// if it does not exit in time. Returns the session writer, if any, so
    /// the caller can finalise it.
    pub fn stop(mut self) -> Option<SessionWriter> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        self.shared.session.lock().take()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn read_loop(mut stdout: impl Read, shared: &Shared) {
    let mut buf = vec![0u8; 256 * 1024];
    let handle_chunk = |chunk: gc_core::ts::Chunk| {
        let len = chunk.data.len() as u64;
        let mut ring = shared.ring.lock();
        let data_for_session = shared.session.lock().is_some().then(|| chunk.data.clone());
        if let Err(e) = ring.push(chunk) {
            log::error!("replay buffer write failed: {e}");
        }
        let t = ring.latest_time().map(|(t, _)| t).unwrap_or(0.0);
        drop(ring);
        if let Some(data) = data_for_session {
            if let Some(s) = shared.session.lock().as_mut() {
                if let Err(e) = s.append(&data, t) {
                    log::error!("session write failed: {e}");
                }
            }
        }
        shared.chunks.fetch_add(1, Ordering::SeqCst);
        shared.bytes.fetch_add(len, Ordering::Relaxed);
    };
    loop {
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunks = shared.chunker.lock().push(&buf[..n]);
                for c in chunks {
                    handle_chunk(c);
                }
            }
        }
    }
    if let Some(last) = shared.chunker.lock().finish() {
        handle_chunk(last);
    }
    shared.exited.store(true, Ordering::SeqCst);
}
