use crate::ffmpeg::{choose_backends, codec_label, probe_encoders, EncoderInfo, Ffmpeg};
use crate::pipeline::{AudioPlan, Pipeline, RingPlan};
use crate::screenshot::{self, ShotTarget};
use crate::session::SessionWriter;
use chrono::{DateTime, Duration as ChronoDuration, Local};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use gc_audio::{AudioEngine, EngineTrack, TrackLevel};
use gc_core::config::{BufferStorage, Quality, Settings, TrackKind};
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
    /// Session recording (with or without the replay buffer).
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
    /// Capturing without a game is allowed by the settings.
    pub desktop_capture: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedMedia {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub game: Option<String>,
}

/// Kinds of in-game popups (each has its own chime).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ToastKind {
    /// Capture is ready for a game that just started.
    Ready,
    Saved,
    RecordStart,
    RecordStop,
    Bookmark,
    Screenshot,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Status(EngineStatus),
    Saved(SavedMedia),
    Notice {
        error: bool,
        message: String,
    },
    /// An in-game popup.
    Toast(Toast),
}

fn toast(kind: ToastKind, title: impl Into<String>, detail: Option<String>) -> EngineEvent {
    EngineEvent::Toast(Toast {
        kind,
        title: title.into(),
        detail,
    })
}

