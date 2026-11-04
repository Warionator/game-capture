//! Multi-track audio capture.
//!
//! ```text
//!  WASAPI capture threads ──► SampleRing (per source, 48 kHz f32 stereo)
//!                                   │
//!                     pump thread (10 ms clock): sum sources of a track,
//!                     apply live gain/mute, measure peak, pad silence
//!                                   │
//!                 writer thread per track ──► named pipe ──► FFmpeg input
//! ```
//!
//! The pump is clock-driven, so every track delivers a continuous stream at
//! exactly 48 kHz even when an application is silent (process loopback
//! produces no packets then). That keeps all tracks in sync with the video.

mod buffer;
mod engine;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod wasapi;

pub use buffer::{apply_gain, peak, SampleRing, TrackControl};
pub use engine::{
    AudioEngine, AudioSink, CaptureHandle, CaptureTarget, EngineTrack, FileSink, SourceFactory,
    TrackLevel,
};
#[cfg(windows)]
pub use pipe::NamedPipeSink;
#[cfg(windows)]
pub use wasapi::WasapiFactory;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Lists active capture (microphone) endpoints.
pub fn list_input_devices() -> Vec<AudioDevice> {
    #[cfg(windows)]
    {
        wasapi::list_capture_devices().unwrap_or_else(|e| {
            log::warn!("enumerating audio devices failed: {e}");
            Vec::new()
        })
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The default source factory for this platform.
pub fn default_factory() -> std::sync::Arc<dyn SourceFactory> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(WasapiFactory)
    }
    #[cfg(not(windows))]
    {
        std::sync::Arc::new(engine::NullFactory)
    }
}
