//! Circular replay buffer of keyframe-aligned MPEG-TS chunks.
//!
//! Chunks live either in RAM (`Arc<[u8]>`) or as files in a spill directory
//! (for long windows / high bitrates, ideally on an NVMe drive). A disk chunk
//! deletes its file when the last reference is dropped, so a replay being
//! saved keeps its chunks alive even while the ring keeps evicting.

use crate::ts::{pts_delta, Chunk};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
enum Payload {
    Memory(Arc<[u8]>),
    Disk(Arc<DiskFile>),
}

#[derive(Debug)]
struct DiskFile {
    path: PathBuf,
}

impl Drop for DiskFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A reference to a buffered chunk that stays valid after eviction.
#[derive(Debug, Clone)]
pub struct ChunkRef {
    payload: PayloadRef,
    /// Seconds since the first chunk of the ring (media time).
    pub t: f64,
    pub len: usize,
}

#[derive(Debug, Clone)]
enum PayloadRef {
    Memory(Arc<[u8]>),
    Disk(Arc<DiskFile>),
    Owned(Arc<[u8]>),
}

impl ChunkRef {
    /// In-memory chunk not owned by any ring (e.g. the in-progress chunk).
    pub fn owned(data: Vec<u8>, t: f64) -> Self {
        let len = data.len();
        ChunkRef {
            payload: PayloadRef::Owned(data.into()),
            t,
            len,
        }
    }

    pub fn write_to(&self, w: &mut dyn Write) -> io::Result<()> {
        match &self.payload {
            PayloadRef::Memory(b) | PayloadRef::Owned(b) => w.write_all(b),
            PayloadRef::Disk(f) => {
                let mut file = std::fs::File::open(&f.path)?;
                io::copy(&mut file, w).map(|_| ())
            }
        }
    }

    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        match &self.payload {
            PayloadRef::Memory(b) | PayloadRef::Owned(b) => Ok(b.to_vec()),
            PayloadRef::Disk(f) => {
                let mut v = Vec::with_capacity(self.len);
                std::fs::File::open(&f.path)?.read_to_end(&mut v)?;
                Ok(v)
            }
        }
    }
}

#[derive(Debug)]
struct Entry {
    payload: Payload,
    t: f64,
    pts: Option<u64>,
    received: Instant,
    len: usize,
}

