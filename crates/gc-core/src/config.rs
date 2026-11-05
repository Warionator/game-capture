//! User settings, persisted as JSON in the per-user config directory.
//!
//! Every field has a serde default so that settings files written by older
//! versions keep loading after new options are added.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Smallest and largest replay window the UI allows (15 s – 20 min).
pub const REPLAY_MIN_SECS: u32 = 15;
pub const REPLAY_MAX_SECS: u32 = 20 * 60;
/// Bitrate bounds in kbit/s (1 – 100 Mbps).
pub const BITRATE_MIN_KBPS: u32 = 1_000;
pub const BITRATE_MAX_KBPS: u32 = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Root of the media library (`<root>/<Game>/<timestamp>_Clip.mp4`).
    /// New captures are written here.
    pub storage_root: PathBuf,
    /// Extra folders the user explicitly added to the gallery (e.g. clips
    /// from another recorder). Indexed read-only; nothing is written there.
    pub extra_library_folders: Vec<PathBuf>,
    pub video: VideoSettings,
    pub replay: ReplaySettings,
    pub audio: AudioSettings,
    pub hotkeys: HotkeySettings,
    pub screenshot: ScreenshotSettings,
    pub system: SystemSettings,
    pub games: GameSettings,
    pub recording: RecordingSettings,
    pub overlay: OverlaySettings,
}

/// Full-session recording.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RecordingSettings {
    /// Start recording when a game launches and stop when it exits.
    pub auto_record_sessions: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum OverlayPosition {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// In-game popups ("Capture ready", "Saved last 2:00"…) and their chimes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct OverlaySettings {
    pub enabled: bool,
    pub position: OverlayPosition,
    pub sound: bool,
    /// Chime volume 0.0 – 1.0.
    pub volume: f32,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            position: OverlayPosition::TopLeft,
            sound: true,
            volume: 0.6,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            storage_root: default_storage_root(),
            extra_library_folders: Vec::new(),
            video: VideoSettings::default(),
            replay: ReplaySettings::default(),
            audio: AudioSettings::default(),
            hotkeys: HotkeySettings::default(),
            screenshot: ScreenshotSettings::default(),
            system: SystemSettings::default(),
            games: GameSettings::default(),
            recording: RecordingSettings::default(),
            overlay: OverlaySettings::default(),
        }
    }
}

fn default_storage_root() -> PathBuf {
    if let Some(profile) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        PathBuf::from(profile).join("Videos").join("game-capture")
    } else {
        PathBuf::from("game-capture")
    }
}

