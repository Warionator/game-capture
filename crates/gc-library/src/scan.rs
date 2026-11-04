use crate::db::{Library, MediaItem};
use chrono::{DateTime, Local};
use gc_core::ffmpeg::{parse_probe, probe_args, thumbnail_args, ProbeInfo};
use gc_core::metadata::{Bookmark, ClipMetadata, MediaKind};
use gc_core::naming;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

/// Reads metadata from media files and renders thumbnails.
pub trait MediaInspector: Send + Sync {
    fn probe(&self, path: &Path) -> anyhow::Result<ProbeInfo>;
    fn thumbnail(
        &self,
        input: &Path,
        output: &Path,
        at_secs: f64,
        is_image: bool,
    ) -> anyhow::Result<()>;
}

/// [`MediaInspector`] backed by the bundled `ffprobe`/`ffmpeg` executables.
pub struct FfmpegInspector {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn hidden_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

impl MediaInspector for FfmpegInspector {
    fn probe(&self, path: &Path) -> anyhow::Result<ProbeInfo> {
        let out = hidden_command(&self.ffprobe)
            .args(probe_args(path))
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        parse_probe(&String::from_utf8_lossy(&out.stdout))
    }

    fn thumbnail(
        &self,
        input: &Path,
        output: &Path,
        at_secs: f64,
        is_image: bool,
    ) -> anyhow::Result<()> {
        if let Some(d) = output.parent() {
            std::fs::create_dir_all(d)?;
        }
        let out = hidden_command(&self.ffmpeg)
            .args(thumbnail_args(input, output, at_secs, is_image))
            .output()?;
        anyhow::ensure!(
            out.status.success() && output.exists(),
            "thumbnail failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub unchanged: u32,
    pub thumbnails_rebuilt: u32,
    pub errors: Vec<String>,
}

fn is_media(path: &Path) -> bool {
    naming::kind_from_extension(path).is_some()
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Stable 64-bit FNV-1a, used for thumbnail cache file names.
fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub fn thumbnail_path(thumbs_dir: &Path, media: &Path, mtime_ms: i64) -> PathBuf {
    let key = format!("{}|{}", media.to_string_lossy().to_lowercase(), mtime_ms);
    thumbs_dir.join(format!("{:016x}.jpg", fnv1a(key.as_bytes())))
}

/// Builds an index entry from probe results, applying the fallbacks:
/// embedded JSON → plain tags → file name → parent folder → mtime.
pub fn item_from_probe(
    path: &Path,
    root: &Path,
    info: &ProbeInfo,
    size: u64,
    mtime: i64,
) -> MediaItem {
    let meta = ClipMetadata::from_tags(&info.tags);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let parsed = naming::parse_file_name(file_name);
    let ext_kind = naming::kind_from_extension(path).unwrap_or(MediaKind::Clip);

    let kind = if ext_kind == MediaKind::Screenshot {
        MediaKind::Screenshot
    } else {
        meta.as_ref()
            .map(|m| m.kind)
            .or(parsed.and_then(|(_, k)| k))
            .unwrap_or(ext_kind)
    };

    let folder_game = path
        .parent()
        .filter(|p| naming::is_under(p, root) && !naming::is_under(root, p))
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|n| !n.eq_ignore_ascii_case(naming::UNCATEGORIZED))
        .map(str::to_string);
    let game_title = meta
        .as_ref()
        .and_then(|m| m.game_title.clone())
        .filter(|g| !g.trim().is_empty())
        .or(folder_game);

    let created_ms = meta
        .as_ref()
        .and_then(|m| DateTime::parse_from_rfc3339(&m.timestamp).ok())
        .map(|t| t.timestamp_millis())
        .or(parsed.map(|(t, _)| t.timestamp_millis()))
        .unwrap_or(mtime);

    let bookmarks = match meta.as_ref().map(|m| m.bookmarks.clone()) {
        Some(b) if !b.is_empty() => b,
        _ => info
            .chapters_ms
            .iter()
            // Our leading "Start" chapter (and any other at 0) is not a bookmark.
            .filter(|&&ms| ms > 0)
            .map(|&offset_ms| Bookmark {
                offset_ms,
                label: None,
            })
            .collect(),
    };

    MediaItem {
        id: 0,
        path: path.to_path_buf(),
        kind,
        game_title,
        executable: meta.as_ref().and_then(|m| m.executable_name.clone()),
        created_ms,
        duration_ms: if info.duration_ms > 0 || kind == MediaKind::Screenshot {
            info.duration_ms
        } else {
            meta.as_ref().map(|m| m.duration_ms).unwrap_or(0)
        },
        width: info.width,
        height: info.height,
        size_bytes: size,
        mtime_ms: mtime,
        bookmarks,
        audio_tracks: info.audio_tracks.clone(),
        video_codec: info.video_codec.clone(),
        thumbnail: None,
    }
}

/// Probes one file, renders its thumbnail and upserts it.
pub fn index_file(
    lib: &Mutex<Library>,
    root: &Path,
    thumbs_dir: &Path,
    inspector: &dyn MediaInspector,
    path: &Path,
) -> anyhow::Result<MediaItem> {
    let fs_meta = std::fs::metadata(path)?;
    let mtime = mtime_ms(&fs_meta);
    let info = match inspector.probe(path) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("probe failed for {}: {e}", path.display());
            ProbeInfo::default()
        }
    };
    let mut item = item_from_probe(path, root, &info, fs_meta.len(), mtime);
    let thumb = thumbnail_path(thumbs_dir, path, mtime);
    let at = if item.duration_ms > 4_000 {
        (item.duration_ms as f64 / 1000.0 * 0.25).min(10.0)
    } else {
        0.0
    };
    if thumb.exists()
        || inspector
            .thumbnail(path, &thumb, at, item.kind == MediaKind::Screenshot)
            .is_ok()
    {
        item.thumbnail = Some(thumb);
    }
    item.id = lib.lock().upsert(&item)?;
    Ok(item)
}

/// Removes index entries (and their cached thumbnails) for files that are
/// not inside any of `roots`. The files themselves are never touched. Used
/// when the library folder changes or an extra folder is removed, so the
/// gallery only ever shows the configured folders.
pub fn prune_outside(lib: &Mutex<Library>, roots: &[PathBuf]) -> u32 {
    let db = lib.lock();
    let Ok(known) = db.known_files() else {
        return 0;
    };
    let mut removed = 0;
    for (path, (id, _, _, thumb)) in known {
        if !roots.iter().any(|r| naming::is_under(&path, r)) && db.remove(id).is_ok() {
            removed += 1;
            if let Some(t) = thumb {
                let _ = std::fs::remove_file(t);
            }
        }
    }
    if removed > 0 {
        log::info!("dropped {removed} index entries outside the library folders");
    }
    removed
}

/// Reconciles the index with the files under `roots` (the library folder
/// plus user-added folders). Entries outside every root are dropped; entries
/// under a root that is currently unreachable (unplugged drive) are kept.
pub fn scan(
    lib: &Mutex<Library>,
    roots: &[PathBuf],
    thumbs_dir: &Path,
    inspector: &dyn MediaInspector,
    mut on_item: impl FnMut(&MediaItem),
) -> ScanReport {
    let mut report = ScanReport {
        removed: prune_outside(lib, roots),
        ..ScanReport::default()
    };
    let known = match lib.lock().known_files() {
        Ok(k) => k,
        Err(e) => {
            report.errors.push(format!("database: {e}"));
            return report;
        }
    };

    for root in roots {
        if !root.is_dir() {
            report.errors.push(format!(
                "folder {} is not reachable; its entries are kept",
                root.display()
            ));
            continue;
        }
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let walker = walkdir::WalkDir::new(root)
            .follow_links(false)
            .max_depth(4)
            .into_iter()
            .filter_entry(|e| {
                e.depth() == 0 || !e.file_name().to_str().is_some_and(|n| n.starts_with('.'))
            });
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    report.errors.push(e.to_string());
                    continue;
                }
            };
            let path = entry.path();
            if !entry.file_type().is_file() || !is_media(path) {
                continue;
            }
            seen.insert(path.to_path_buf());
            let Ok(meta) = entry.metadata() else { continue };
            let (size, mtime) = (meta.len(), mtime_ms(&meta));
            match known.get(path) {
                Some((_, s, m, thumb)) if *s == size && *m == mtime => {
                    // Unchanged; make sure the cached thumbnail still exists
                    // (the cache dir may have been wiped by a reformat).
                    if thumb.as_ref().is_some_and(|t| t.exists()) {
                        report.unchanged += 1;
                        continue;
                    }
                    report.thumbnails_rebuilt += 1;
                }
                Some(_) => report.updated += 1,
                None => report.added += 1,
            }
            match index_file(lib, root, thumbs_dir, inspector, path) {
                Ok(item) => on_item(&item),
                Err(e) => report.errors.push(format!("{}: {e}", path.display())),
            }
        }

        // Drop entries under this (reachable) root whose files vanished.
        let db = lib.lock();
        for (path, (id, _, _, thumb)) in &known {
            if naming::is_under(path, root)
                && !seen.contains(path)
                && !path.exists()
                && db.remove(*id).is_ok()
            {
                report.removed += 1;
                if let Some(t) = thumb {
                    let _ = std::fs::remove_file(t);
                }
            }
        }
    }
    log::info!(
        "library scan of {:?}: +{} ~{} -{} ={} ({} errors)",
        roots,
        report.added,
        report.updated,
        report.removed,
        report.unchanged,
        report.errors.len()
    );
    report
}