#[derive(Debug, Clone)]
pub enum Store {
    Memory,
    Disk(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RingStats {
    pub chunks: usize,
    pub bytes: u64,
    pub buffered_secs: f64,
    pub window_secs: f64,
    pub on_disk: bool,
}

#[derive(Debug)]
pub struct ReplayRing {
    entries: VecDeque<Entry>,
    window: Duration,
    store: Store,
    max_bytes: u64,
    bytes: u64,
    seq: u64,
}

static RING_ID: AtomicU64 = AtomicU64::new(0);

impl ReplayRing {
    /// `max_bytes` bounds memory use (ignored for disk storage except as a
    /// sanity cap). The ring keeps slightly more than `window` so that a
    /// saved clip is never shorter than requested.
    pub fn new(window: Duration, store: Store, max_bytes: u64) -> io::Result<Self> {
        let store = match store {
            Store::Disk(dir) => {
                let dir = dir.join(format!(
                    "ring-{}-{}",
                    std::process::id(),
                    RING_ID.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&dir)?;
                Store::Disk(dir)
            }
            Store::Memory => Store::Memory,
        };
        Ok(Self {
            entries: VecDeque::new(),
            window,
            store,
            max_bytes,
            bytes: 0,
            seq: 0,
        })
    }

    pub fn window(&self) -> Duration {
        self.window
    }

    pub fn set_window(&mut self, window: Duration) {
        self.window = window;
        self.evict();
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Media time of the next chunk relative to the ring start.
    fn next_time(&self, pts: Option<u64>, received: Instant) -> f64 {
        let Some(last) = self.entries.back() else {
            return 0.0;
        };
        let wall = received.duration_since(last.received).as_secs_f64();
        match (last.pts, pts) {
            (Some(a), Some(b)) => {
                let d = pts_delta(a, b) as f64 / 90_000.0;
                // Trust PTS unless it jumps (pipeline restart, discontinuity).
                if d > 0.0 && d < 30.0 && (d - wall).abs() < 10.0 {
                    last.t + d
                } else {
                    last.t + wall
                }
            }
            _ => last.t + wall,
        }
    }

    pub fn push(&mut self, chunk: Chunk) -> io::Result<()> {
        self.push_at(chunk, Instant::now())
    }

    pub fn push_at(&mut self, chunk: Chunk, received: Instant) -> io::Result<()> {
        let t = self.next_time(chunk.pts, received);
        let len = chunk.data.len();
        let payload = match &self.store {
            Store::Memory => Payload::Memory(chunk.data.into()),
            Store::Disk(dir) => {
                self.seq += 1;
                let path = dir.join(format!("{:08}.ts", self.seq));
                std::fs::write(&path, &chunk.data)?;
                Payload::Disk(Arc::new(DiskFile { path }))
            }
        };
        self.entries.push_back(Entry {
            payload,
            t,
            pts: chunk.pts,
            received,
            len,
        });
        self.bytes += len as u64;
        self.evict();
        Ok(())
    }

    fn span_from(&self, idx: usize) -> f64 {
        match (self.entries.get(idx), self.entries.back()) {
            (Some(a), Some(b)) => b.t - a.t,
            _ => 0.0,
        }
    }

    fn evict(&mut self) {
        let window = self.window.as_secs_f64();
        // Drop the oldest chunk while the remainder still covers the window.
        while self.entries.len() > 1 && self.span_from(1) >= window {
            self.pop_front();
        }
        while self.entries.len() > 1 && self.bytes > self.max_bytes {
            log::warn!("replay buffer over its memory budget; dropping oldest chunk");
            self.pop_front();
        }
    }

    fn pop_front(&mut self) {
        if let Some(e) = self.entries.pop_front() {
            self.bytes -= e.len as u64;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn stats(&self) -> RingStats {
        RingStats {
            chunks: self.entries.len(),
            bytes: self.bytes,
            buffered_secs: self.span_from(0) + self.entries.back().map(|_| 1.0).unwrap_or(0.0),
            window_secs: self.window.as_secs_f64(),
            on_disk: matches!(self.store, Store::Disk(_)),
        }
    }

    /// Chunks covering (at least) the last `secs` seconds, oldest first.
    pub fn snapshot(&self, secs: f64) -> Vec<ChunkRef> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let end = self.media_now();
        let mut start = 0;
        for (i, e) in self.entries.iter().enumerate().rev() {
            if end - e.t >= secs {
                start = i;
                break;
            }
        }
        self.entries
            .iter()
            .skip(start)
            .map(|e| ChunkRef {
                payload: match &e.payload {
                    Payload::Memory(b) => PayloadRef::Memory(b.clone()),
                    Payload::Disk(f) => PayloadRef::Disk(f.clone()),
                },
                t: e.t,
                len: e.len,
            })
            .collect()
    }

    /// Media time of the newest chunk's start and when it arrived.
    pub fn latest_time(&self) -> Option<(f64, Instant)> {
        self.entries.back().map(|e| (e.t, e.received))
    }

    /// Best estimate of the media time being captured right now. A chunk
    /// only arrives once the *next* keyframe is encoded, i.e. roughly one
    /// chunk duration after its start, so that duration is added before
    /// extrapolating with the time elapsed since arrival.
    pub fn media_now(&self) -> f64 {
        let n = self.entries.len();
        let Some(last) = self.entries.back() else {
            return 0.0;
        };
        let chunk_len = if n >= 2 {
            (last.t - self.entries[n - 2].t).clamp(0.0, 10.0)
        } else {
            1.0
        };
        last.t + chunk_len + last.received.elapsed().as_secs_f64().min(2.0)
    }

    pub fn spill_dir(&self) -> Option<&Path> {
        match &self.store {
            Store::Disk(d) => Some(d),
            Store::Memory => None,
        }
    }
}

impl Drop for ReplayRing {
    fn drop(&mut self) {
        self.entries.clear();
        if let Store::Disk(dir) = &self.store {
            // Files still referenced by in-flight saves are removed by their
            // own Drop; remove_dir only succeeds once the directory is empty.
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(i: u64) -> Chunk {
        Chunk {
            data: vec![i as u8; 100],
            pts: Some(i * 90_000),
        }
    }

    #[test]
    fn evicts_beyond_window_using_pts() {
        let mut r = ReplayRing::new(Duration::from_secs(10), Store::Memory, u64::MAX).unwrap();
        let t0 = Instant::now();
        for i in 0..30 {
            r.push_at(chunk(i), t0 + Duration::from_secs(i)).unwrap();
        }
        let s = r.stats();
        // 11 chunks: span 10 s between first and last chunk start.
        assert_eq!(s.chunks, 11);
        assert_eq!(s.bytes, 1100);
        let snap = r.snapshot(5.0);
        assert!(snap.len() >= 5 && snap.len() <= 7, "{}", snap.len());
        assert_eq!(snap.last().unwrap().read_all().unwrap(), vec![29u8; 100]);
    }

    #[test]
    fn memory_budget_is_enforced() {
        let mut r = ReplayRing::new(Duration::from_secs(1000), Store::Memory, 450).unwrap();
        let t0 = Instant::now();
        for i in 0..10 {
            r.push_at(chunk(i), t0 + Duration::from_secs(i)).unwrap();
        }
        assert_eq!(r.stats().chunks, 4);
    }

    #[test]
    fn pts_discontinuity_falls_back_to_wallclock() {
        let mut r = ReplayRing::new(Duration::from_secs(100), Store::Memory, u64::MAX).unwrap();
        let t0 = Instant::now();
        r.push_at(chunk(0), t0).unwrap();
        r.push_at(
            Chunk {
                data: vec![1],
                pts: Some(999_999_999),
            },
            t0 + Duration::from_secs(1),
        )
        .unwrap();
        assert!((r.latest_time().unwrap().0 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn disk_chunks_survive_eviction_while_referenced() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = ReplayRing::new(
            Duration::from_secs(2),
            Store::Disk(dir.path().into()),
            u64::MAX,
        )
        .unwrap();
        let spill = r.spill_dir().unwrap().to_path_buf();
        let t0 = Instant::now();
        for i in 0..3 {
            r.push_at(chunk(i), t0 + Duration::from_secs(i)).unwrap();
        }
        let held = r.snapshot(100.0);
        assert_eq!(held.len(), 3);
        for i in 3..10 {
            r.push_at(chunk(i), t0 + Duration::from_secs(i)).unwrap();
        }
        // Evicted from the ring, but still readable through the snapshot.
        assert_eq!(held[0].read_all().unwrap(), vec![0u8; 100]);
        let files = std::fs::read_dir(&spill).unwrap().count();
        assert_eq!(files, 3 + 3);
        drop(held);
        assert_eq!(std::fs::read_dir(&spill).unwrap().count(), 3);
        drop(r);
        assert!(!spill.exists());
    }
}