impl Settings {
    /// Loads settings from `path`, falling back to defaults when the file is
    /// missing. A corrupt file is renamed to `*.bak` instead of being lost.
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    s.sanitize();
                    s
                }
                Err(err) => {
                    log::warn!(
                        "settings file {} is invalid ({err}); using defaults",
                        path.display()
                    );
                    let _ = std::fs::rename(path, path.with_extension("json.bak"));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Atomically writes the settings (write to temp file, then rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Every folder the gallery shows: the library root first, then the
    /// user-added folders (duplicates and nested repeats removed).
    pub fn library_roots(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for p in std::iter::once(&self.storage_root).chain(&self.extra_library_folders) {
            if p.as_os_str().is_empty() || out.iter().any(|r| crate::naming::is_under(p, r)) {
                continue;
            }
            // A newly added parent folder supersedes folders inside it.
            out.retain(|r| !crate::naming::is_under(r, p));
            out.push(p.clone());
        }
        out
    }

    /// Clamps every numeric option into its supported range.
    pub fn sanitize(&mut self) {
        self.replay.duration_secs = self
            .replay
            .duration_secs
            .clamp(REPLAY_MIN_SECS, REPLAY_MAX_SECS);
        self.replay.max_memory_mb = self.replay.max_memory_mb.clamp(64, 16_384);
        self.video.quality.sanitize();
        self.replay.quality.sanitize();
        for src in self.audio.sources_mut() {
            src.volume = src.volume.clamp(0.0, 2.0);
        }
        self.screenshot.jpeg_quality = self.screenshot.jpeg_quality.clamp(10, 100);
        self.overlay.volume = if self.overlay.volume.is_finite() {
            self.overlay.volume.clamp(0.0, 1.0)
        } else {
            0.6
        };
        let mic = &mut self.audio.mic;
        mic.noise_suppression_strength = mic.noise_suppression_strength.clamp(1, 40);
        mic.noise_gate_db = if mic.noise_gate_db.is_finite() {
            mic.noise_gate_db
                .clamp(NOISE_GATE_MIN_DB, NOISE_GATE_MAX_DB)
        } else {
            -45.0
        };
    }

    /// Whether changing from `self` to `other` requires restarting the
    /// capture pipeline (encoder, resolution, track layout…). Volume and mute
    /// changes are applied live and do not.
    pub fn pipeline_differs(&self, other: &Settings) -> bool {
        let tracks = |s: &Settings| {
            s.audio
                .track_layout()
                .into_iter()
                .map(|t| (t.title, t.kind))
                .collect::<Vec<_>>()
        };
        let capture = |v: &VideoSettings| {
            (
                v.capture_mode,
                v.focus_screen,
                v.encoder,
                v.codec,
                v.monitor_index,
                v.capture_cursor,
                v.zero_copy,
            )
        };
        // Recording quality and the container only matter when a recording
        // starts or a file is written, so they never restart the buffer.
        capture(&self.video) != capture(&other.video)
            || self.replay.quality != other.replay.quality
            || self.replay.enabled != other.replay.enabled
            || self.replay.storage != other.replay.storage
            || self.audio.mic.device_id != other.audio.mic.device_id
            || self.audio.mic.noise_suppression != other.audio.mic.noise_suppression
            || self.audio.mic.noise_suppression_strength
                != other.audio.mic.noise_suppression_strength
            || self.audio.mixdown_enabled != other.audio.mixdown_enabled
            || tracks(self) != tracks(other)
    }
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    #[default]
    Native,
    #[serde(rename = "2160p")]
    P2160,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
}

impl Resolution {
    /// Target output height, `None` for native.
    pub fn height(self) -> Option<u32> {
        match self {
            Resolution::Native => None,
            Resolution::P2160 => Some(2160),
            Resolution::P1440 => Some(1440),
            Resolution::P1080 => Some(1080),
            Resolution::P720 => Some(720),
        }
    }

    /// Output size for a given source size. Never upscales and keeps the
    /// aspect ratio; widths are rounded to an even number (encoder
    /// requirement for 4:2:0 chroma).
    pub fn output_size(self, src_w: u32, src_h: u32) -> (u32, u32) {
        match self.height() {
            Some(h) if h < src_h && src_h > 0 => {
                let w = ((src_w as f64 * h as f64 / src_h as f64) / 2.0).round() as u32 * 2;
                (w.max(2), h)
            }
            _ => (src_w & !1, src_h & !1),
        }
    }
}

/// Capture frame rate. `0` means "source / uncapped".
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct FrameRate(pub u32);

impl FrameRate {
    pub const SUPPORTED: [u32; 5] = [30, 60, 120, 144, 0];
    pub fn is_uncapped(self) -> bool {
        self.0 == 0
    }
}

impl Default for FrameRate {
    fn default() -> Self {
        FrameRate(60)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum EncoderPreference {
    /// Probe at runtime: NVENC → AMF → QuickSync → software.
    #[default]
    Auto,
    Nvenc,
    Amf,
    Qsv,
    Software,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    #[default]
    H264,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RateControl {
    /// Constant bitrate (kbit/s).
    Cbr { bitrate_kbps: u32 },
    /// Variable bitrate with a peak of `max_kbps`.
    Vbr { bitrate_kbps: u32, max_kbps: u32 },
    /// Constant quality (CQP for hardware encoders, CRF for software).
    /// Lower is better quality; 0–51.
    Quality { level: u8 },
}

impl Default for RateControl {
    fn default() -> Self {
        RateControl::Vbr {
            bitrate_kbps: 25_000,
            max_kbps: 40_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Container {
    #[default]
    Mp4,
    Mkv,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }
}

/// How a running game is captured.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    /// Windows Graphics Capture of the game window only: other windows and
    /// notifications can never appear in a clip.
    #[default]
    Window,
    /// DXGI Desktop Duplication of the monitor the game is on.
    Monitor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct VideoSettings {
    pub capture_mode: CaptureMode,
    /// Replace the picture with a "Game not in focus" screen while another
    /// window is in front of the game.
    pub focus_screen: bool,
    /// Allow capturing the desktop (replay buffer, recordings, screenshots)
    /// when no game is running. Off by default: nothing is captured outside
    /// games.
    pub allow_desktop_capture: bool,
    /// Quality of session recordings (clips use [`ReplaySettings::quality`]).
    #[serde(flatten)]
    pub quality: Quality,
    pub encoder: EncoderPreference,
    pub codec: Codec,
    pub container: Container,
    /// DXGI output index to capture when no game window is known.
    pub monitor_index: u32,
    pub capture_cursor: bool,
    /// Keeps frames on the GPU from capture to encoder. When disabled, frames
    /// are downloaded to system memory first (slower, but works around driver
    /// bugs). The engine falls back automatically when the GPU path fails.
    pub zero_copy: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            capture_mode: CaptureMode::Window,
            focus_screen: true,
            allow_desktop_capture: false,
            quality: Quality::default(),
            encoder: EncoderPreference::Auto,
            codec: Codec::H264,
            container: Container::Mp4,
            monitor_index: 0,
            capture_cursor: false,
            zero_copy: true,
        }
    }
}

/// Resolution, frame rate and rate control of one output (clips or
/// recordings each have their own).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Quality {
    pub resolution: Resolution,
    pub fps: FrameRate,
    pub rate_control: RateControl,
}

impl Default for Quality {
    fn default() -> Self {
        Self {
            resolution: Resolution::Native,
            fps: FrameRate::default(),
            rate_control: RateControl::default(),
        }
    }
}

impl Quality {
    fn sanitize(&mut self) {
        if !FrameRate::SUPPORTED.contains(&self.fps.0) {
            self.fps = FrameRate::default();
        }
        let clamp = |v: u32| v.clamp(BITRATE_MIN_KBPS, BITRATE_MAX_KBPS);
        self.rate_control = match self.rate_control {
            RateControl::Cbr { bitrate_kbps } => RateControl::Cbr {
                bitrate_kbps: clamp(bitrate_kbps),
            },
            RateControl::Vbr {
                bitrate_kbps,
                max_kbps,
            } => {
                let b = clamp(bitrate_kbps);
                RateControl::Vbr {
                    bitrate_kbps: b,
                    max_kbps: clamp(max_kbps).max(b),
                }
            }
            RateControl::Quality { level } => RateControl::Quality {
                level: level.min(51),
            },
        };
    }

    /// Worst-case bitrate in kbit/s, used to size the replay buffer.
    pub fn peak_kbps(&self) -> u32 {
        match self.rate_control {
            RateControl::Cbr { bitrate_kbps } => bitrate_kbps,
            RateControl::Vbr { max_kbps, .. } => max_kbps,
            // Constant quality has no bound; assume a generous 60 Mbps.
            RateControl::Quality { .. } => 60_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Replay buffer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum BufferStorage {
    /// Keep in RAM when the window fits in `max_memory_mb`, else spill to disk.
    #[default]
    Auto,
    Memory,
    Disk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplaySettings {
    pub enabled: bool,
    pub duration_secs: u32,
    pub storage: BufferStorage,
    /// RAM budget for the in-memory ring buffer.
    pub max_memory_mb: u32,
    /// Directory for the on-disk ring buffer; `None` = system temp dir.
    pub disk_dir: Option<PathBuf>,
    /// Start the buffer automatically when a game is detected.
    pub auto_start: bool,
    /// Quality of saved clips (recordings use [`VideoSettings::quality`]).
    pub quality: Quality,
}

impl Default for ReplaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_secs: 120,
            storage: BufferStorage::Auto,
            max_memory_mb: 1024,
            disk_dir: None,
            auto_start: true,
            quality: Quality::default(),
        }
    }
}

impl ReplaySettings {
    /// Resolves `Auto` into a concrete storage given the video peak bitrate.
    pub fn effective_storage(&self, peak_kbps: u32) -> BufferStorage {
        match self.storage {
            BufferStorage::Auto => {
                // video + ~1 Mbps of audio, plus 10 % container overhead
                let bytes =
                    (peak_kbps as u64 + 1_000) * 1000 / 8 * self.duration_secs as u64 * 11 / 10;
                if bytes <= self.max_memory_mb as u64 * 1024 * 1024 {
                    BufferStorage::Memory
                } else {
                    BufferStorage::Disk
                }
            }
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioSource {
    pub enabled: bool,
    /// Linear gain, 0.0 – 2.0 (1.0 = unchanged).
    pub volume: f32,
    pub muted: bool,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 1.0,
            muted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct MicSettings {
    #[serde(flatten)]
    pub source: AudioSource,
    /// WASAPI endpoint id; `None` = default communications device.
    pub device_id: Option<String>,
    /// FFmpeg `afftdn` spectral denoiser.
    pub noise_suppression: bool,
    /// Denoise strength in dB (`afftdn` noise reduction), 1–40.
    pub noise_suppression_strength: u8,
    /// Noise gate: silence the mic while its level is below the threshold.
    /// Applied live (no restart), so the threshold can be tuned while
    /// watching the level meter.
    pub noise_gate: bool,
    /// Gate threshold in dBFS (-80 – -10).
    pub noise_gate_db: f32,
}

pub const NOISE_GATE_MIN_DB: f32 = -80.0;
pub const NOISE_GATE_MAX_DB: f32 = -10.0;

impl Default for MicSettings {
    fn default() -> Self {
        Self {
            source: AudioSource {
                enabled: false,
                ..AudioSource::default()
            },
            device_id: None,
            noise_suppression: true,
            noise_suppression_strength: 12,
            noise_gate: false,
            noise_gate_db: -45.0,
        }
    }
}

/// An application captured into its own track, matched by executable name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppTrack {
    pub name: String,
    pub executables: Vec<String>,
    #[serde(flatten)]
    pub source: AudioSource,
    /// Also mix this app into the master track.
    pub include_in_mix: bool,
    /// Set when the track is an installed web app (PWA). Its sound is played
    /// by the browser's shared audio process, so `executables` is the
    /// browser and the track contains everything that browser plays.
    pub web_app: Option<WebAppRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WebAppRef {
    /// Chromium app id (from the window's `…_crx_<id>` AppUserModelID).
    pub app_id: String,
    /// "Microsoft Edge", "Google Chrome"…
    pub browser: String,
}

impl Default for AppTrack {
    fn default() -> Self {
        Self {
            name: String::new(),
            executables: Vec::new(),
            source: AudioSource::default(),
            include_in_mix: true,
            web_app: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioSettings {
    /// Track 1: master mixdown of every source with `include_in_mix`.
    pub mixdown_enabled: bool,
    /// Track 2: the game process tree only (process loopback).
    pub game: AudioSource,
    /// Track 3.
    pub mic: MicSettings,
    /// Track 4: voice chat app.
    pub voice: AppTrack,
    /// Additional per-application tracks (Spotify, Chrome…).
    pub extra_tracks: Vec<AppTrack>,
    /// When the OS lacks process loopback (< build 19041) capture the whole
    /// system mix as the game track instead of recording silence.
    /// `None` = automatic: on only when per-app capture is unavailable.
    pub system_audio_fallback: Option<bool>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            mixdown_enabled: true,
            game: AudioSource::default(),
            mic: MicSettings::default(),
            voice: AppTrack {
                name: "Discord".into(),
                executables: vec!["Discord.exe".into()],
                ..AppTrack::default()
            },
            extra_tracks: Vec::new(),
            system_audio_fallback: None,
        }
    }
}

/// What feeds a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TrackKind {
    /// Sum of every other track with `in_mix`.
    Mix,
    Game,
    Mic,
    /// Applications matched by executable name (case-insensitive).
    Apps {
        executables: Vec<String>,
    },
}

/// One isolated audio source and how it is routed into the container.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSpec {
    pub title: String,
    pub kind: TrackKind,
    /// Whether the source contributes to the mixdown.
    pub in_mix: bool,
    pub volume: f32,
    pub muted: bool,
    /// FFmpeg `afftdn` denoiser strength in dB (microphone only).
    pub denoise: Option<u8>,
    /// Live noise-gate threshold in dBFS (microphone only).
    pub gate_db: Option<f32>,
}

/// Friendly track name for an executable: `Spotify.exe` → `Spotify`.
pub fn app_label(exe: &str) -> String {
    let stem = exe.trim();
    let stem = stem
        .rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("exe"))
        .map(|(s, _)| s)
        .unwrap_or(stem);
    let mut c = stem.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => "App".into(),
    }
}

impl AudioSettings {
    pub fn sources_mut(&mut self) -> Vec<&mut AudioSource> {
        let mut v = vec![&mut self.game, &mut self.mic.source, &mut self.voice.source];
        v.extend(self.extra_tracks.iter_mut().map(|t| &mut t.source));
        v
    }

    /// Isolated input sources, in the order they become container tracks
    /// after the optional mixdown. Disabled sources are omitted.
    pub fn input_tracks(&self) -> Vec<TrackSpec> {
        let mut out = Vec::new();
        if self.game.enabled {
            out.push(TrackSpec {
                title: "Game".into(),
                kind: TrackKind::Game,
                in_mix: true,
                volume: self.game.volume,
                muted: self.game.muted,
                denoise: None,
                gate_db: None,
            });
        }
        if self.mic.source.enabled {
            out.push(TrackSpec {
                title: "Microphone".into(),
                kind: TrackKind::Mic,
                in_mix: true,
                volume: self.mic.source.volume,
                muted: self.mic.source.muted,
                denoise: self
                    .mic
                    .noise_suppression
                    .then_some(self.mic.noise_suppression_strength),
                gate_db: self.mic.noise_gate.then_some(self.mic.noise_gate_db),
            });
        }
        for app in std::iter::once(&self.voice).chain(self.extra_tracks.iter()) {
            if app.source.enabled && !app.executables.is_empty() {
                out.push(TrackSpec {
                    title: if app.name.trim().is_empty() {
                        app_label(&app.executables[0])
                    } else {
                        app.name.trim().to_string()
                    },
                    kind: TrackKind::Apps {
                        executables: app.executables.clone(),
                    },
                    in_mix: app.include_in_mix,
                    volume: app.source.volume,
                    muted: app.source.muted,
                    denoise: None,
                    gate_db: None,
                });
            }
        }
        out
    }

    /// Full container track layout: `[Master]` followed by the inputs.
    pub fn track_layout(&self) -> Vec<TrackSpec> {
        let inputs = self.input_tracks();
        let mut out = Vec::with_capacity(inputs.len() + 1);
        if self.mixdown_enabled && inputs.iter().any(|t| t.in_mix) {
            out.push(TrackSpec {
                title: "Master".into(),
                kind: TrackKind::Mix,
                in_mix: false,
                volume: 1.0,
                muted: false,
                denoise: None,
                gate_db: None,
            });
        }
        out.extend(inputs);
        out
    }
}

// ---------------------------------------------------------------------------
// Hotkeys, screenshots, system, games
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HotkeySettings {
    pub save_replay: String,
    pub toggle_recording: String,
    pub add_bookmark: String,
    pub screenshot: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            save_replay: "Alt+F10".into(),
            toggle_recording: "Alt+F9".into(),
            add_bookmark: "Alt+F8".into(),
            screenshot: "Alt+F12".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ScreenshotSettings {
    pub format: ImageFormat,
    pub jpeg_quality: u8,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            jpeg_quality: 92,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SystemSettings {
    /// Writes `HKCU\Software\Microsoft\GameBar\AllowAutoGameMode = 0`.
    pub disable_game_mode: bool,
    /// Registers `game-capture --minimized` under the `Run` key.
    pub autostart: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    pub show_notifications: bool,
    /// Look for new versions on startup and every few hours.
    pub check_for_updates: bool,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            disable_game_mode: true,
            autostart: true,
            close_to_tray: true,
            show_notifications: true,
            check_for_updates: true,
        }
    }
}

/// A user-registered game (manual override).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomGame {
    pub display_name: String,
    /// Executable file name, e.g. `eldenring.exe` (matched case-insensitively).
    pub executable: String,
    /// Optional full path for disambiguation.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GameSettings {
    pub auto_detect: bool,
    pub custom: Vec<CustomGame>,
    /// Executables that must never be treated as games.
    pub excluded: Vec<String>,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            auto_detect: true,
            custom: Vec::new(),
            excluded: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_never_upscales_and_keeps_aspect() {
        assert_eq!(Resolution::P1080.output_size(2560, 1440), (1920, 1080));
        assert_eq!(Resolution::P720.output_size(3440, 1440), (1720, 720));
        assert_eq!(Resolution::P2160.output_size(1920, 1080), (1920, 1080));
        assert_eq!(Resolution::Native.output_size(1366, 768), (1366, 768));
        assert_eq!(Resolution::Native.output_size(1365, 767), (1364, 766));
    }

    #[test]
    fn sanitize_clamps_ranges() {
        let mut s = Settings::default();
        s.replay.duration_secs = 5;
        s.video.quality.rate_control = RateControl::Vbr {
            bitrate_kbps: 500_000,
            max_kbps: 10,
        };
        s.video.quality.fps = FrameRate(75);
        s.audio.game.volume = 9.0;
        s.sanitize();
        assert_eq!(s.replay.duration_secs, REPLAY_MIN_SECS);
        assert_eq!(
            s.video.quality.rate_control,
            RateControl::Vbr {
                bitrate_kbps: BITRATE_MAX_KBPS,
                max_kbps: BITRATE_MAX_KBPS
            }
        );
        assert_eq!(s.video.quality.fps, FrameRate(60));
        assert_eq!(s.audio.game.volume, 2.0);
    }

    #[test]
    fn default_track_layout_matches_spec() {
        let mut a = AudioSettings::default();
        a.mic.source.enabled = true;
        let titles: Vec<_> = a.track_layout().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["Master", "Game", "Microphone", "Discord"]);
        a.extra_tracks.push(AppTrack {
            name: "Spotify".into(),
            executables: vec!["Spotify.exe".into()],
            include_in_mix: false,
            ..AppTrack::default()
        });
        let layout = a.track_layout();
        assert_eq!(layout.len(), 5);
        a.extra_tracks[0].name = String::new();
        a.extra_tracks[0].executables = vec!["chrome.exe".into()];
        assert_eq!(a.track_layout()[4].title, "Chrome");
        assert_eq!(app_label("obs64.EXE"), "Obs64");
        assert!(!layout[4].in_mix);
    }

    #[test]
    fn mixdown_omitted_without_mixable_inputs() {
        let mut a = AudioSettings::default();
        a.game.enabled = false;
        a.voice.source.enabled = false;
        assert!(a.track_layout().is_empty());
    }

    #[test]
    fn settings_roundtrip_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let mut s = Settings::default();
        s.video.codec = Codec::Hevc;
        s.video.quality.rate_control = RateControl::Quality { level: 23 };
        s.save(&p).unwrap();
        assert_eq!(Settings::load_or_default(&p), s);

        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["video"]["rateControl"],
            serde_json::json!({"mode": "quality", "level": 23})
        );
        assert_eq!(json["video"]["resolution"], "native");
        assert_eq!(json["audio"]["mic"]["noiseSuppression"], true);
        assert_eq!(json["audio"]["voice"]["includeInMix"], true);

        std::fs::write(&p, r#"{"replay":{"durationSecs":300},"video":{"rateControl":{"mode":"cbr","bitrateKbps":8000}}}"#).unwrap();
        let s = Settings::load_or_default(&p);
        assert_eq!(s.replay.duration_secs, 300);
        assert_eq!(
            s.video.quality.rate_control,
            RateControl::Cbr { bitrate_kbps: 8000 }
        );
        assert!(s.replay.enabled);

        std::fs::write(&p, "not json").unwrap();
        assert_eq!(Settings::load_or_default(&p), Settings::default());
        assert!(p.with_extension("json.bak").exists());
    }

    #[test]
    fn library_roots_dedupe() {
        let s = Settings {
            storage_root: PathBuf::from("D:/Captures"),
            extra_library_folders: vec![
                PathBuf::from("D:/Medal"),
                PathBuf::from("d:/captures/Old"),
                PathBuf::from("D:/medal"),
                PathBuf::from(""),
            ],
            ..Default::default()
        };
        assert_eq!(
            s.library_roots(),
            [PathBuf::from("D:/Captures"), PathBuf::from("D:/Medal")]
        );
    }

    #[test]
    fn auto_buffer_storage_spills_to_disk() {
        let mut r = ReplaySettings {
            duration_secs: 120,
            max_memory_mb: 1024,
            ..Default::default()
        };
        assert_eq!(r.effective_storage(40_000), BufferStorage::Memory);
        r.duration_secs = 1200;
        assert_eq!(r.effective_storage(40_000), BufferStorage::Disk);
    }

    #[test]
    fn volume_change_does_not_restart_pipeline() {
        let a = Settings::default();
        let mut b = a.clone();
        b.audio.game.volume = 0.5;
        b.audio.voice.source.muted = true;
        b.audio.mic.noise_gate = true;
        b.audio.mic.noise_gate_db = -30.0;
        assert!(!a.pipeline_differs(&b), "gate and volume are applied live");
        b.video.quality.fps = FrameRate(144);
        b.video.container = Container::Mkv;
        assert!(
            !a.pipeline_differs(&b),
            "recording quality applies to the next recording"
        );
        b.replay.quality.fps = FrameRate(30);
        assert!(a.pipeline_differs(&b), "clip quality restarts the buffer");
        b.replay.quality = a.replay.quality;
        b.video.codec = Codec::Av1;
        assert!(a.pipeline_differs(&b));
    }
}
