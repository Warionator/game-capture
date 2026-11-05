//! Self-update.
//!
//! Installed copies download a signed update in the background, keep it in
//! `%LOCALAPPDATA%\GameCapture\updates` (so it survives restarts) and apply
//! it when the user clicks "Restart to update". Portable copies only
//! announce new versions; the user downloads them.

use crate::state::AppState;
use gc_core::update::{detect_install_kind, package_file_name, stale_packages, InstallKind};
use parking_lot::Mutex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

/// Where portable users download new versions.
const RELEASES_URL: &str = "https://github.com/Warionator/game-capture/releases";
/// Let startup (capture, library scan) settle before the first check.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// The release manifest of the downloaded package, for restoring it offline.
const PENDING_FILE: &str = "pending.json";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Installer: package downloaded and verified, waiting for a restart.
    Ready {
        version: String,
        notes: Option<String>,
    },
    /// Portable: a newer version exists; the user downloads it.
    Available {
        version: String,
        notes: Option<String>,
        url: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub kind: InstallKind,
    pub current_version: String,
    pub phase: Phase,
    /// Unix ms of the last completed check.
    pub last_checked_ms: Option<i64>,
}

pub struct UpdateState {
    kind: InstallKind,
    current_version: String,
    dir: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    phase: Phase,
    /// The verified, downloaded update and where its package is stored.
    ready: Option<(Update, PathBuf)>,
    checking: bool,
    last_checked: Option<Instant>,
    last_checked_ms: Option<i64>,
}

impl UpdateState {
    fn info(&self) -> UpdateInfo {
        let g = self.inner.lock();
        UpdateInfo {
            kind: self.kind,
            current_version: self.current_version.clone(),
            phase: g.phase.clone(),
            last_checked_ms: g.last_checked_ms,
        }
    }
}

fn state(app: &AppHandle) -> Arc<UpdateState> {
    app.state::<Arc<UpdateState>>().inner().clone()
}

fn set_phase(app: &AppHandle, st: &UpdateState, phase: Phase) {
    st.inner.lock().phase = phase;
    let _ = app.emit("update", st.info());
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn updater(app: &AppHandle) -> Result<Updater, String> {
    let handle = app.clone();
    app.updater_builder()
        .timeout(Duration::from_secs(60))
        // `Update::install` runs the installer and exits the process without
        // the normal exit path, so finalise recordings here first.
        .on_before_exit(move || crate::state::shutdown(&handle))
        .build()
        .map_err(|e| e.to_string())
}

/// Deletes cached packages except the one for `keep` (and the manifest when
/// nothing is kept).
fn clear_cache(dir: &Path, keep: Option<&str>) {
    for path in stale_packages(dir, keep) {
        if keep.is_some() && path.file_name().is_some_and(|n| n == PENDING_FILE) {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("updater: could not delete {}: {e}", path.display());
        }
    }
}

pub fn init(app: &AppHandle) {
    let kind = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(detect_install_kind))
        .unwrap_or(InstallKind::Portable);
    let dir = app
        .path()
        .app_local_data_dir()
        .map(|d| d.join("updates"))
        .unwrap_or_else(|_| std::env::temp_dir().join("game-capture-updates"));
    let st = Arc::new(UpdateState {
        kind,
        current_version: app.package_info().version.to_string(),
        dir,
        inner: Mutex::new(Inner {
            phase: Phase::Idle,
            ready: None,
            checking: false,
            last_checked: None,
            last_checked_ms: None,
        }),
    });
    app.manage(st.clone());
    log::info!("updater: {kind:?} install, version {}", st.current_version);

    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("updater".into())
        .spawn(move || {
            match kind {
                InstallKind::Installer => restore_pending(&app, &st),
                InstallKind::Portable => clear_cache(&st.dir, None),
            }
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                let enabled = app
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.settings.read().system.check_for_updates)
                    .unwrap_or(true);
                let due = st
                    .inner
                    .lock()
                    .last_checked
                    .is_none_or(|t| t.elapsed() >= CHECK_INTERVAL);
                if enabled && due {
                    tauri::async_runtime::block_on(check(&app));
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        });
    if let Err(e) = spawned {
        log::error!("updater: could not start: {e}");
    }
}

/// Shows an update downloaded in an earlier session without going online.
fn restore_pending(app: &AppHandle, st: &UpdateState) {
    let restored = (|| -> Option<(Update, PathBuf)> {
        let raw = std::fs::read(st.dir.join(PENDING_FILE)).ok()?;
        let raw: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        // `None` once the pending version is installed (no longer newer).
        let update = updater(app).ok()?.restore_update(raw).ok()??;
        let path = st.dir.join(package_file_name(&update.version));
        update.verify(&std::fs::read(&path).ok()?).ok()?;
        Some((update, path))
    })();
    match restored {
        Some((update, path)) => {
            log::info!("updater: {} already downloaded", update.version);
            let phase = Phase::Ready {
                version: update.version.clone(),
                notes: update.body.clone(),
            };
            st.inner.lock().ready = Some((update, path));
            set_phase(app, st, phase);
        }
        None => clear_cache(&st.dir, None),
    }
}

