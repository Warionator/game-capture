//! Long session recording.
//!
//! While recording, keyframe-aligned MPEG-TS chunks are appended to a hidden
//! `.<name>.recording.ts` file next to the final output, with a JSON sidecar
//! holding the metadata and bookmarks so far. MPEG-TS stays readable up to
//! the last complete packet, so a crash or power loss loses at most the last
//! moment of footage: [`recover_sessions`] finalises leftovers on the next
//! launch. On a normal stop the file is remuxed (stream copy) into the
//! configured MP4/MKV container with embedded metadata and chapters.

use crate::ffmpeg::Ffmpeg;
use gc_core::ffmpeg::RemuxSpec;
use gc_core::metadata::{Bookmark, ClipMetadata};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub final_path: PathBuf,
    pub meta: ClipMetadata,
    pub audio_titles: Vec<String>,
}

pub struct SessionWriter {
    ts_path: PathBuf,
    json_path: PathBuf,
    file: BufWriter<File>,
    pub info: SessionInfo,
    start_t: Option<f64>,
    last_t: f64,
    pub bytes: u64,
}

pub fn temp_paths(final_path: &Path) -> (PathBuf, PathBuf) {
    let dir = final_path.parent().unwrap_or(Path::new("."));
    let stem = final_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    (
        dir.join(format!(".{stem}.recording.ts")),
        dir.join(format!(".{stem}.recording.json")),
    )
}

impl SessionWriter {
    pub fn create(
        final_path: PathBuf,
        meta: ClipMetadata,
        audio_titles: Vec<String>,
    ) -> anyhow::Result<Self> {
        if let Some(d) = final_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let (ts_path, json_path) = temp_paths(&final_path);
        let file = BufWriter::with_capacity(4 << 20, File::create(&ts_path)?);
        let w = Self {
            ts_path,
            json_path,
            file,
            info: SessionInfo {
                final_path,
                meta,
                audio_titles,
            },
            start_t: None,
            last_t: 0.0,
            bytes: 0,
        };
        w.write_sidecar()?;
        Ok(w)
    }

    fn write_sidecar(&self) -> anyhow::Result<()> {
        let tmp = self.json_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.info)?)?;
        std::fs::rename(&tmp, &self.json_path)?;
        Ok(())
    }

    /// Appends one chunk whose media time is `t`.
    pub fn append(&mut self, data: &[u8], t: f64) -> std::io::Result<()> {
        self.start_t.get_or_insert(t);
        self.last_t = t;
        self.bytes += data.len() as u64;
        self.file.write_all(data)
    }

    /// Recorded duration so far (seconds).
    pub fn elapsed(&self, now_t: f64) -> f64 {
        self.start_t.map(|s| (now_t - s).max(0.0)).unwrap_or(0.0)
    }

    pub fn add_bookmark(&mut self, now_t: f64, label: Option<String>) -> anyhow::Result<u64> {
        let offset_ms = (self.elapsed(now_t) * 1000.0) as u64;
        self.info.meta.bookmarks.push(Bookmark { offset_ms, label });
        let _ = self.file.flush();
        self.write_sidecar()?;
        Ok(offset_ms)
    }

    /// Flushes, remuxes into the final container and removes temp files.
    pub fn finish(mut self, ff: &Ffmpeg) -> anyhow::Result<PathBuf> {
        self.file.flush()?;
        let duration = self.start_t.map(|s| self.last_t - s + 1.0).unwrap_or(0.0);
        self.info.meta.duration_ms = (duration.max(0.0) * 1000.0) as u64;
        drop(self.file);
        finalize(ff, &self.ts_path, &self.json_path, self.info)
    }
}

fn finalize(ff: &Ffmpeg, ts: &Path, json: &Path, info: SessionInfo) -> anyhow::Result<PathBuf> {
    let size = std::fs::metadata(ts).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(ts);
        let _ = std::fs::remove_file(json);
        anyhow::bail!("session contains no footage");
    }
    let chapters = ts.with_extension("chapters.txt");
    let chapters_file = if info.meta.bookmarks.is_empty() {
        None
    } else {
        std::fs::write(&chapters, info.meta.ffmetadata_chapters())?;
        Some(chapters.to_string_lossy().into_owned())
    };
    let spec = RemuxSpec {
        tags: info.meta.to_tags(),
        audio_titles: info.audio_titles.clone(),
        chapters_file,
    };
    let out = &info.final_path;
    let result = ff.remux_file(ts, out, &spec);
    let _ = std::fs::remove_file(&chapters);
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(ts);
            let _ = std::fs::remove_file(json);
            Ok(out.clone())
        }
        Err(e) => {
            // Keep the raw stream so nothing is lost; it is still playable.
            let fallback = out.with_extension("ts");
            if std::fs::rename(ts, &fallback).is_ok() {
                let _ = std::fs::remove_file(json);
                log::error!(
                    "session remux failed ({e}); kept raw stream at {}",
                    fallback.display()
                );
            }
            Err(e)
        }
    }
}

/// Finalises sessions left behind by a crash. Returns the recovered files.
pub fn recover_sessions(root: &Path, ff: &Ffmpeg) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(games) = std::fs::read_dir(root) else {
        return out;
    };
    for dir in games.flatten().filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !(name.starts_with('.') && name.ends_with(".recording.json")) {
                continue;
            }
            let ts = path.with_extension("ts");
            let info = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<SessionInfo>(&b).ok());
            // Duration in the sidecar is unknown (0); the library scanner reads
            // the real one from the container.
            let Some(info) = info else { continue };
            match finalize(ff, &ts, &path, info) {
                Ok(p) => {
                    log::info!("recovered interrupted session {}", p.display());
                    out.push(p);
                }
                Err(e) => log::warn!("could not recover {}: {e}", ts.display()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_paths_are_hidden_siblings() {
        let (ts, json) = temp_paths(Path::new("/lib/Fortnite/2026-11-04_14-00-12_Session.mkv"));
        assert_eq!(
            ts,
            Path::new("/lib/Fortnite/.2026-11-04_14-00-12_Session.recording.ts")
        );
        assert_eq!(
            json,
            Path::new("/lib/Fortnite/.2026-11-04_14-00-12_Session.recording.json")
        );
        assert_eq!(json.with_extension("ts"), ts);
    }
}
