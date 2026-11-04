//! Background game detection.
//!
//! Every couple of seconds the watcher takes a cheap process snapshot. Only
//! the foreground process and processes matching user-registered games are
//! inspected in depth (module list, version info, fullscreen state), so the
//! idle cost stays negligible. Auto-detected games must qualify on two
//! consecutive polls; user-registered games are picked up immediately. Once a game is detected it stays active until
//! its process exits, even if the user alt-tabs.

use crate::process::{self, ProcessEntry};
use crossbeam_channel::Sender;
use gc_core::config::GameSettings;
use gc_core::detect::{classify, display_name, Verdict};
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedGame {
    pub pid: u32,
    pub exe_name: String,
    pub exe_path: Option<String>,
    pub display_name: String,
    pub verdict: Verdict,
    #[serde(skip)]
    pub icon_png: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub enum WatchEvent {
    Started(DetectedGame),
    Stopped(DetectedGame),
}

pub struct ProcessWatcher {
    stop: Arc<AtomicBool>,
    rescan: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    current: Arc<RwLock<Option<DetectedGame>>>,
}

const POLL: Duration = Duration::from_millis(2000);
/// How long a rejected foreground process is not re-inspected.
const NEGATIVE_TTL: Duration = Duration::from_secs(15);

impl ProcessWatcher {
    pub fn spawn(cfg: Arc<RwLock<GameSettings>>, tx: Sender<WatchEvent>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let rescan = Arc::new(AtomicBool::new(false));
        let current: Arc<RwLock<Option<DetectedGame>>> = Arc::default();
        let thread = {
            let (stop, rescan, current) = (stop.clone(), rescan.clone(), current.clone());
            std::thread::Builder::new()
                .name("gc-process-watcher".into())
                .spawn(move || run(cfg, tx, stop, rescan, current))
                .expect("spawn watcher thread")
        };
        Self {
            stop,
            rescan,
            thread: Some(thread),
            current,
        }
    }

    pub fn current(&self) -> Option<DetectedGame> {
        self.current.read().clone()
    }

    /// Forget cached negative verdicts (call after the game list changes).
    pub fn rescan(&self) {
        self.rescan.store(true, Ordering::SeqCst);
    }
}

impl Drop for ProcessWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    cfg: Arc<RwLock<GameSettings>>,
    tx: Sender<WatchEvent>,
    stop: Arc<AtomicBool>,
    rescan: Arc<AtomicBool>,
    current: Arc<RwLock<Option<DetectedGame>>>,
) {
    let mut rejected: HashMap<u32, Instant> = HashMap::new();
    // A process that looked like a game on the previous poll. Auto-detected
    // games must qualify on two consecutive polls, so transient fullscreen
    // windows (screenshot region pickers, splash screens) don't count.
    let mut pending: Option<u32> = None;
    let own_pid = std::process::id();
    while !stop.load(Ordering::SeqCst) {
        let procs = process::snapshot();
        if rescan.swap(false, Ordering::SeqCst) {
            rejected.clear();
        }

        // 1. Is the current game still running?
        let active = current.read().clone();
        if let Some(game) = active {
            if procs.iter().any(|p| p.pid == game.pid) {
                sleep_unless_stopped(&stop, POLL);
                continue;
            }
            *current.write() = None;
            let _ = tx.send(WatchEvent::Stopped(game));
        }

        // 2. Look for a new one.
        let settings = cfg.read().clone();
        let fg = process::foreground_window();
        let mut candidates: Vec<&ProcessEntry> = procs
            .iter()
            .filter(|p| {
                settings
                    .custom
                    .iter()
                    .any(|g| g.executable.eq_ignore_ascii_case(&p.exe_name))
            })
            .collect();
        if let Some(f) = fg {
            if let Some(p) = procs.iter().find(|p| p.pid == f.pid) {
                candidates.push(p);
            }
        }
        rejected.retain(|_, t| t.elapsed() < NEGATIVE_TTL);
        let was_pending = pending.take();

        for p in candidates {
            if p.pid == own_pid || rejected.contains_key(&p.pid) {
                continue;
            }
            let facts = process::gather_facts(p.pid, &p.exe_name, fg);
            match classify(&facts, &settings) {
                Verdict::NotAGame => {
                    rejected.insert(p.pid, Instant::now());
                }
                Verdict::Detected if was_pending != Some(p.pid) => {
                    pending = Some(p.pid);
                }
                verdict => {
                    let game = DetectedGame {
                        pid: p.pid,
                        display_name: display_name(&facts, &settings),
                        icon_png: facts
                            .exe_path
                            .as_deref()
                            .and_then(|path| process::icon_png(path, 128)),
                        exe_name: facts.exe_name,
                        exe_path: facts.exe_path,
                        verdict,
                    };
                    log::info!(
                        "game detected: {} ({}, pid {})",
                        game.display_name,
                        game.exe_name,
                        game.pid
                    );
                    *current.write() = Some(game.clone());
                    let _ = tx.send(WatchEvent::Started(game));
                    break;
                }
            }
        }
        sleep_unless_stopped(&stop, POLL);
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(step);
        waited += step;
    }
}