/// `125` → `2:05`.
pub fn clock(secs: f64) -> String {
    let s = secs.round().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
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
type Slot = Arc<Mutex<Option<Pipeline>>>;

/// Meter-only audio engine for the settings page while nothing is captured.
struct Monitor {
    engine: AudioEngine,
    last_poll: Instant,
    key: MonitorKey,
}

#[derive(Clone, PartialEq)]
struct MonitorKey {
    tracks: Vec<(String, TrackKind)>,
    mic: Option<String>,
    game_pid: Option<u32>,
}

#[derive(Clone)]
struct MonitorCfg {
    settings: Settings,
    game_pid: Option<u32>,
}

impl MonitorCfg {
    fn key(&self) -> MonitorKey {
        MonitorKey {
            tracks: self
                .settings
                .audio
                .input_tracks()
                .into_iter()
                .map(|t| (t.title, t.kind))
                .collect(),
            mic: self.settings.audio.mic.device_id.clone(),
            game_pid: self.game_pid,
        }
    }
}

const MONITOR_IDLE: Duration = Duration::from_secs(3);

pub struct CaptureEngine {
    tx: Sender<Cmd>,
    status: Arc<RwLock<EngineStatus>>,
    replay: Slot,
    recorder: Slot,
    probe: Probe,
    monitor: Arc<Mutex<Option<Monitor>>>,
    monitor_cfg: Arc<RwLock<MonitorCfg>>,
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
            fps: settings.replay.quality.fps.0,
            last_error: None,
            ffmpeg_found: paths.ffmpeg.is_some(),
            pending_restart: false,
            encoders_probed: false,
            capture_source: None,
            focus_lost: false,
            desktop_capture: settings.video.allow_desktop_capture,
        }));
        let replay: Slot = Arc::default();
        let recorder: Slot = Arc::default();
        let probe: Probe = Arc::default();
        let monitor: Arc<Mutex<Option<Monitor>>> = Arc::default();
        let monitor_cfg = Arc::new(RwLock::new(MonitorCfg {
            settings: settings.clone(),
            game_pid: None,
        }));
        let worker = Worker {
            ff: paths.ffmpeg,
            temp_dir: paths.temp_dir,
            settings,
            pending: None,
            game: None,
            manual: false,
            paused: false,
            status: status.clone(),
            replay: replay.clone(),
            recorder: recorder.clone(),
            probe: probe.clone(),
            monitor: monitor.clone(),
            events,
            last_error: None,
            restart_backoff: Duration::from_secs(2),
            next_restart: None,
            current_encoder: None,
            shutting_down: false,
            degraded: false,
            capture_label: None,
            replay_quality: None,
            announced: None,
            capture_window: None,
            window_change: None,
        };
        let thread = std::thread::Builder::new()
            .name("gc-capture-engine".into())
            .spawn(move || worker.run(rx))
            .expect("spawn engine thread");
        Self {
            tx,
            status,
            replay,
            recorder,
            probe,
            monitor,
            monitor_cfg,
            thread: Some(thread),
        }
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn update_settings(&self, s: Settings) {
        {
            let mut cfg = self.monitor_cfg.write();
            cfg.settings = s.clone();
            let key = cfg.key();
            let mut m = self.monitor.lock();
            match m.as_ref() {
                Some(mon) if mon.key == key => {
                    for t in s.audio.input_tracks() {
                        mon.engine.set_track_by_title(&t.title, t.volume, t.muted);
                        mon.engine.set_gate_by_title(&t.title, t.gate_db);
                    }
                }
                Some(_) => *m = None,
                None => {}
            }
        }
        self.send(Cmd::Settings(Box::new(s)));
    }
    pub fn set_game(&self, g: Option<GameContext>) {
        self.monitor_cfg.write().game_pid = g.as_ref().map(|g| g.pid);
        if let Some(m) = self.monitor.lock().as_ref() {
            m.engine.set_game_pid(g.as_ref().map(|g| g.pid));
        }
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

    /// Live levels. While capturing they come from the capture itself;
    /// otherwise a meter-only audio engine is started on demand (and stopped
    /// a few seconds after the last call), so levels can be checked in the
    /// settings without capturing anything.
    pub fn audio_levels(&self) -> Vec<TrackLevel> {
        for slot in [&self.replay, &self.recorder] {
            if let Some(p) = slot.lock().as_ref() {
                let l = p.audio_levels();
                if !l.is_empty() {
                    return l;
                }
            }
        }
        let cfg = self.monitor_cfg.read().clone();
        let key = cfg.key();
        let mut m = self.monitor.lock();
        if m.as_ref().is_some_and(|mon| mon.key != key) {
            *m = None;
        }
        if m.is_none() {
            let tracks: Vec<EngineTrack> = cfg
                .settings
                .audio
                .input_tracks()
                .into_iter()
                .map(|t| EngineTrack {
                    title: t.title,
                    kind: t.kind,
                    volume: t.volume,
                    muted: t.muted,
                    gate_db: t.gate_db,
                })
                .collect();
            if tracks.is_empty() {
                return Vec::new();
            }
            let fallback = cfg
                .settings
                .audio
                .system_audio_fallback
                .unwrap_or(!gc_platform::system::supports_process_loopback());
            match AudioEngine::start(
                tracks,
                cfg.game_pid,
                cfg.settings.audio.mic.device_id.clone(),
                fallback,
                gc_audio::default_factory(),
                None,
            ) {
                Ok(engine) => {
                    *m = Some(Monitor {
                        engine,
                        last_poll: Instant::now(),
                        key,
                    })
                }
                Err(e) => {
                    log::debug!("audio monitor: {e}");
                    return Vec::new();
                }
            }
        }
        let mon = m.as_mut().expect("just created");
        mon.last_poll = Instant::now();
        mon.engine.levels()
    }

    /// Live volume change; also persisted by the caller in settings.
    pub fn set_volume(&self, track_title: &str, volume: f32, muted: bool) {
        for slot in [&self.replay, &self.recorder] {
            if let Some(p) = slot.lock().as_ref() {
                p.set_volume(track_title, volume, muted);
            }
        }
        if let Some(m) = self.monitor.lock().as_ref() {
            m.engine.set_track_by_title(track_title, volume, muted);
        }
    }
}

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        self.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.monitor.lock().take();
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
    /// Replay buffer pipeline (clip quality).
    replay: Slot,
    /// Separate pipeline for a recording whose quality differs from the
    /// clip quality (or when the replay buffer is off).
    recorder: Slot,
    probe: Probe,
    monitor: Arc<Mutex<Option<Monitor>>>,
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
    /// Quality the running replay pipeline was started with.
    replay_quality: Option<Quality>,
    /// Game (pid) the "capture ready" popup was shown for.
    announced: Option<u32>,
    /// Game window the running pipeline was started for.
    capture_window: Option<GameWindow>,
    /// A different game window seen by consecutive ticks, and how many.
    window_change: Option<(GameWindow, u8)>,
}

