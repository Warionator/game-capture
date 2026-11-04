# game-capture

Lightweight, low-overhead game clipping and recording for Windows 10/11.
Replay-buffer clips, long session recordings, per-application audio tracks
and a local media library — no accounts, uploads, overlays or social features.

Built with **Rust + Tauri v2** (small idle footprint) and a **React +
Tailwind** UI. Video is captured with DXGI Desktop Duplication and encoded on
the GPU (NVENC / AMF / QuickSync) through a bundled FFmpeg; audio is captured
per process with the WASAPI process-loopback API.

## Features

| Area | What it does |
| --- | --- |
| Game detection | Polls processes, inspects the foreground process for loaded D3D9/11/12, Vulkan or OpenGL modules, fullscreen windows, launcher install paths and Microsoft Store packages. Names come from version info / package manifests / install folders; icons are extracted to PNG. Manual override: pick a running window or browse for an `.exe`. |
| Capture | Games are captured with **Windows Graphics Capture of the game window only** (`gfxcapture`), so other apps and notifications never appear; monitor capture (`ddagrab`, Desktop Duplication) is the fallback and an option. D3D11 textures go directly to the hardware encoder — no CPU pixel copies and **no API hooks / DLL injection**, so anti-cheat is not triggered. GPU-side scaling via `scale_d3d11` / `vpp_amf` / `scale_qsv`. |
| Focus screen | While the game is not the foreground window, the recording switches (live, via `streamselect`) to a "Game not in focus" screen. |
| Encoders | Probed at startup by encoding test frames: NVENC → AMF → QuickSync → software (x264 / x265 / SVT-AV1). H.264, HEVC and AV1. If a configuration fails to start, the engine retries without zero-copy and then with the next encoder. |
| Audio | Track 1 master mix · Track 2 game (process tree only) · Track 3 microphone (device choice, adjustable noise suppression, live noise gate with threshold marker on the meter) · Track 4 Discord (by executable) · any number of extra app tracks. Live per-track gain, mute and peak meters. |
| Replay buffer | 15 s – 20 min of keyframe-aligned MPEG-TS chunks kept in RAM, spilling to disk (ideally NVMe) when the window does not fit the RAM budget. Saving is a stream copy — no re-encode. |
| Recording | Session recording shares the same encoder as the replay buffer (one GPU encode). Written crash-safely as MPEG-TS and remuxed to MP4/MKV on stop; interrupted sessions are recovered on next launch. |
| Hotkeys | Global low-level keyboard hook (`SetWindowsHookEx`): save replay, toggle recording, bookmark, screenshot. |
| Bookmarks | Stored in the embedded metadata and as container chapters; shown as markers on the player timeline. |
| Screenshots | PNG/JPEG straight from the duplicated GPU framebuffer. |
| Library | `<root>/<Game>/<YYYY-MM-DD_HH-MM-SS>_<Kind>.<ext>` (Kind = Clip, Session or Screenshot). Metadata (`game_title`, `executable_name`, `timestamp`, `duration`, `bookmarks`, `resolution`, …) is embedded in every file. A SQLite cache makes the gallery instant; the self-healing scanner rebuilds it — including thumbnails — from the files alone after a reinstall or backup restore. |
| Gallery | Grid grouped by game or date, filters for game, type, date, duration, search and sort; right-click: open location, open externally, copy file to clipboard, details/bookmarks, delete. Only the library folder and folders you add are shown. |
| Player & editor | Single click opens the player: bookmark timeline, Master mix by default, or the individual tracks with their own volume/mute. Drag the handles to trim; save the trim and track levels over the original or as a copy (lossless video copy). |
| System | Optional *Disable Windows Game Mode* (`HKCU\Software\Microsoft\GameBar`), start with Windows (`HKCU\…\Run`, `--minimized`), tray icon with Pause / Save Replay / Dashboard / Settings / Quit. |

## Repository layout

```
crates/
  gc-core      Platform-independent logic: settings, naming, hotkey parsing,
               FFmpeg argument construction, MPEG-TS chunker, replay ring,
               metadata model, game-detection heuristics. Fully unit-tested.
  gc-library   SQLite index + self-healing scanner + thumbnails.
  gc-platform  Windows: process watcher, icons, keyboard hook, registry,
               Explorer/clipboard helpers, DXGI monitor enumeration.
  gc-audio     WASAPI process/system loopback + microphone, clock-driven
               mixer, named-pipe delivery to FFmpeg.
  gc-capture   Capture engine: FFmpeg process, encoder probing & fallback,
               replay save, session recording, screenshots.
src-tauri/     Tauri v2 shell: commands, tray, event wiring.
ui/            React 19 + Tailwind v4 frontend (runs standalone with a mock
               backend via `npm --prefix ui run dev`).
docs/          Architecture notes.
scripts/       FFmpeg download helper.
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## Building (Windows)

Prerequisites: Rust (stable, MSVC toolchain), Node.js 20+, the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) (WebView2 is
preinstalled on Windows 10/11).

```powershell
npm install                 # root (Tauri CLI) + ui dependencies
npm run fetch-ffmpeg        # downloads ffmpeg.exe/ffprobe.exe into src-tauri/binaries
npm run dev                 # run the app with hot reload
npm run build               # NSIS installer in target/release/bundle/nsis
```

`npm run build` passes `src-tauri/tauri.bundle.conf.json`, which bundles the
FFmpeg binaries. Plain `cargo build` works without them; the app then looks
for FFmpeg next to the executable and on `PATH`.

### Developing on Linux/macOS

The core crates and the UI build anywhere:

```bash
cargo test -p gc-core -p gc-library -p gc-platform -p gc-audio -p gc-capture
npm --prefix ui run dev     # UI against an in-browser mock backend
```

With `ffmpeg` on `PATH`, the end-to-end tests run the real pipeline
(synthetic video source → encoder → chunker → ring → MP4/MKV with embedded
metadata and chapters → ffprobe). The Windows code is type-checked with
`cargo clippy --workspace --target x86_64-pc-windows-gnu`.

## Notes & limitations

- **Per-app audio** needs Windows 10 2004 (build 19041) or newer. On older
  builds the game track can fall back to the full system mix.
- **Elevated games**: Windows does not deliver keyboard input from an
  administrator window to non-elevated hooks; run game-capture elevated for
  such games.
- **Capture source**: Desktop Duplication captures the monitor the game is on
  (cropped for windowed games). Exclusive-fullscreen games on a monitor other
  than the one being duplicated are not captured.
- **HDR**: the desktop is captured as 8-bit BGRA; HDR output is tone-mapped by
  Windows.
- **Preview player**: uses WebView2's decoders. HEVC/AV1 need the matching
  Microsoft Store video extensions; anything else can be opened in the default
  external player from the context menu.
- **FFmpeg licensing**: the fetch script downloads GPL builds (x264/x265).
  Redistributing installers that bundle them means complying with the GPL;
  use an LGPL build if that is a concern (hardware encoders still work).