/// Local time for display/logging helpers.
pub fn local_from_ms(ms: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&Local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Fake {
        probes: AtomicU32,
    }
    impl MediaInspector for Fake {
        fn probe(&self, path: &Path) -> anyhow::Result<ProbeInfo> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            let mut tags = BTreeMap::new();
            if path.to_string_lossy().contains("tagged") {
                let m = ClipMetadata {
                    version: 1,
                    kind: MediaKind::Session,
                    game_title: Some("Real Name™".into()),
                    executable_name: Some("game.exe".into()),
                    timestamp: "2026-11-04T10:00:00+00:00".into(),
                    duration_ms: 5000,
                    bookmarks: vec![Bookmark {
                        offset_ms: 42,
                        label: None,
                    }],
                    resolution: None,
                    encoder: None,
                    audio_tracks: vec![],
                    mix_tracks: vec![],
                };
                tags.extend(m.to_tags());
            }
            Ok(ProbeInfo {
                duration_ms: 5000,
                width: Some(1280),
                height: Some(720),
                chapters_ms: vec![1000],
                tags,
                ..Default::default()
            })
        }
        fn thumbnail(&self, _: &Path, out: &Path, _: f64, _: bool) -> anyhow::Result<()> {
            std::fs::create_dir_all(out.parent().unwrap())?;
            std::fs::write(out, b"jpg")?;
            Ok(())
        }
    }

    #[test]
    fn self_healing_scan() {
        let root = tempfile::tempdir().unwrap();
        let thumbs = tempfile::tempdir().unwrap();
        let r = root.path();
        for p in [
            "Fortnite/2026-11-04_13-37-00_Clip.mp4",
            "Fortnite/2026-11-04_14-12-05_Screenshot.png",
            "Elden Ring/tagged.mkv",
            "Uncategorized/2026-11-04_16-00-00_Clip.mp4",
            "Fortnite/notes.txt",
            ".hidden/2026-11-04_16-00-00_Clip.mp4",
        ] {
            let p = r.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"data").unwrap();
        }
        let roots = vec![r.to_path_buf()];
        let lib = Mutex::new(Library::open_in_memory().unwrap());
        let fake = Fake {
            probes: AtomicU32::new(0),
        };
        let rep = scan(&lib, &roots, thumbs.path(), &fake, |_| {});
        assert_eq!(
            (rep.added, rep.removed, rep.unchanged),
            (4, 0, 0),
            "{rep:?}"
        );

        let items = lib.lock().list(&Default::default()).unwrap();
        let by_name = |n: &str| items.iter().find(|i| i.path.ends_with(n)).unwrap().clone();
        let clip = by_name("2026-11-04_13-37-00_Clip.mp4");
        assert_eq!(clip.game_title.as_deref(), Some("Fortnite"));
        assert_eq!(clip.kind, MediaKind::Clip);
        assert_eq!(
            clip.bookmarks[0].offset_ms, 1000,
            "chapters used as bookmarks"
        );
        assert!(clip.thumbnail.as_ref().unwrap().exists());
        let shot = by_name("2026-11-04_14-12-05_Screenshot.png");
        assert_eq!(shot.kind, MediaKind::Screenshot);
        let tagged = by_name("tagged.mkv");
        assert_eq!(tagged.game_title.as_deref(), Some("Real Name™"));
        assert_eq!(tagged.kind, MediaKind::Session);
        assert_eq!(tagged.bookmarks[0].offset_ms, 42);
        assert_eq!(tagged.created_ms, 1_793_786_400_000);
        assert_eq!(by_name("2026-11-04_16-00-00_Clip.mp4").game_title, None);

        // Second scan: nothing to do.
        let before = fake.probes.load(Ordering::SeqCst);
        let rep = scan(&lib, &roots, thumbs.path(), &fake, |_| {});
        assert_eq!((rep.added, rep.unchanged), (0, 4));
        assert_eq!(fake.probes.load(Ordering::SeqCst), before);

        // Thumbnail cache wiped (reformat) → rebuilt; file deleted → removed.
        for e in std::fs::read_dir(thumbs.path()).unwrap() {
            std::fs::remove_file(e.unwrap().path()).unwrap();
        }
        std::fs::remove_file(r.join("Fortnite/2026-11-04_13-37-00_Clip.mp4")).unwrap();
        let rep = scan(&lib, &roots, thumbs.path(), &fake, |_| {});
        assert_eq!((rep.thumbnails_rebuilt, rep.removed), (3, 1), "{rep:?}");
        assert_eq!(lib.lock().count().unwrap(), 3);
    }

    #[test]
    fn only_configured_folders_are_shown() {
        let base = tempfile::tempdir().unwrap();
        let thumbs = tempfile::tempdir().unwrap();
        let captures = base.path().join("Captures");
        let medal = base.path().join("Medal");
        for p in [
            medal.join("Clips/Fortnite/2026-11-01_10-00-00_Clip.mp4"),
            medal.join("Clips/Valorant/2026-11-02_10-00-00_Clip.mp4"),
            captures.join("Elden Ring/2026-11-04_13-37-00_Clip.mp4"),
        ] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"data").unwrap();
        }
        let lib = Mutex::new(Library::open_in_memory().unwrap());
        let fake = Fake {
            probes: AtomicU32::new(0),
        };
        let count = || lib.lock().count().unwrap();

        // Library pointed at Medal first, then switched to the empty-ish
        // Captures folder: Medal's clips must disappear from the gallery.
        scan(
            &lib,
            std::slice::from_ref(&medal),
            thumbs.path(),
            &fake,
            |_| {},
        );
        assert_eq!(count(), 2);
        let rep = scan(
            &lib,
            std::slice::from_ref(&captures),
            thumbs.path(),
            &fake,
            |_| {},
        );
        assert_eq!((rep.removed, rep.added), (2, 1), "{rep:?}");
        assert_eq!(count(), 1);
        assert!(
            medal
                .join("Clips/Fortnite/2026-11-01_10-00-00_Clip.mp4")
                .exists(),
            "files untouched"
        );

        // Explicitly added as an extra folder: shown again.
        let rep = scan(
            &lib,
            &[captures.clone(), medal.clone()],
            thumbs.path(),
            &fake,
            |_| {},
        );
        assert_eq!(rep.added, 2);
        let items = lib.lock().list(&Default::default()).unwrap();
        let games: Vec<_> = items.iter().filter_map(|i| i.game_title.clone()).collect();
        assert!(
            games.contains(&"Fortnite".to_string()) && games.contains(&"Elden Ring".to_string())
        );

        // Removing it again prunes immediately, without a full scan.
        assert_eq!(prune_outside(&lib, std::slice::from_ref(&captures)), 2);
        assert_eq!(count(), 1);

        // An unreachable folder (unplugged drive) keeps its entries.
        scan(
            &lib,
            &[captures.clone(), medal.clone()],
            thumbs.path(),
            &fake,
            |_| {},
        );
        std::fs::rename(&medal, base.path().join("unplugged")).unwrap();
        let rep = scan(
            &lib,
            &[captures.clone(), medal.clone()],
            thumbs.path(),
            &fake,
            |_| {},
        );
        assert_eq!(rep.removed, 0);
        assert_eq!(count(), 3);
    }
}
