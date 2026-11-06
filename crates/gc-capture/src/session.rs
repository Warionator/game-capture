//! Long session recording.
//!
//! While recording, keyframe-aligned MPEG-TS chunks are streamed into a
//! live FFmpeg stream-copy muxer that writes the final MP4/MKV next to the
//! output (as a hidden `.<name>.recording.<ext>`), so stopping only has to
//! write the index and rename the file: a second or two, however long the
//! session was. Metadata whose final value is only known at the end
//! (duration, bookmarks) is written as fixed-size, space-padded placeholders
//! and overwritten in place once the muxer is done.
//!
//! The same chunks are also appended to a hidden `.<name>.recording.ts` with a
//! JSON sidecar. MPEG-TS stays readable up to the last complete packet, so a
//! crash or power loss loses at most the last moment of footage:
//! [`recover_sessions`] finalises leftovers on the next launch. The TS file is
//! also the fallback when the live muxer fails.

use crate::ffmpeg::Ffmpeg;
use crossbeam_channel::{Sender, TrySendError};
use gc_core::ffmpeg::RemuxSpec;
use gc_core::metadata::{Bookmark, ClipMetadata, METADATA_TAG};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
    mux: Option<LiveMux>,
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

/// Hidden path the live muxer writes to before the final rename.
fn live_path(json: &Path, final_path: &Path) -> PathBuf {
    let ext = final_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");
    json.with_extension(ext)
}

fn meta_file_path(json: &Path) -> PathBuf {
    json.with_extension("meta.txt")
}

fn hide(path: &Path) {
    if let Err(e) = gc_platform::shell::set_hidden(path, true) {
        log::debug!("could not hide {}: {e}", path.display());
    }
}

/// Reserved bytes for the JSON metadata tag (bookmarks grow it).
const META_JSON_LEN: usize = 64 * 1024;
/// Reserved bytes for the plain `bookmarks` tag.
const BOOKMARKS_LEN: usize = 32 * 1024;
/// How far from the end of the file the patcher looks for the placeholders
/// (MP4 writes its index, including the tags, last).
const PATCH_TAIL: u64 = 64 << 20;
/// And from the start (Matroska may write tags in its header).
const PATCH_HEAD: u64 = 16 << 20;
/// Chunks the muxer may lag behind before it is given up on.
const MUX_QUEUE: usize = 512;

/// Pads a JSON document to `len` bytes with whitespace before its closing
/// bracket (still valid JSON, and not trailing whitespace a reader might
/// trim). `None` when it does not fit.
fn pad_json(json: &str, len: usize) -> Option<String> {
    if json.len() > len || json.len() < 2 {
        return None;
    }
    let (body, close) = json.split_at(json.len() - 1);
    Some(format!("{body}{}{close}", " ".repeat(len - json.len())))
}

fn duration_tag(ms: u64) -> String {
    format!("{:012.3}", ms as f64 / 1000.0)
}

/// Tag values that are rewritten in place when the session ends.
#[derive(Clone)]
struct Placeholders {
    meta_json: String,
    bookmarks: String,
    duration: String,
}

impl Placeholders {
    fn new(meta: &ClipMetadata) -> Option<Self> {
        Some(Self {
            meta_json: pad_json(&serde_json::to_string(meta).ok()?, META_JSON_LEN)?,
            bookmarks: pad_json("[]", BOOKMARKS_LEN)?,
            duration: duration_tag(0),
        })
    }

    /// Final values, same byte lengths as the placeholders.
    fn finalize(meta: &ClipMetadata) -> Self {
        let mut meta = meta.clone();
        loop {
            let json = serde_json::to_string(&meta).unwrap_or_default();
            let marks = serde_json::to_string(&meta.bookmarks).unwrap_or_default();
            if let (Some(meta_json), Some(bookmarks)) = (
                pad_json(&json, META_JSON_LEN),
                pad_json(&marks, BOOKMARKS_LEN),
            ) {
                return Self {
                    meta_json,
                    bookmarks,
                    duration: duration_tag(meta.duration_ms),
                };
            }
            // Thousands of bookmarks: keep as many as fit.
            log::warn!("too many bookmarks to embed; dropping the last one");
            meta.bookmarks.pop();
        }
    }

    fn tags(&self, meta: &ClipMetadata) -> Vec<(String, String)> {
        meta.to_tags()
            .into_iter()
            .map(|(k, v)| {
                let v = if k == METADATA_TAG {
                    self.meta_json.clone()
                } else if k == "bookmarks" {
                    self.bookmarks.clone()
                } else if k == "duration" {
                    self.duration.clone()
                } else {
                    v
                };
                (k, v)
            })
            .collect()
    }
}

