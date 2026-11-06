# Architecture

## Process & thread model

```
                         ┌──────────────── game-capture.exe ────────────────┐
 Windows                 │                                                  │
 ───────                 │  gc-process-watcher ─ WatchEvent ─┐              │
 Toolhelp snapshots ───► │                                   ▼              │
 foreground window       │  gc-hotkeys (WH_KEYBOARD_LL) ─► gc-capture-engine│◄── Tauri commands
                         │                                   │  (one worker │    (UI, tray)
                         │                                   │   thread)    │
                         │            ┌──────────────────────┘              │
                         │            ▼                                     │
                         │   Pipeline ── spawns ──► ffmpeg.exe ─────────────┼─► GPU
                         │     │  gc-ffmpeg-reader ◄── MPEG-TS (stdout)     │
                         │     │     └─► TsChunker ─► ReplayRing (RAM/disk) │
                         │     │                  └─► SessionWriter (.ts)   │
                         │     └─ AudioEngine                               │
                         │          gc-wasapi × sources ─► SampleRing       │
                         │          gc-audio-pump (10 ms clock, gain, peak) │
                         │          gc-audio-writer × tracks ─► \\.\pipe\…  │──► ffmpeg inputs
                         └──────────────────────────────────────────────────┘
```

The engine worker serialises every state change (game started/stopped,
settings, hotkey actions, pause), so there are no races between e.g. a
settings change and a replay save. Expensive work — remuxing a clip,
finalising a session, screenshots — runs on short-lived threads and reports
back through `EngineEvent`s.

## Video path

`gc-core::ffmpeg::PipelineSpec::build_args` produces the FFmpeg command. The
capture source is the `ddagrab` filter (DXGI Desktop Duplication), which
outputs `AV_PIX_FMT_D3D11` frames:

| Backend | Native resolution | Downscaled |
| --- | --- | --- |
| NVENC / AMF | D3D11 frames passed straight to the encoder | `scale_d3d11` (FFmpeg ≥ 8, D3D11 video processor, also BGRA→NV12) or `vpp_amf` |
| QuickSync | `hwmap=derive_device=qsv` | `scale_qsv` after mapping |
| Software | `hwdownload` → `x264`/`x265`/`SVT-AV1` | CPU `scale` |

If the zero-copy graph fails (driver/FFmpeg mismatch) the engine retries the
same encoder with a download path, then the next backend. Encoders are
verified at startup by encoding three synthetic frames each (in parallel).

Keyframes are forced every second (`-g fps` / `-force_key_frames`), which is
the granularity of the replay buffer.

## Replay buffer

FFmpeg writes MPEG-TS to stdout. `TsChunker` parses packets, finds the video
PID from the PMT and cuts the stream at each keyframe's PES start — more
precisely at the PAT that FFmpeg emits immediately before every video
keyframe, so each chunk is independently decodable. Each chunk carries the
keyframe PTS.

`ReplayRing` keeps chunks while the remaining chunks still cover the window
(timing from PTS deltas, falling back to wall-clock on discontinuities). In
memory mode chunks are `Arc<[u8]>`; in disk mode each chunk is a file whose
`Drop` deletes it, so an in-flight save holds its chunks alive even while the
ring evicts them.

Saving a replay snapshots the chunk references (+ the in-progress chunk) and
pipes them into `ffmpeg -c copy` producing MP4 (`+faststart+use_metadata_tags`)
or MKV with metadata tags, per-track titles and bookmark chapters.

## Session recording

The session writer appends the same chunks to a hidden
`.<name>.recording.ts` with a JSON sidecar (metadata + bookmarks, rewritten on
each bookmark). Stopping remuxes it to the final container. On startup
`recover_sessions` finalises any leftovers — a crash loses at most the last
partial chunk.

## Audio path

Each capture source (process loopback for a PID tree, system loopback, or a
microphone endpoint) runs its own WASAPI client in shared mode with
`AUTOCONVERTPCM`, so Windows resamples to 48 kHz stereo float. Sources push
into bounded `SampleRing`s.

The pump thread runs on a 10 ms clock and emits exactly 48 000 frames/s per
track regardless of whether a source produced anything (process loopback is
silent — produces no packets — when the app is quiet). A 30 ms jitter
cushion per source avoids clicks. Live gain/mute live in atomics
(`TrackControl`) so UI changes never restart the pipeline.

Each track is written to an outbound named pipe that FFmpeg opens as a raw
`f32le` input. Writers wait until every pipe is connected and discard
pre-connection audio so tracks start aligned. In FFmpeg, inputs marked "in
mix" are split: one copy is its own track, the other goes into `amix`
(+ `alimiter`) for the Master track. The microphone can be denoised with
`afftdn`.

The resolver re-scans processes every 2 s, so an app started after capture
began (e.g. Discord) is picked up automatically.

## Library

Files are the source of truth. `gc-library` keeps a SQLite cache
(`media`, `game_icons`). `scan()` walks the library root, compares size and
mtime with the cache, probes new/changed files with ffprobe and regenerates
missing thumbnails. Metadata is resolved in order:

1. the `GC_METADATA` JSON tag (written by game-capture),
2. plain tags (`game_title`, `duration`, `bookmarks`…),
3. the file name (`YYYY-MM-DD_HH-MM-SS_<Kind>`),
4. the parent folder (game) and file mtime.

Chapters written by other tools are imported as bookmarks.

## Windows integration

- **Game detection** (`gc-platform::watcher` + `gc-core::detect`): only the
  foreground process and user-registered executables are inspected in depth.
  Rendering fullscreen is never enough on its own (video players, browsers
  and editors do that too); a game needs positive evidence. Strong: Windows
  Game Bar lists the exe as a game (`HKCU\System\GameConfigStore\Children`)
  or it is installed in a launcher library (`steamapps\common`, Epic, Xbox…).
  Weak, only counted with a fullscreen/borderless foreground window: game
  engine/SDK/anti-cheat DLLs (`UnityPlayer.dll`, `steam_api64.dll`…), an
  Unreal `-Shipping` exe, a `Games` folder, or a Store package whose modules
  can't be read. A readable module list must also include a graphics API.
  Known non-games (players, editors, the Steam build of Godot…) are excluded.
- **Hotkeys**: one `WH_KEYBOARD_LL` hook on a dedicated message-loop thread;
  the hook only matches and forwards, never blocks, and does not swallow keys.
- **Registry**: Game Mode (`AllowAutoGameMode` and `AutoGameModeEnabled`)
  and the `Run` key. Debug builds never register autostart.
- **Clipboard**: `CF_HDROP` + `Preferred DropEffect = COPY`, so a clip can be
  pasted into Explorer, Discord, etc.
