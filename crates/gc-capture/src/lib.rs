//! Capture engine.
//!
//! [`CaptureEngine`] owns a single worker thread that serialises every state
//! change (game started/stopped, settings changed, hotkeys…). While a game is
//! running it keeps one FFmpeg process alive that grabs the game's monitor via
//! DXGI Desktop Duplication and encodes on the GPU; the MPEG-TS output feeds
//! the replay ring buffer and, when recording, the session file. Saving a
//! replay or finishing a session remuxes the buffered stream (stream copy, no
//! re-encode) into MP4/MKV with embedded metadata.

pub mod edit;
mod engine;
mod ffmpeg;
mod pipeline;
mod screenshot;
mod session;

pub use engine::{
    CaptureEngine, CaptureState, EngineEvent, EnginePaths, EngineStatus, GameContext, SavedMedia,
};
pub use ffmpeg::{probe_encoders, EncoderInfo, Ffmpeg};
pub use session::recover_sessions;