/// Checks for a newer version and, for installed copies, downloads it.
pub async fn check(app: &AppHandle) -> UpdateInfo {
    let st = state(app);
    {
        let mut g = st.inner.lock();
        if g.checking {
            drop(g);
            return st.info();
        }
        g.checking = true;
        if !matches!(g.phase, Phase::Ready { .. } | Phase::Available { .. }) {
            g.phase = Phase::Checking;
        }
    }
    let _ = app.emit("update", st.info());

    let result = run_check(app, &st).await;
    {
        let mut g = st.inner.lock();
        g.checking = false;
        g.last_checked = Some(Instant::now());
        g.last_checked_ms = Some(now_ms());
        match result {
            Ok(phase) => g.phase = phase,
            Err(e) => {
                log::warn!("updater: check failed: {e}");
                // Keep showing an update that is already downloaded.
                if g.ready.is_none() {
                    g.phase = Phase::Error { message: e };
                } else if let Some((u, _)) = &g.ready {
                    g.phase = Phase::Ready {
                        version: u.version.clone(),
                        notes: u.body.clone(),
                    };
                }
            }
        }
    }
    let info = st.info();
    let _ = app.emit("update", info.clone());
    info
}

async fn run_check(app: &AppHandle, st: &UpdateState) -> Result<Phase, String> {
    let Some(update) = updater(app)?.check().await.map_err(|e| e.to_string())? else {
        st.inner.lock().ready = None;
        clear_cache(&st.dir, None);
        return Ok(Phase::UpToDate);
    };
    let version = update.version.clone();
    let notes = update.body.clone();
    log::info!("updater: version {version} is available");

    if st.kind == InstallKind::Portable {
        let url = format!("{RELEASES_URL}/tag/v{version}");
        return Ok(Phase::Available {
            version,
            notes,
            url,
        });
    }

    let ready = Phase::Ready {
        version: version.clone(),
        notes: notes.clone(),
    };
    if st
        .inner
        .lock()
        .ready
        .as_ref()
        .is_some_and(|(u, _)| u.version == version)
    {
        return Ok(ready);
    }

    let path = st.dir.join(package_file_name(&version));
    let cached = std::fs::read(&path)
        .ok()
        .filter(|bytes| update.verify(bytes).is_ok());
    if cached.is_none() {
        set_phase(
            app,
            st,
            Phase::Downloading {
                version: version.clone(),
                percent: None,
            },
        );
        let mut received = 0u64;
        let mut shown: Option<u8> = None;
        // `download` verifies the signature before returning.
        let bytes = update
            .download(
                |chunk, total| {
                    received += chunk as u64;
                    let percent = total
                        .filter(|t| *t > 0)
                        .map(|t| (received * 100 / t).min(100) as u8);
                    if percent != shown {
                        shown = percent;
                        set_phase(
                            app,
                            st,
                            Phase::Downloading {
                                version: version.clone(),
                                percent,
                            },
                        );
                    }
                },
                || {},
            )
            .await
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&st.dir).map_err(|e| e.to_string())?;
        let part = path.with_extension("part");
        std::fs::write(&part, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        log::info!("updater: downloaded {}", path.display());
    }
    clear_cache(&st.dir, Some(&version));
    let manifest = serde_json::to_vec(&update.raw_json).map_err(|e| e.to_string())?;
    std::fs::write(st.dir.join(PENDING_FILE), manifest).map_err(|e| e.to_string())?;
    st.inner.lock().ready = Some((update, path));
    Ok(ready)
}

pub fn info(app: &AppHandle) -> UpdateInfo {
    state(app).info()
}

/// Runs the downloaded installer; on success the process exits and the
/// installer restarts the app.
pub fn install(app: &AppHandle) -> Result<(), String> {
    let st = state(app);
    let (update, path) = st
        .inner
        .lock()
        .ready
        .clone()
        .ok_or("No update has been downloaded yet")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    log::info!("updater: installing {}", update.version);
    update.install(bytes).map_err(|e| e.to_string())
}

/// Opens the release page of an available version (portable copies).
pub fn open_release_page(app: &AppHandle) -> Result<(), String> {
    let url = match state(app).info().phase {
        Phase::Available { url, .. } => url,
        _ => format!("{RELEASES_URL}/latest"),
    };
    gc_platform::shell::open_default(Path::new(&url)).map_err(|e| e.to_string())
}
