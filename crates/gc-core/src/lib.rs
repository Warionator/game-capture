//! Platform-independent core of `game-capture`.
//!
//! Everything in this crate is pure Rust and unit-tested on any OS. Windows
//! specific code (capture, audio, hooks, registry) lives in `gc-platform`,
//! `gc-audio` and `gc-capture`; those crates consume the types defined here.

pub mod config;
pub mod detect;
pub mod ffmpeg;
pub mod hotkey;
pub mod metadata;
pub mod naming;
pub mod ring;
pub mod ts;

pub use config::Settings;
pub use metadata::{Bookmark, ClipMetadata, MediaKind};