/// A game's main window: handle, client size and, when capture follows the
/// window's position on the monitor (Desktop Duplication), its rectangle.
type GameWindow = (isize, (u32, u32), Option<(i32, i32, u32, u32)>);

/// Consecutive 1 s ticks a new game window must persist before capture
/// switches to it (rides out loading screens and mode switches).
const WINDOW_SWITCH_TICKS: u8 = 3;

struct Target {
    video: VideoSource,
    size: (u32, u32),
    shot: Option<ShotTarget>,
    label: &'static str,
    /// The focus screen makes sense (a game is being captured).
    focus_ok: bool,
}

/// Where the active recording lives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    Replay,
    Recorder,
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

    fn emit(&self, ev: EngineEvent) {
        let _ = self.events.send(ev);
    }

    fn notice(&self, error: bool, message: impl Into<String>) {
        let message = message.into();
        if error {
            log::warn!("{message}");
        } else {
            log::info!("{message}");
        }
        self.emit(EngineEvent::Notice { error, message });
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

    fn desktop_allowed(&self) -> bool {
        self.settings.video.allow_desktop_capture
    }

    /// Whether the replay buffer pipeline should be running.
    fn wants_pipeline(&self) -> bool {
        !self.paused
            && self.settings.replay.enabled
            && ((self.manual && self.desktop_allowed())
                || (self.game.is_some() && self.settings.replay.auto_start))
    }

    fn host(&self) -> Option<Host> {
        let has = |slot: &Slot| {
            slot.lock()
                .as_ref()
                .is_some_and(|p| p.shared.session.lock().is_some())
        };
        if has(&self.recorder) {
            Some(Host::Recorder)
        } else if has(&self.replay) {
            Some(Host::Replay)
        } else {
            None
        }
    }

    fn is_recording(&self) -> bool {
        self.host().is_some()
    }

    /// Refuses an action outside games unless desktop capture is enabled.
    fn check_capture_allowed(&self, what: &str) -> bool {
        if self.game.is_some() || self.desktop_allowed() {
            return true;
        }
        self.notice(
            true,
            format!("No game is running – {what} is off outside games (Settings › Recording › Desktop capture)."),
        );
        false
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Settings(s) => self.apply_settings(s),
            Cmd::Game(g) => {
                let changed = g.as_ref().map(|x| x.pid) != self.game.as_ref().map(|x| x.pid);
                let ended = g.is_none();
                self.game = g;
                if !changed {
                    return;
                }
                self.degraded = false;
                self.window_change = None;
                if self.is_recording() && !ended {
                    // Switched games mid-recording: keep recording.
                    return;
                }
                self.stop_pipeline(true);
                if ended {
                    self.announced = None;
                }
                self.after_game_change();
            }
            Cmd::StartManual => {
                if !self.desktop_allowed() && self.game.is_none() {
                    self.check_capture_allowed("desktop capture");
                    return;
                }
                self.manual = true;
                self.paused = false;
                if self.replay.lock().is_none() {
                    if self.settings.replay.enabled {
                        self.start_pipeline();
                    } else {
                        self.notice(true, "The replay buffer is disabled in settings");
                    }
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
                    self.after_game_change();
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

    /// (Re)starts what the current game and settings call for: the replay
    /// buffer and, with full-session recording, a recording.
    fn after_game_change(&mut self) {
        if self.paused {
            return;
        }
        if self.wants_pipeline() && self.replay.lock().is_none() {
            self.start_pipeline();
        }
        if self.game.is_some()
            && self.settings.recording.auto_record_sessions
            && !self.is_recording()
        {
            self.start_recording();
        }
        // Popup once per game launch.
        if let Some(g) = &self.game {
            let running = self.replay.lock().is_some() || self.recorder.lock().is_some();
            if running && self.announced != Some(g.pid) {
                self.announced = Some(g.pid);
                let mut parts = Vec::new();
                if self.replay.lock().is_some() {
                    let key = &self.settings.hotkeys.save_replay;
                    let secs = clock(self.settings.replay.duration_secs as f64);
                    parts.push(if key.is_empty() {
                        format!("Replay buffer on ({secs})")
                    } else {
                        format!("{key} saves the last {secs}")
                    });
                }
                if self.is_recording() {
                    parts.push("recording this session".into());
                }
                self.emit(toast(
                    if self.is_recording() {
                        ToastKind::RecordStart
                    } else {
                        ToastKind::Ready
                    },
                    format!("Capturing {}", g.display_name),
                    Some(parts.join(" · ")),
                ));
            }
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
        // Live volume / gate changes.
        for slot in [&self.replay, &self.recorder] {
            if let Some(p) = slot.lock().as_ref() {
                for t in s.audio.input_tracks() {
                    p.set_volume(&t.title, t.volume, t.muted);
                    p.set_gate(&t.title, t.gate_db);
                }
            }
        }
        let desktop_turned_off = self.desktop_allowed() && !s.video.allow_desktop_capture;
        self.settings = *s;
        if desktop_turned_off && self.game.is_none() {
            self.manual = false;
            self.stop_pipeline(true);
            return;
        }
        if restart {
            self.stop_replay(true);
        }
        if self.wants_pipeline() && self.replay.lock().is_none() {
            self.start_pipeline();
        } else if !self.wants_pipeline() && self.host() != Some(Host::Replay) {
            self.stop_replay(true);
        }
        if self.game.is_some()
            && self.settings.recording.auto_record_sessions
            && !self.is_recording()
            && !self.paused
        {
            self.start_recording();
        }
    }

    fn tick(&mut self) {
        // Stop the settings-page audio monitor when nobody is looking.
        {
            let mut m = self.monitor.lock();
            if m.as_ref()
                .is_some_and(|mon| mon.last_poll.elapsed() > MONITOR_IDLE)
            {
                *m = None;
            }
        }
        // Recorder health: a dead recorder ends the recording.
        let recorder_dead = {
            let mut g = self.recorder.lock();
            match g.as_mut() {
                Some(p) => !p.is_running() || p.shared.stalled(Duration::from_secs(8)),
                None => false,
            }
        };
        if recorder_dead {
            let msg = self
                .recorder
                .lock()
                .as_ref()
                .map(|p| p.shared.stderr_summary())
                .unwrap_or_default();
            self.notice(true, format!("Recording stopped unexpectedly: {msg}"));
            self.emit(toast(
                ToastKind::Error,
                "Recording stopped unexpectedly",
                None,
            ));
            self.stop_recorder(true);
        }

        let dead = {
            let mut guard = self.replay.lock();
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
            self.stop_replay(true);
            self.next_restart = Some(Instant::now() + self.restart_backoff);
            self.restart_backoff = (self.restart_backoff * 2).min(Duration::from_secs(60));
        }
        if self.replay.lock().is_none()
            && self.wants_pipeline()
            && self.next_restart.is_none_or(|t| Instant::now() >= t)
        {
            self.start_pipeline();
        }
        self.check_game_window();
    }

    /// The game's current main window and its client size, plus its
    /// rectangle on the monitor when `with_rect`.
    #[cfg(windows)]
    fn current_game_window(&self, with_rect: bool) -> Option<GameWindow> {
        use gc_platform::display::{window_client_size, window_placement};
        let g = self.game.as_ref()?;
        let h = g
            .hwnd
            .or_else(|| gc_platform::process::main_window(g.pid))?;
        let size = window_client_size(h)?;
        let rect = with_rect
            .then(|| window_placement(h).map(|p| p.rect))
            .flatten();
        Some((h, size, rect))
    }
    #[cfg(not(windows))]
    fn current_game_window(&self, _with_rect: bool) -> Option<GameWindow> {
        None
    }

    /// Restarts capture when the game's main window changed since capture
    /// started: many games open a small launcher/splash window first (CoD
    /// "Game HQ"), window capture is locked to one window and size, and
    /// monitor capture to the rectangle the window had (games often create
    /// their window off-centre and move it to 0,0 a moment later).
    fn check_game_window(&mut self) {
        let running = self.replay.lock().is_some() || self.recorder.lock().is_some();
        let now = if running {
            let with_rect = self.capture_window.is_some_and(|w| w.2.is_some());
            self.current_game_window(with_rect)
        } else {
            None
        };
        // No usable window right now (minimised, closing): keep going.
        let Some(now) = now.filter(|w| self.capture_window != Some(*w)) else {
            self.window_change = None;
            return;
        };
        let seen = match self.window_change {
            Some((w, n)) if w == now => n + 1,
            _ => 1,
        };
        if seen < WINDOW_SWITCH_TICKS {
            self.window_change = Some((now, seen));
            return;
        }
        self.window_change = None;
        let old = self.capture_window;
        log::info!(
            "game window changed ({old:?} -> {now:?}); restarting capture on the new window"
        );
        if old.is_none_or(|(h, _, _)| h != now.0) {
            // A new window may capture fine where the old one stalled.
            self.degraded = false;
        }
        let was_recording = self.is_recording();
        self.stop_pipeline(true);
        if self.wants_pipeline() {
            self.start_pipeline();
        }
        if was_recording {
            self.start_recording();
            self.notice(
                false,
                "The game window changed; the recording continues in a new file",
            );
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

    /// Starts an encoder pipeline at `quality`, trying encoders, capture
    /// sources and the focus screen from best to most compatible.
    fn launch(&mut self, quality: Quality, ring: RingPlan) -> Result<Pipeline, String> {
        let Some(ff) = self.ff.clone() else {
            return Err("FFmpeg is missing; capture is unavailable".into());
        };
        // Free the settings-page audio monitor (it would double-capture).
        self.monitor.lock().take();
        // Wait (bounded) for the encoder probe so the first start picks a
        // working hardware encoder.
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.probe.read().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let (caps, probed) = self.probe.read().clone().unwrap_or_default();

        let s = self.settings.clone();
        let window = self.current_game_window(true);
        let targets = self.resolve_targets(&caps);
        let font = Self::focus_font(&caps);
        let audio = AudioPlan {
            layout: s.audio.track_layout(),
            game_pid: self.game.as_ref().map(|g| g.pid),
            mic_device: s.audio.mic.device_id.clone(),
            system_fallback: s
                .audio
                .system_audio_fallback
                .unwrap_or(!gc_platform::system::supports_process_loopback()),
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
                            fps: quality.fps,
                            resolution: quality.resolution,
                            backend,
                            codec,
                            rate: quality.rate_control,
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
                        let started = Pipeline::start(&ff, spec, &audio, ring.clone(), game_pid)
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
                                // Only monitor capture depends on where the
                                // window is.
                                let follows_rect =
                                    matches!(target.video, VideoSource::Ddagrab { .. });
                                self.capture_window =
                                    window.map(|(h, s, r)| (h, s, r.filter(|_| follows_rect)));
                                self.window_change = None;
                                return Ok(p);
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
        Err(format!(
            "Could not start capture. {}",
            errors.last().cloned().unwrap_or_default()
        ))
    }

    fn start_pipeline(&mut self) {
        self.status.write().state = CaptureState::Starting;
        self.publish();
        let s = self.settings.clone();
        let quality = s.replay.quality;
        let storage = s.replay.effective_storage(quality.peak_kbps());
        let ring = RingPlan {
            window: Duration::from_secs(s.replay.duration_secs as u64 + 2),
            store: match storage {
                BufferStorage::Disk => Store::Disk(
                    s.replay
                        .disk_dir
                        .clone()
                        .unwrap_or_else(|| self.temp_dir.clone()),
                ),
                _ => Store::Memory,
            },
            max_bytes: if storage == BufferStorage::Disk {
                u64::MAX
            } else {
                s.replay.max_memory_mb as u64 * 1024 * 1024
            },
        };
        match self.launch(quality, ring) {
            Ok(p) => {
                *self.replay.lock() = Some(p);
                self.replay_quality = Some(quality);
                self.last_error = None;
                self.restart_backoff = Duration::from_secs(2);
                self.next_restart = None;
                if self.game.is_none() {
                    self.emit(toast(
                        ToastKind::Ready,
                        "Capturing the desktop",
                        Some(format!(
                            "{} saves the last {}",
                            self.settings.hotkeys.save_replay,
                            clock(self.settings.replay.duration_secs as f64)
                        )),
                    ));
                }
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                self.notice(true, e);
                self.next_restart = Some(Instant::now() + self.restart_backoff);
                self.restart_backoff = (self.restart_backoff * 2).min(Duration::from_secs(60));
            }
        }
    }

    fn stop_replay(&mut self, finalize: bool) {
        let p = self.replay.lock().take();
        self.replay_quality = None;
        if let Some(p) = p {
            if let Some(session) = p.stop() {
                if finalize {
                    self.finish_session(session);
                }
            }
        }
    }

    fn stop_recorder(&mut self, finalize: bool) {
        let p = self.recorder.lock().take();
        if let Some(p) = p {
            if let Some(session) = p.stop() {
                if finalize {
                    self.finish_session(session);
                }
            }
        }
    }

    /// Stops all pipelines; finalises any session in the background.
    fn stop_pipeline(&mut self, finalize: bool) {
        self.stop_recorder(finalize);
        self.stop_replay(finalize);
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
                let _ = events.send(toast(ToastKind::Error, "Saving the recording failed", None));
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
            audio_tracks: self.track_titles(&p.audio_titles),
            mix_tracks: self.track_titles(&p.mix_titles),
        }
    }

    /// Track names as written into files: the game track is named after the
    /// game ("Elden Ring"), apps after the app, the rest stay generic.
    fn track_titles(&self, titles: &[String]) -> Vec<String> {
        titles
            .iter()
            .map(|t| match &self.game {
                Some(g) if t == "Game" && !g.display_name.trim().is_empty() => {
                    g.display_name.clone()
                }
                _ => t.clone(),
            })
            .collect()
    }

    fn save_replay(&mut self) {
        if !self.settings.replay.enabled {
            self.notice(true, "Replay buffer is disabled in settings");
            return;
        }
        if !self.check_capture_allowed("the replay buffer") {
            return;
        }
        let Some(ff) = self.ff.clone() else { return };
        let guard = self.replay.lock();
        let Some(p) = guard.as_ref() else {
            drop(guard);
            self.notice(true, "The replay buffer is not running yet.");
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
        let saved_len = clock(span.min(secs));
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
                    faststart: true,
                    ..Default::default()
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
                    let _ = events.send(toast(
                        ToastKind::Saved,
                        format!("Saved last {saved_len}"),
                        game.clone(),
                    ));
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
                    let _ = events.send(toast(ToastKind::Error, "Saving the replay failed", None));
                }
            }
        });
    }

    fn toggle_recording(&mut self) {
        if self.is_recording() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    /// Starts a session recording. It shares the replay buffer's encoder when
    /// both use the same quality; otherwise a second encoder runs for the
    /// recording only.
    fn start_recording(&mut self) {
        if !self.check_capture_allowed("recording") {
            return;
        }
        let quality = self.settings.video.quality;
        let shared = self.replay.lock().is_some() && self.replay_quality == Some(quality);
        let host = if shared {
            Host::Replay
        } else {
            self.status.write().state = CaptureState::Starting;
            self.publish();
            let ring = RingPlan {
                window: Duration::from_secs(3),
                store: Store::Memory,
                max_bytes: 512 << 20,
            };
            match self.launch(quality, ring) {
                Ok(p) => {
                    *self.recorder.lock() = Some(p);
                    Host::Recorder
                }
                Err(e) => {
                    self.notice(true, format!("Cannot record: {e}"));
                    self.emit(toast(ToastKind::Error, "Recording could not start", None));
                    return;
                }
            }
        };
        let slot = match host {
            Host::Replay => self.replay.clone(),
            Host::Recorder => self.recorder.clone(),
        };
        let guard = slot.lock();
        let Some(p) = guard.as_ref() else { return };
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
        let game = meta.game_title.clone();
        let titles = meta.audio_tracks.clone();
        match SessionWriter::create(out, meta, titles, self.ff.as_ref()) {
            Ok(w) => {
                *p.shared.session.lock() = Some(w);
                drop(guard);
                self.notice(false, "Recording started");
                // The launch popup already announces auto-recordings.
                if self.announced.is_some() || self.game.is_none() {
                    self.emit(toast(ToastKind::RecordStart, "Recording started", game));
                }
            }
            Err(e) => {
                drop(guard);
                self.notice(true, format!("Cannot start recording: {e}"));
                if host == Host::Recorder {
                    self.stop_recorder(false);
                }
            }
        }
    }

    fn stop_recording(&mut self) {
        let secs = self.status.read().recording_secs;
        match self.host() {
            Some(Host::Recorder) => self.stop_recorder(true),
            Some(Host::Replay) => {
                let session = self
                    .replay
                    .lock()
                    .as_ref()
                    .and_then(|p| p.shared.session.lock().take());
                if let Some(s) = session {
                    self.finish_session(s);
                }
            }
            None => return,
        }
        self.notice(false, "Recording stopped – saving…");
        self.emit(toast(
            ToastKind::RecordStop,
            "Recording saved",
            Some(format!("{} recorded", clock(secs))),
        ));
        if let Some(s) = self.pending.take() {
            self.apply_settings(s);
        }
        if !self.wants_pipeline() {
            self.stop_replay(true);
        }
    }

    fn bookmark(&mut self) {
        let replay_t = self.replay.lock().as_ref().map(|p| {
            let t = p.shared.now_media_time();
            p.shared.bookmarks.lock().push(t);
            t
        });
        let offset = match self.host() {
            Some(host) => {
                let slot = if host == Host::Replay {
                    &self.replay
                } else {
                    &self.recorder
                };
                slot.lock().as_ref().and_then(|p| {
                    let t = p.shared.now_media_time();
                    p.shared
                        .session
                        .lock()
                        .as_mut()
                        .and_then(|s| s.add_bookmark(t, None).ok())
                })
            }
            None => None,
        };
        if replay_t.is_none() && offset.is_none() {
            self.notice(true, "Nothing is being captured – bookmark ignored");
            return;
        }
        let text = match offset {
            Some(ms) => format!("Bookmark added at {}", clock(ms as f64 / 1000.0)),
            None => "Bookmark added".to_string(),
        };
        self.notice(false, text.clone());
        self.emit(toast(ToastKind::Bookmark, text, None));
    }

    fn screenshot(&mut self) {
        if !self.check_capture_allowed("screenshots") {
            return;
        }
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
                    let _ = events.send(toast(
                        ToastKind::Screenshot,
                        "Screenshot saved",
                        game.clone(),
                    ));
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
            let replay = self.replay.lock();
            let recorder = self.recorder.lock();
            let mut s = self.status.write();
            s.game = self.game.clone();
            s.replay_enabled = self.settings.replay.enabled;
            s.last_error = self.last_error.clone();
            s.ffmpeg_found = self.ff.is_some();
            s.pending_restart = self.pending.is_some();
            s.desktop_capture = self.desktop_allowed();
            // The session, wherever it is.
            let session_secs = [replay.as_ref(), recorder.as_ref()]
                .into_iter()
                .flatten()
                .find_map(|p| {
                    let t = p.shared.now_media_time();
                    p.shared.session.lock().as_ref().map(|w| w.elapsed(t))
                });
            s.recording_secs = session_secs.unwrap_or(0.0);
            match replay.as_ref().or(recorder.as_ref()) {
                Some(p) => {
                    s.state = if session_secs.is_some() {
                        CaptureState::Recording
                    } else {
                        CaptureState::Buffering
                    };
                    s.buffer = replay.as_ref().map(|r| r.shared.ring.lock().stats());
                    s.encoder = Some(encoder_name(p.spec.backend, p.spec.codec).to_string());
                    s.encoder_label = Some(format!(
                        "{} {}",
                        p.spec.backend.label(),
                        codec_label(p.spec.codec)
                    ));
                    s.zero_copy = p.spec.is_zero_copy();
                    s.output_size = Some(p.spec.output_size());
                    s.fps = p.spec.fps.0;
                    s.capture_source = self.capture_label.map(String::from);
                    s.focus_lost = p.focus_lost();
                }
                None => {
                    s.buffer = None;
                    s.encoder = None;
                    s.encoder_label = None;
                    s.zero_copy = false;
                    s.output_size = None;
                    s.fps = self.settings.replay.quality.fps.0;
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

#[cfg(test)]
mod tests {
    #[test]
    fn clock_format() {
        assert_eq!(super::clock(125.4), "2:05");
        assert_eq!(super::clock(59.6), "1:00");
        assert_eq!(super::clock(3725.0), "1:02:05");
    }
}