fn ffmetadata(tags: &[(String, String)]) -> String {
    let escape = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if matches!(c, '=' | ';' | '#' | '\\' | '\n') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    let mut out = String::from(";FFMETADATA1\n");
    for (k, v) in tags {
        out.push_str(&format!("{}={}\n", escape(k), escape(v)));
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    (0..=hay.len() - needle.len())
        .filter(|&i| hay[i] == first)
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Overwrites placeholder tag values in a finished file with their final,
/// equal-length values. Returns how many were patched.
fn patch_tags(path: &Path, pairs: &[(&str, &str)]) -> std::io::Result<usize> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let len = f.metadata()?.len();
    let mut regions = vec![(len.saturating_sub(PATCH_TAIL), len)];
    if len > PATCH_TAIL {
        regions.push((0, PATCH_HEAD.min(len - PATCH_TAIL)));
    }
    let mut pending: Vec<(&str, &str)> = pairs
        .iter()
        .filter(|(old, new)| old != new && old.len() == new.len())
        .copied()
        .collect();
    let mut patched = 0;
    for (start, end) in regions {
        if pending.is_empty() {
            break;
        }
        let mut buf = vec![0u8; (end - start) as usize];
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut buf)?;
        pending.retain(|(old, new)| match find(&buf, old.as_bytes()) {
            Some(pos) => {
                let ok = f
                    .seek(SeekFrom::Start(start + pos as u64))
                    .and_then(|_| f.write_all(new.as_bytes()))
                    .is_ok();
                if ok {
                    patched += 1;
                }
                !ok
            }
            None => true,
        });
    }
    f.sync_all()?;
    Ok(patched)
}

/// FFmpeg stream-copying the session into its final container as it is
/// recorded.
struct LiveMux {
    tx: Option<Sender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<String>>,
    child: Child,
    path: PathBuf,
    meta_file: PathBuf,
    placeholders: Placeholders,
    failed: Arc<AtomicBool>,
}

impl LiveMux {
    fn start(ff: &Ffmpeg, info: &SessionInfo, json: &Path) -> anyhow::Result<Self> {
        let path = live_path(json, &info.final_path);
        let meta_file = meta_file_path(json);
        let _ = std::fs::remove_file(&path);
        let placeholders =
            Placeholders::new(&info.meta).ok_or_else(|| anyhow::anyhow!("metadata too large"))?;
        std::fs::write(&meta_file, ffmetadata(&placeholders.tags(&info.meta)))?;
        hide(&meta_file);
        let spec = RemuxSpec {
            tags: Vec::new(),
            audio_titles: info.audio_titles.clone(),
            chapters_file: None,
            metadata_file: Some(meta_file.to_string_lossy().into_owned()),
            faststart: false,
        };
        let mut child = ff
            .command()
            .args(gc_core::ffmpeg::remux_args("pipe:0", &path, &spec))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut err_pipe = child.stderr.take().expect("piped stderr");
        let failed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded::<Vec<u8>>(MUX_QUEUE);
        let writer = {
            let failed = failed.clone();
            let path = path.clone();
            std::thread::Builder::new()
                .name("gc-session-mux".into())
                .spawn(move || {
                    let mut hidden = false;
                    for data in rx {
                        if stdin.write_all(&data).is_err() {
                            failed.store(true, Ordering::SeqCst);
                            break;
                        }
                        // FFmpeg creates the file once it has probed the
                        // input; hide it as soon as it shows up.
                        if !hidden && path.exists() {
                            hide(&path);
                            hidden = true;
                        }
                    }
                    // Dropping stdin signals EOF; FFmpeg then writes the index.
                })?
        };
        // Drain stderr so a chatty FFmpeg never blocks on a full pipe; keep the tail.
        let stderr = std::thread::Builder::new()
            .name("gc-session-mux-err".into())
            .spawn(move || {
                let mut tail = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = err_pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > 8192 {
                        tail.drain(..tail.len() - 4096);
                    }
                }
                String::from_utf8_lossy(&tail).trim().to_string()
            })?;
        Ok(Self {
            tx: Some(tx),
            writer: Some(writer),
            stderr: Some(stderr),
            child,
            path,
            meta_file,
            placeholders,
            failed,
        })
    }

    fn push(&mut self, data: &[u8]) {
        let Some(tx) = &self.tx else { return };
        match tx.try_send(data.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::error!("session muxer fell behind; will finalise from the raw stream");
                self.failed.store(true, Ordering::SeqCst);
                self.tx = None;
            }
            Err(TrySendError::Disconnected(_)) => {
                self.failed.store(true, Ordering::SeqCst);
                self.tx = None;
            }
        }
    }

    /// Closes the input, waits for FFmpeg to write the index and patches the
    /// final metadata in. Returns the finished (still hidden) file.
    fn finish(mut self, meta: &ClipMetadata) -> anyhow::Result<PathBuf> {
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(st) = self.child.try_wait()? {
                break Some(st);
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let err = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let _ = std::fs::remove_file(&self.meta_file);
        let ok = status.is_some_and(|s| s.success()) && !self.failed.load(Ordering::SeqCst);
        if !ok {
            let _ = std::fs::remove_file(&self.path);
            anyhow::bail!(
                "live muxer failed{}",
                if err.is_empty() {
                    String::new()
                } else {
                    format!(": {err}")
                }
            );
        }
        let fin = Placeholders::finalize(meta);
        let pairs = [
            (self.placeholders.meta_json.as_str(), fin.meta_json.as_str()),
            (self.placeholders.bookmarks.as_str(), fin.bookmarks.as_str()),
            (self.placeholders.duration.as_str(), fin.duration.as_str()),
        ];
        let wanted = pairs.iter().filter(|(a, b)| a != b).count();
        match patch_tags(&self.path, &pairs) {
            Ok(n) if n < wanted => log::warn!(
                "patched {n} of {wanted} session tags in {}",
                self.path.display()
            ),
            Ok(_) => {}
            Err(e) => log::warn!("could not patch session tags: {e}"),
        }
        Ok(self.path.clone())
    }
}

