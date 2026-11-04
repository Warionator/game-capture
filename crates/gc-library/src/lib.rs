//! Local media index (SQLite) and the self-healing library scanner.
//!
//! The files on disk are the source of truth; the database is a cache that
//! makes gallery queries instant. [`scan`] reconciles the two: new files are
//! probed (embedded metadata → file name → folder name fallbacks) and get a
//! thumbnail, changed files are re-probed and vanished files are dropped.
//! Restoring a backup into the library folder therefore rebuilds the gallery
//! automatically.

mod db;
mod scan;

pub use db::{GameSummary, Library, MediaFilter, MediaItem, SortOrder};
pub use scan::{
    index_file, item_from_probe, local_from_ms, scan, thumbnail_path, FfmpegInspector,
    MediaInspector, ScanReport,
};
