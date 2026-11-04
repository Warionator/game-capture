//! Minimal file + stderr logger (rotates at 5 MB, keeps one old file).

use log::{Level, Log, Metadata, Record};
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 5 * 1024 * 1024;

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
    level: Level,
}

impl Log for FileLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= self.level
            && (m.target().starts_with("gc_")
                || m.target().starts_with("game_capture")
                || m.level() <= Level::Warn)
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!(
            "{} {:5} [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            r.level(),
            r.target(),
            r.args()
        );
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        let mut guard = self.file.lock();
        if guard
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() > MAX_BYTES)
        {
            *guard = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("old.log"));
        }
        if guard.is_none() {
            *guard = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
        }
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().as_mut() {
            let _ = f.flush();
        }
    }
}

pub fn init(path: PathBuf) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let level = if cfg!(debug_assertions) {
        Level::Debug
    } else {
        Level::Info
    };
    let logger = FileLogger {
        path,
        file: Mutex::new(None),
        level,
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level.to_level_filter());
    }
}