impl Drop for LiveMux {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl SessionWriter {
    pub fn create(
        final_path: PathBuf,
        meta: ClipMetadata,
        audio_titles: Vec<String>,
        ff: Option<&Ffmpeg>,
    ) -> anyhow::Result<Self> {
        if let Some(d) = final_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let (ts_path, json_path) = temp_paths(&final_path);
        let file = File::create(&ts_path)?;
        hide(&ts_path);
        let file = BufWriter::with_capacity(4 << 20, file);
        let info = SessionInfo {
            final_path,
            meta,
            audio_titles,
        };
        let mux = ff.and_then(|ff| match LiveMux::start(ff, &info, &json_path) {
            Ok(m) => Some(m),
            Err(e) => {
                log::warn!("live session muxer unavailable ({e}); will remux at the end");
                None
            }
        });
        let w = Self {
            ts_path,
            json_path,
            file,
            mux,
            info,
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
        hide(&tmp);
        std::fs::rename(&tmp, &self.json_path)?;
        Ok(())
    }

    /// Appends one chunk whose media time is `t`.
    pub fn append(&mut self, data: &[u8], t: f64) -> std::io::Result<()> {
        self.start_t.get_or_insert(t);
        self.last_t = t;
        self.bytes += data.len() as u64;
        if let Some(m) = self.mux.as_mut() {
            m.push(data);
        }
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

    /// Flushes, finalises the live-muxed file (or remuxes the raw stream if
    /// that failed) and removes temp files.
    pub fn finish(mut self, ff: &Ffmpeg) -> anyhow::Result<PathBuf> {
        self.file.flush()?;
        let duration = self.start_t.map(|s| self.last_t - s + 1.0).unwrap_or(0.0);
        self.info.meta.duration_ms = (duration.max(0.0) * 1000.0) as u64;
        drop(self.file);
        if let Some(mux) = self.mux.take() {
            if self.bytes == 0 {
                drop(mux);
            } else {
                match mux.finish(&self.info.meta) {
                    Ok(done) => {
                        let out = self.info.final_path.clone();
                        match std::fs::rename(&done, &out) {
                            Ok(()) => {
                                let _ = gc_platform::shell::set_hidden(&out, false);
                                let _ = std::fs::remove_file(&self.ts_path);
                                let _ = std::fs::remove_file(&self.json_path);
                                return Ok(out);
                            }
                            Err(e) => {
                                log::error!("could not move finished session into place: {e}");
                                let _ = std::fs::remove_file(&done);
                            }
                        }
                    }
                    Err(e) => log::error!("{e}; remuxing the raw stream instead"),
                }
            }
        }
        let _ = std::fs::remove_file(live_path(&self.json_path, &self.info.final_path));
        let _ = std::fs::remove_file(meta_file_path(&self.json_path));
        finalize(ff, &self.ts_path, &self.json_path, self.info)
    }
}

/// Slow path: stream-copies the whole raw TS file into the final container.
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
        ..Default::default()
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
                let _ = gc_platform::shell::set_hidden(&fallback, false);
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
            // A live-muxed file cut off by the crash has no index; rebuild
            // from the raw stream instead.
            let _ = std::fs::remove_file(live_path(&path, &info.final_path));
            let _ = std::fs::remove_file(meta_file_path(&path));
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
        assert_eq!(
            live_path(&json, Path::new("/x/a.mp4")),
            Path::new("/lib/Fortnite/.2026-11-04_14-00-12_Session.recording.mp4")
        );
    }

