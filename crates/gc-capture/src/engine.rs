use crate::ffmpeg::{choose_backends, codec_label, probe_encoders, EncoderInfo, Ffmpeg};
use crate::pipeline::{AudioPlan, Pipeline, RingPlan};
use crate::screenshot::{self, ShotTarget};
use crate::session::SessionWriter;
use chrono::{DateTime, Duration as ChronoDuration, Local};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use gc_audio::TrackLevel;
use gc_core::config::{BufferStorage, Settings};
use gc_core::ffmpeg::{
    encoder_name, Backend, FfmpegCaps, FocusScreen, Output, PipelineSpec, RemuxSpec, VideoSource,
};
use gc_core::metadata::{Bookmark, ClipMetadata, MediaKind, SCHEMA_VERSION};
use gc_core::naming;
use gc_core::ring::{RingStats, Store};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct EnginePaths {
    pub ffmpeg: Option<Ffmpeg>,
    /// Scratch space (disk ring buffer, chapter files).
    pub temp_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameContext {
    pub pid: u32,
    pub exe_name: String,
    pub display_name: String,
    #[serde(skip)]
    pub hwnd: Option<isize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureState {
    /// Nothing running (no game, or replay disabled).
    Idle,
    Starting,
    /// Replay buffer running.
    Buffering,
    /// Session recording (implies buffering).
    Recording,
    Paused,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub state: CaptureState,
    pub game: Option<GameContext>,
    pub recording_secs: f64,
    pub replay_enabled: bool,
    pub buffer: Option<RingStats>,
    pub encoder: Option<String>,
    pub encoder_label: Option<String>,
    pub zero_copy: bool,
    pub output_size: Option<(u32, u32)>,
    pub fps: u32,
    pub last_error: Option<String>,
    pub ffmpeg_found: bool,
    pub pending_restart: bool,
    pub encoders_probed: bool,
    /// "Game window", "Monitor"… while capturing.
    pub capture_source: Option<String>,
    /// The "Game not in focus" screen is being recorded right now.
    pub focus_lost: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedMedia {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub game: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Status(EngineStatus),
    Saved(SavedMedia),
    Notice { error: bool, message: String },
}

enum Cmd {
    Settings(Box<Settings>),
    Game(Option<GameContext>),
    StartManual,
    StopCapture,
    SaveReplay,
    ToggleRecording,
    Bookmark,
    Screenshot,
    Pause(bool),
    ReprobeEncoders,
    Shutdown,
}

type Probe = Arc<RwLock<Option<(FfmpegCaps, Vec<EncoderInfo>)>>>;

pub struct CaptureEngine {
    tx: Sender<Cmd>,
    status: Arc<RwLock<EngineStatus>>,
    pipeline: Arc<Mutex<Option<Pipeline>>>,
    probe: Probe,
    thread: Option<JoinHandle<()>>,
}

impl CaptureEngine {
    pub fn new(paths: EnginePaths, settings: Settings, events: Sender<EngineEvent>) -> Self {
        let (tx, rx) = unbounded();
        let status = Arc::new(RwLock::new(EngineStatus {
            state: CaptureState::Idle,
            game: None,
            recording_secs: 0.0,
            replay_enabled: settings.replay.enabled,
            buffer: None,
            encoder: None,
            encoder_label: None,
            zero_copy: false,
            output_size: None,
            fps: settings.video.fps.0,
            last_error: None,
            ffmpeg_found: paths.ffmpeg.is_some(),
            pending_restart: false,
            encoders_probed: false,
            capture_source: None,
            focus_lost: false,
        }));
        let pipeline: Arc<Mutex<Option<Pipeline>>> = Arc::default();
        let probe: Probe = Arc::default();
        let worker = Worker {
            ff: paths.ffmpeg,
            temp_dir: paths.temp_dir,
            settings,
            pending: None,
            game: None,
            manual: false,
            paused: false,
            status: status.clone(),
            pipeline: pipeline.clone(),
            probe: probe.clone(),
            events,
            last_error: None,
            restart_backoff: Duration::from_secs(2),
            next_restart: None,
            current_encoder: None,
            shutting_down: false,
            degraded: false,
            capture_label: None,
        };
        let thread = std::thread::Builder::new()
            .name("gc-capture-engine".into())
            .spawn(move || worker.run(rx))
            .expect("spawn engine thread");
        Self {
            tx,
            status,
            pipeline,
            probe,
            thread: Some(thread),
        }
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn update_settings(&self, s: Settings) {
        self.send(Cmd::Settings(Box::new(s)));
    }
    pub fn set_game(&self, g: Option<GameContext>) {
        self.send(Cmd::Game(g));
    }
    /// Starts the buffer without a detected game (desktop capture).
    pub fn start_manual(&self) {
        self.send(Cmd::StartManual);
    }
    pub fn stop_capture(&self) {
        self.send(Cmd::StopCapture);
    }
    pub fn save_replay(&self) {
        self.send(Cmd::SaveReplay);
    }
    pub fn toggle_recording(&self) {
        self.send(Cmd::ToggleRecording);
    }
    pub fn add_bookmark(&self) {
        self.send(Cmd::Bookmark);
    }
    pub fn screenshot(&self) {
        self.send(Cmd::Screenshot);
    }
    pub fn set_paused(&self, p: bool) {
        self.send(Cmd::Pause(p));
    }
    pub fn reprobe_encoders(&self) {
        self.send(Cmd::ReprobeEncoders);
    }

    pub fn status(&self) -> EngineStatus {
        self.status.read().clone()
    }

    pub fn encoders(&self) -> Vec<EncoderInfo> {
        self.probe
            .read()
            .as_ref()
            .map(|(_, e)| e.clone())
            .unwrap_or_default()
    }

    pub fn audio_levels(&self) -> Vec<TrackLevel> {
        self.pipeline
            .lock()
            .as_ref()
            .map(|p| p.audio_levels())
            .unwrap_or_default()
    }

    /// Live volume change; also persisted by the caller in settings.
    pub fn set_volume(&self, track_title: &str, volume: f32, muted: bool) {
        if let Some(p) = self.pipeline.lock().as_ref() {
            p.set_volume(track_title, volume, muted);
        }
    }
}

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        self.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Worker {
    ff: Option<Ffmpeg>,
    temp_dir: PathBuf,
    settings: Settings,
    /// Settings waiting for the current recording to finish.
    pending: Option<Box<Settings>>,
    game: Option<GameContext>,
    manual: bool,
    paused: bool,
    status: Arc<RwLock<EngineStatus>>,
    pipeline: Arc<Mutex<Option<Pipeline>>>,
    probe: Probe,
    events: Sender<EngineEvent>,
    last_error: Option<String>,
    restart_backoff: Duration,
    next_restart: Option<Instant>,
    current_encoder: Option<(Backend, gc_core::config::Codec)>,
    /// During shutdown sessions are finalised synchronously so the process
    /// does not exit mid-remux.
    shutting_down: bool,
    /// Window capture / focus screen stalled once for this game: use the
    /// plain monitor capture until the game changes.
    degraded: bool,
    capture_label: Option<&'static str>,
}

struct Target {
    video: VideoSource,
    size: (u32, u32),
    shot: Option<ShotTarget>,
    label: &'static str,
    /// The focus screen makes sense (a game is being captured).
    focus_ok: bool,
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        self.spawn_probe();
        self.publish();
        loop {
            match rx.recv_timeout(Duration::from_millis(1000)) {
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => self.tick(),
            }
            self.publish();
        }
        self.shutting_down = true;
        self.stop_pipeline(true);
    }

    fn notice(&self, error: bool, message: impl Into<String>) {
        let message = message.into();
        if error {
            log::warn!("{message}");
        } else {
            log::info!("{message}");
        }
        let _ = self.events.send(EngineEvent::Notice { error, message });
    }

    fn spawn_probe(&self) {
        let Some(ff) = self.ff.clone() else { return };
        let probe = self.probe.clone();
        let events = self.events.clone();
        let status = self.status.clone();
        std::thread::Builder::new()
            .name("gc-encoder-probe".into())
            .spawn(move || {
                let caps = ff.caps();
                let enc = probe_encoders(&ff, &caps);
                log::info!(
                    "working encoders: {:?}",
                    enc.iter()
                        .filter(|e| e.working)
                        .map(|e| &e.name)
                        .collect::<Vec<_>>()
                );
                *probe.write() = Some((caps, enc));
                let mut s = status.write();
                s.encoders_probed = true;
                let _ = events.send(EngineEvent::Status(s.clone()));
            })
            .ok();
    }

    fn wants_pipeline(&self) -> bool {
        !self.paused
            && (self.manual
                || (self.game.is_some()
                    && self.settings.replay.enabled
                    && self.settings.replay.auto_start))
    }

    fn is_recording(&self) -> bool {
        self.pipeline
            .lock()
            .as_ref()
            .is_some_and(|p| p.shared.session.lock().is_some())
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Settings(s) => self.apply_settings(s),
            Cmd::Game(g) => {
                let changed = g.as_ref().map(|x| x.pid) != self.game.as_ref().map(|x| x.pid);
                self.game = g;
                if !changed {
                    return;
                }
                self.degraded = false;
                if self.is_recording() {
                    if self.game.is_none() {
                        // Game closed: finish the session with it.
                        self.stop_pipeline(true);
                        if self.wants_pipeline() {
                            self.start_pipeline();
                        }
                    }
                    return;
                }
                self.stop_pipeline(true);
                if self.wants_pipeline() {
                    self.start_pipeline();
                }
            }
            Cmd::StartManual => {
                self.manual = true;
                self.paused = false;
                if self.pipeline.lock().is_none() {
                    self.start_pipeline();
                }
            }
            Cmd::StopCapture => {
                self.manual = false;
                self.stop_pipeline(true);
            }
            Cmd::SaveReplay => self.save_replay(),
            Cmd::ToggleRecording => self.toggle_recording(),
            Cmd::Bookmark => self.bookmark(),
            Cmd::Screenshot => self.screenshot(),
            Cmd::Pause(p) => {
                self.paused = p;
                if p {
                    self.stop_pipeline(true);
                    self.notice(false, "Capture paused");
                } else {
                    self.notice(false, "Capture resumed");
                    if self.wants_pipeline() {
                        self.start_pipeline();
                    }
                }
            }
            Cmd::ReprobeEncoders => {
                *self.probe.write() = None;
                self.status.write().encoders_probed = false;
                self.spawn_probe();
            }
            Cmd::Shutdown => {}
        }
    }

    fn apply_settings(&mut self, s: Box<Settings>) {
        let restart = self.settings.pipeline_differs(&s)
            || self.settings.replay.duration_secs != s.replay.duration_secs;
        if restart && self.is_recording() {
            self.pending = Some(s);
            self.notice(
                false,
                "New capture settings will apply when the recording stops",
            );
            return;
        }
        // Live volume changes.
        if let Some(p) = self.pipeline.lock().as_ref() {
            for t in s.audio.input_tracks() {
                p.set_volume(&t.title, t.volume, t.muted);
                p.set_gate(&t.title, t.gate_db);
            }
        }
        self.settings = *s;
        if restart {
            self.stop_pipeline(true);
        }
        if self.wants_pipeline() && self.pipeline.lock().is_none() {
            self.start_pipeline();
        } else if !self.wants_pipeline() && !self.is_recording() {
            self.stop_pipeline(true);
        }
    }

    fn tick(&mut self) {
        let dead = {
            let mut guard = self.pipeline.lock();
            match guard.as_mut() {
                Some(p) if p.shared.stalled(Duration::from_secs(8)) => {
                    // Running but producing nothing (typically a window that
                    // can't be captured): retry with plain monitor capture.
                    if p.spec.focus_screen.is_some()
                        || matches!(p.spec.video, VideoSource::Window { .. })
                    {
                        self.degraded = true;
                    }
                    Some(format!("no video for 8 s. {}", p.shared.stderr_summary()))
                }
                Some(p) => (!p.is_running()).then(|| p.shared.stderr_summary()),
                None => None,
            }
        };
        if let Some(err) = dead {
            self.last_error = Some(format!("Capture stopped unexpectedly: {err}"));
            self.notice(true, self.last_error.clone().unwrap_or_default());
            self.stop_pipeline(true);
            self.next_restart = Some(Instant::now() + self.restart_backoff);
            self.restart_backoff = (self.restart_backoff * 2).min(Duration::from_secs(60));
        }
        if self.pipeline.lock().is_none()
            && self.wants_pipeline()
            && self.next_restart.is_none_or(|t| Instant::now() >= t)
        {
            self.start_pipeline();
        }
    }

    /// Capture sources to try, best first: the game window itself (Windows
    /// Graphics Capture), then the monitor it is on, then (dev hosts) a test
    /// pattern.
    fn resolve_targets(&self, caps: &FfmpegCaps) -> Vec<Target> {
        let mut out = Vec::new();
        #[cfg(windows)]
        {
            use gc_core::config::CaptureMode;
            use gc_platform::display::{list_monitors, window_client_size, window_placement};
            use gc_platform::process::main_window;
            let draw_mouse = self.settings.video.capture_cursor;
            if let Some(g) = &self.game {
                let hwnd = g.hwnd.or_else(|| main_window(g.pid));
                let placement = hwnd.and_then(window_placement);
                let shot = placement.as_ref().map(|pl| ShotTarget {
                    adapter_idx: pl.monitor.adapter_idx,
                    output_idx: pl.monitor.output_idx,
                    region: (!pl.fullscreen).then_some(pl.rect),
                });
                if self.settings.video.capture_mode == CaptureMode::Window
                    && !self.degraded
                    && caps.has_filter("gfxcapture")
                {
                    if let Some((h, (w, ht))) =
                        hwnd.and_then(|h| window_client_size(h).map(|s| (h, s)))
                    {
                        out.push(Target {
                            video: VideoSource::Window {
                                hwnd: h as u64,
                                draw_mouse,
                                adapter_idx: None,
                            },
                            size: (w & !1, ht & !1),
                            shot,
                            label: "Game window",
                            focus_ok: true,
                        });
                    }
                }
                if let Some(pl) = placement {
                    let region = (!pl.fullscreen).then_some(pl.rect);
                    let size = region
                        .map(|r| (r.2, r.3))
                        .unwrap_or((pl.monitor.width, pl.monitor.height));
                    out.push(Target {
                        video: VideoSource::Ddagrab {
                            adapter_idx: (pl.monitor.adapter_idx != 0)
                                .then_some(pl.monitor.adapter_idx),
                            output_idx: pl.monitor.output_idx,
                            draw_mouse,
                            region,
                        },
                        size,
                        shot,
                        label: "Monitor",
                        focus_ok: true,
                    });
                    return out;
                }
            }
            let monitors = list_monitors();
            let m = monitors
                .get(self.settings.video.monitor_index as usize)
                .or_else(|| monitors.iter().find(|m| m.primary))
                .or(monitors.first());
            if let Some(m) = m {
                out.push(Target {
                    video: VideoSource::Ddagrab {
                        adapter_idx: (m.adapter_idx != 0).then_some(m.adapter_idx),
                        output_idx: m.output_idx,
                        draw_mouse,
                        region: None,
                    },
                    size: (m.width, m.height),
                    shot: Some(ShotTarget {
                        adapter_idx: m.adapter_idx,
                        output_idx: m.output_idx,
                        region: None,
                    }),
                    label: "Monitor",
                    focus_ok: self.game.is_some(),
                });
                return out;
            }
        }
        let _ = caps;
        // Development hosts without Desktop Duplication: synthetic video.
        out.push(Target {
            video: VideoSource::TestPattern,
            size: (1280, 720),
            shot: None,
            label: "Test pattern",
            focus_ok: false,
        });
        out
    }

    /// The screenshot target for the current game/monitor.
    fn shot_target(&self) -> Option<ShotTarget> {
        self.resolve_targets(&FfmpegCaps::default())
            .into_iter()
            .find_map(|t| t.shot)
    }

    /// TTF used for the focus screen caption (`None` = plain screen).
    fn focus_font(caps: &FfmpegCaps) -> Option<String> {
        if !caps.has_filter("drawtext") {
            return None;
        }
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        ["segoeui.ttf", "arial.ttf"]
            .iter()
            .map(|f| std::path::Path::new(&windir).join("Fonts").join(f))
            .find(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
    }

    fn start_pipeline(&mut self) {
        let Some(ff) = self.ff.clone() else {
            self.last_error = Some("FFmpeg is missing; capture is unavailable".into());
            return;
        };
        self.status.write().state = CaptureState::Starting;
        self.publish();
        // Wait (bounded) for the encoder probe so the first start picks a
        // working hardware encoder.
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.probe.read().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let (caps, probed) = self.probe.read().clone().unwrap_or_default();

        let s = self.settings.clone();
        let temp_dir = self.temp_dir.clone();
        let targets = self.resolve_targets(&caps);
        let font = Self::focus_font(&caps);
        let layout = s.audio.track_layout();
        let audio = AudioPlan {
            layout,
            game_pid: self.game.as_ref().map(|g| g.pid),
            mic_device: s.audio.mic.device_id.clone(),
            system_fallback: s
                .audio
                .system_audio_fallback
                .unwrap_or(!gc_platform::system::supports_process_loopback()),
        };
        let replay_secs = if s.replay.enabled {
            s.replay.duration_secs
        } else {
            3
        };
        let storage = s.replay.effective_storage(s.video.peak_kbps());
        let ring_plan = || RingPlan {
            window: Duration::from_secs(replay_secs as u64 + 2),
            store: match storage {
                BufferStorage::Disk => Store::Disk(
                    s.replay
                        .disk_dir
                        .clone()
                        .unwrap_or_else(|| temp_dir.clone()),
                ),
                _ => Store::Memory,
            },
            max_bytes: if storage == BufferStorage::Disk {
                u64::MAX
            } else {
                s.replay.max_memory_mb as u64 * 1024 * 1024
            },
        };

        let mut errors = Vec::new();
        let order = choose_backends(s.video.encoder, s.video.codec, &probed);
        let game_pid = self.game.as_ref().map(|g| g.pid);
        for &(backend, codec) in &order {
            let zc_variants: &[bool] = if s.video.zero_copy && backend.is_hardware() {
                &[true, false]
            } else {
                &[false]
            };
            for &zero_copy in zc_variants {
                for target in &targets {
                    let focus_variants: &[bool] = if s.video.focus_screen
                        && target.focus_ok
                        && !self.degraded
                        && game_pid.is_some()
                    {
                        &[true, false]
                    } else {
                        &[false]
                    };
                    for &focus in focus_variants {
                        let spec = PipelineSpec {
                            video: target.video.clone(),
                            source_size: target.size,
                            fps: s.video.fps,
                            resolution: s.video.resolution,
                            backend,
                            codec,
                            rate: s.video.rate_control,
                            zero_copy,
                            gpu_scaler: caps.gpu_scaler(backend),
                            has_scale_d3d11: caps.has_filter("scale_d3d11"),
                            focus_screen: focus.then(|| FocusScreen {
                                font_file: font.clone(),
                            }),
                            audio: Vec::new(),
                            mixdown: s.audio.mixdown_enabled,
                            audio_bitrate_kbps: 160,
                            output: Output::StdoutTs,
                        };
                        if zero_copy && !spec.is_zero_copy() {
                            continue; // identical to the download variant
                        }
                        let started = Pipeline::start(&ff, spec, &audio, ring_plan(), game_pid)
                            .and_then(|mut p| {
                                p.wait_healthy(Duration::from_secs(8))?;
                                Ok(p)
                            });
                        let name = encoder_name(backend, codec);
                        match started {
                            Ok(p) => {
                                log::info!(
                                    "capture running: {name} from {} {}x{}, zero-copy: {}, focus screen: {focus}",
                                    target.label,
                                    target.size.0,
                                    target.size.1,
                                    p.spec.is_zero_copy()
                                );
                                if (backend, codec) != order[0]
                                    && self.current_encoder != Some((backend, codec))
                                {
                                    self.notice(false, format!("Using fallback encoder {name}"));
                                }
                                self.current_encoder = Some((backend, codec));
                                self.capture_label = Some(target.label);
                                *self.pipeline.lock() = Some(p);
                                self.last_error = None;
                                self.restart_backoff = Duration::from_secs(2);
                                self.next_restart = None;
                                return;
                            }
                            Err(e) => {
                                log::warn!(
                                    "{name} from {} (zero-copy {zero_copy}, focus {focus}) failed: {e}",
                                    target.label
                                );
                                errors.push(format!("{name}: {e}"));
                            }
                        }
                    }
                }
            }
        }
        self.last_error = Some(format!(
            "Could not start capture. {}",
            errors.last().cloned().unwrap_or_default()
        ));
        self.notice(true, self.last_error.clone().unwrap_or_default());
        self.next_restart = Some(Instant::now() + self.restart_backoff);
        self.restart_backoff = (self.restart_backoff * 2).min(Duration::from_secs(60));
    }

    /// Stops the pipeline; finalises any session in the background.
    fn stop_pipeline(&mut self, finalize: bool) {
        let p = self.pipeline.lock().take();
        if let Some(p) = p {
            if let Some(session) = p.stop() {
                if finalize {
                    self.finish_session(session);
                }
            }
        }
        if let Some(s) = self.pending.take() {
            self.settings = *s;
        }
    }

    fn finish_session(&self, session: SessionWriter) {
        let Some(ff) = self.ff.clone() else { return };
        let events = self.events.clone();
        let game = session.info.meta.game_title.clone();
        let job = move || match session.finish(&ff) {
            Ok(path) => {
                let _ = events.send(EngineEvent::Saved(SavedMedia {
                    path,
                    kind: MediaKind::Session,
                    game,
                }));
            }
            Err(e) => {
                let _ = events.send(EngineEvent::Notice {
                    error: true,
                    message: format!("Saving the recording failed: {e}"),
                });
            }
        };
        if self.shutting_down {
            job();
        } else {
            std::thread::spawn(job);
        }
    }

    fn base_meta(&self, kind: MediaKind, start: DateTime<Local>, p: &Pipeline) -> ClipMetadata {
        let (w, h) = p.spec.output_size();
        ClipMetadata {
            version: SCHEMA_VERSION,
            kind,
            game_title: self.game.as_ref().map(|g| g.display_name.clone()),
            executable_name: self.game.as_ref().map(|g| g.exe_name.clone()),
            timestamp: start.to_rfc3339(),
            duration_ms: 0,
            bookmarks: Vec::new(),
            resolution: Some(format!("{w}x{h}")),
            encoder: Some(encoder_name(p.spec.backend, p.spec.codec).to_string()),
            audio_tracks: p.audio_titles.clone(),
        }
    }

    fn save_replay(&mut self) {
        if !self.settings.replay.enabled {
            self.notice(true, "Replay buffer is disabled in settings");
            return;
        }
        let Some(ff) = self.ff.clone() else { return };
        let guard = self.pipeline.lock();
        let Some(p) = guard.as_ref() else {
            drop(guard);
            self.notice(true, "Replay buffer is not running (no game detected). Start capture from the tray or dashboard.");
            return;
        };
        let secs = self.settings.replay.duration_secs as f64;
        let snap = p.shared.snapshot(secs);
        if snap.is_empty() {
            drop(guard);
            self.notice(true, "Replay buffer is still empty");
            return;
        }
        let t0 = snap[0].t;
        let end = p.shared.now_media_time().max(t0);
        let span = end - t0;
        let now = Local::now();
        let start = now - ChronoDuration::milliseconds((span * 1000.0) as i64);
        let mut meta = self.base_meta(MediaKind::Clip, start, p);
        meta.duration_ms = (span * 1000.0) as u64;
        meta.bookmarks = p
            .shared
            .bookmarks
            .lock()
            .iter()
            .filter(|b| **b >= t0 && **b <= end)
            .map(|b| Bookmark {
                offset_ms: ((b - t0) * 1000.0) as u64,
                label: None,
            })
            .collect();
        drop(guard);

        let ext = self.settings.video.container.extension();
        let out = naming::output_path(
            &self.settings.storage_root,
            meta.game_title.as_deref(),
            now,
            MediaKind::Clip,
            ext,
        );
        let temp_dir = self.temp_dir.clone();
        let events = self.events.clone();
        let game = meta.game_title.clone();
        self.notice(false, format!("Saving last {} s…", secs as u32));
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                if let Some(d) = out.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::create_dir_all(&temp_dir)?;
                let chapters = temp_dir.join(format!("chapters-{}.txt", now.timestamp_millis()));
                let chapters_file = if meta.bookmarks.is_empty() {
                    None
                } else {
                    std::fs::write(&chapters, meta.ffmetadata_chapters())?;
                    Some(chapters.to_string_lossy().into_owned())
                };
                let spec = RemuxSpec {
                    tags: meta.to_tags(),
                    audio_titles: meta.audio_tracks.clone(),
                    chapters_file,
                };
                let child = ff.spawn_remux(&out, &spec)?;
                let r = Ffmpeg::finish_remux(child, |w| {
                    for c in &snap {
                        c.write_to(w)?;
                    }
                    Ok(())
                });
                let _ = std::fs::remove_file(&chapters);
                r
            })();
            match result {
                Ok(()) => {
                    let _ = events.send(EngineEvent::Saved(SavedMedia {
                        path: out,
                        kind: MediaKind::Clip,
                        game,
                    }));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&out);
                    let _ = events.send(EngineEvent::Notice {
                        error: true,
                        message: format!("Saving the replay failed: {e}"),
                    });
                }
            }
        });
    }

    fn toggle_recording(&mut self) {
        // Stop?
        let existing = self
            .pipeline
            .lock()
            .as_ref()
            .and_then(|p| p.shared.session.lock().take());
        if let Some(session) = existing {
            self.notice(false, "Recording stopped – saving…");
            self.finish_session(session);
            if let Some(s) = self.pending.take() {
                self.apply_settings(s);
            }
            if !self.wants_pipeline() {
                self.stop_pipeline(true);
            }
            return;
        }
        // Start (capture desktop if no game is running).
        if self.pipeline.lock().is_none() {
            self.manual = self.game.is_none() || self.manual;
            self.start_pipeline();
        }
        let guard = self.pipeline.lock();
        let Some(p) = guard.as_ref() else {
            drop(guard);
            self.notice(true, "Cannot record: capture failed to start");
            return;
        };
        let now = Local::now();
        let meta = self.base_meta(MediaKind::Session, now, p);
        let ext = self.settings.video.container.extension();
        let out = naming::output_path(
            &self.settings.storage_root,
            meta.game_title.as_deref(),
            now,
            MediaKind::Session,
            ext,
        );
        match SessionWriter::create(out, meta, p.audio_titles.clone()) {
            Ok(w) => {
                *p.shared.session.lock() = Some(w);
                drop(guard);
                self.notice(false, "Recording started");
            }
            Err(e) => {
                drop(guard);
                self.notice(true, format!("Cannot start recording: {e}"));
            }
        }
    }

    fn bookmark(&mut self) {
        let guard = self.pipeline.lock();
        let Some(p) = guard.as_ref() else {
            drop(guard);
            self.notice(true, "Nothing is being captured – bookmark ignored");
            return;
        };
        let now_t = p.shared.now_media_time();
        p.shared.bookmarks.lock().push(now_t);
        let offset = p
            .shared
            .session
            .lock()
            .as_mut()
            .and_then(|s| s.add_bookmark(now_t, None).ok());
        drop(guard);
        match offset {
            Some(ms) => {
                let s = ms / 1000;
                self.notice(
                    false,
                    format!(
                        "Bookmark added at {:02}:{:02}:{:02}",
                        s / 3600,
                        s / 60 % 60,
                        s % 60
                    ),
                );
            }
            None => self.notice(false, "Bookmark added"),
        }
    }

    fn screenshot(&mut self) {
        let Some(shot) = self.shot_target() else {
            self.notice(true, "Screenshots need Windows Desktop Duplication");
            return;
        };
        let fmt = self.settings.screenshot.format;
        let quality = self.settings.screenshot.jpeg_quality;
        let game = self.game.as_ref().map(|g| g.display_name.clone());
        let out = naming::output_path(
            &self.settings.storage_root,
            game.as_deref(),
            Local::now(),
            MediaKind::Screenshot,
            fmt.extension(),
        );
        let events = self.events.clone();
        std::thread::spawn(
            move || match screenshot::capture(shot, fmt, quality, &out) {
                Ok(_) => {
                    let _ = events.send(EngineEvent::Saved(SavedMedia {
                        path: out,
                        kind: MediaKind::Screenshot,
                        game,
                    }));
                }
                Err(e) => {
                    let _ = events.send(EngineEvent::Notice {
                        error: true,
                        message: format!("Screenshot failed: {e}"),
                    });
                }
            },
        );
    }

    fn publish(&self) {
        let st = {
            let guard = self.pipeline.lock();
            let mut s = self.status.write();
            s.game = self.game.clone();
            s.replay_enabled = self.settings.replay.enabled;
            s.fps = self.settings.video.fps.0;
            s.last_error = self.last_error.clone();
            s.ffmpeg_found = self.ff.is_some();
            s.pending_restart = self.pending.is_some();
            match guard.as_ref() {
                Some(p) => {
                    let now_t = p.shared.now_media_time();
                    let session = p.shared.session.lock();
                    s.recording_secs = session.as_ref().map(|w| w.elapsed(now_t)).unwrap_or(0.0);
                    s.state = if session.is_some() {
                        CaptureState::Recording
                    } else {
                        CaptureState::Buffering
                    };
                    drop(session);
                    s.buffer = Some(p.shared.ring.lock().stats());
                    s.encoder = Some(encoder_name(p.spec.backend, p.spec.codec).to_string());
                    s.encoder_label = Some(format!(
                        "{} {}",
                        p.spec.backend.label(),
                        codec_label(p.spec.codec)
                    ));
                    s.zero_copy = p.spec.is_zero_copy();
                    s.output_size = Some(p.spec.output_size());
                    s.capture_source = self.capture_label.map(String::from);
                    s.focus_lost = p.focus_lost();
                }
                None => {
                    s.recording_secs = 0.0;
                    s.buffer = None;
                    s.encoder = None;
                    s.encoder_label = None;
                    s.zero_copy = false;
                    s.output_size = None;
                    s.capture_source = None;
                    s.focus_lost = false;
                    s.state = if self.paused {
                        CaptureState::Paused
                    } else if self.last_error.is_some() && self.wants_pipeline() {
                        CaptureState::Error
                    } else {
                        CaptureState::Idle
                    };
                }
            }
            s.clone()
        };
        let _ = self.events.send(EngineEvent::Status(st));
    }
}