    #[test]
    fn padding_keeps_valid_json_and_length() {
        let p = pad_json("[]", 64).unwrap();
        assert_eq!(p.len(), 64);
        assert!(p.starts_with('[') && p.ends_with(']'));
        assert!(serde_json::from_str::<Vec<u8>>(&p).unwrap().is_empty());
        assert!(pad_json("[1,2,3]", 3).is_none());
        assert_eq!(duration_tag(0).len(), duration_tag(3_599_999_000).len());
    }

    #[test]
    fn patches_placeholders_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.bin");
        let old = pad_json(r#"{"a":1}"#, 40).unwrap();
        let new = pad_json(r#"{"a":1,"b":[1,2]}"#, 40).unwrap();
        let mut body = vec![0u8; 1000];
        body.extend_from_slice(old.as_bytes());
        body.extend_from_slice(&[7u8; 100]);
        std::fs::write(&f, &body).unwrap();
        assert_eq!(patch_tags(&f, &[(&old, &new)]).unwrap(), 1);
        let out = std::fs::read(&f).unwrap();
        assert_eq!(out.len(), body.len());
        assert_eq!(&out[1000..1040], new.as_bytes());
        // Unequal lengths are never written.
        assert_eq!(patch_tags(&f, &[(&new, "short")]).unwrap(), 0);
    }

    /// Full live path against a real FFmpeg: chunks stream into the muxer,
    /// a bookmark lands mid-way, and stopping yields only the final MP4 with
    /// the bookmark and duration patched into its metadata. Skipped without
    /// ffmpeg (set GC_FFMPEG_DIR or put it on PATH).
    #[test]
    fn live_session_finishes_fast_with_patched_metadata() {
        use gc_core::metadata::MediaKind;
        let search: Vec<PathBuf> = std::env::var_os("GC_FFMPEG_DIR")
            .map(|d| vec![PathBuf::from(d)])
            .unwrap_or_default();
        let Ok(ff) = Ffmpeg::locate(&search) else {
            eprintln!("ffmpeg not found; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.ts");
        let st = ff
            .command()
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=30:duration=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=12",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-c:v",
                "libx264",
                "-g",
                "30",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-f",
                "mpegts",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(st.success());

        let game_dir = dir.path().join("Test Game");
        let out = game_dir.join("2026-11-06_01-22-36_Session.mp4");
        let meta = ClipMetadata {
            version: 1,
            kind: MediaKind::Session,
            game_title: Some("Test Game".into()),
            executable_name: Some("test.exe".into()),
            timestamp: "2026-11-06T01:22:36+00:00".into(),
            duration_ms: 0,
            bookmarks: vec![],
            resolution: Some("320x180".into()),
            encoder: Some("libx264".into()),
            audio_tracks: vec!["Master".into(), "Game".into()],
            mix_tracks: vec![],
        };
        let mut w = SessionWriter::create(
            out.clone(),
            meta,
            vec!["Master".into(), "Game".into()],
            Some(&ff),
        )
        .unwrap();
        let mut chunker = gc_core::ts::TsChunker::new();
        let mut chunks = chunker.push(&std::fs::read(&src).unwrap());
        chunks.extend(chunker.finish());
        assert!(chunks.len() > 5, "only {} chunks", chunks.len());
        let n = chunks.len();
        for (i, c) in chunks.into_iter().enumerate() {
            let t = c.pts.map(|p| p as f64 / 90_000.0).unwrap_or(0.0);
            w.append(&c.data, t).unwrap();
            if i == n / 2 {
                w.add_bookmark(t, Some("halfway".into())).unwrap();
            }
        }
        let started = Instant::now();
        let path = w.finish(&ff).unwrap();
        let took = started.elapsed();
        assert_eq!(path, out);
        assert!(took < Duration::from_secs(5), "finish took {took:?}");

        // Only the final file is left.
        let names: Vec<String> = std::fs::read_dir(&game_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["2026-11-06_01-22-36_Session.mp4"]);

        let info = crate::edit::probe(&ff, &out).unwrap();
        assert!(info.duration_ms > 10_000, "duration {}", info.duration_ms);
        assert_eq!(info.audio_tracks, ["Master", "Game"]);
        // The raw-stream fallback would have written chapters; the live path
        // cannot, so this proves the fast path was taken.
        assert!(info.chapters_ms.is_empty(), "fell back to the slow remux");
        let m = ClipMetadata::from_tags(&info.tags).expect("embedded metadata");
        assert_eq!(m.kind, MediaKind::Session);
        assert_eq!(m.bookmarks.len(), 1);
        assert_eq!(m.bookmarks[0].label.as_deref(), Some("halfway"));
        assert!(m.duration_ms > 10_000, "meta duration {}", m.duration_ms);
    }
}
