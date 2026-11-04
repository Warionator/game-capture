// In-browser stand-in for the Rust backend (development / design review).

import type {
  EngineStatus,
  GameSummary,
  MediaFilter,
  MediaItem,
  Settings,
  TrackLevel,
} from "./types";

type Handler = (payload: unknown) => void;
const listeners = new Map<string, Set<Handler>>();

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((h) => h(payload));
}

// Dev hook: lets UI checks push backend events (e.g. overlay popups).
(globalThis as { __gcMockEmit?: typeof emit }).__gcMockEmit = emit;

const HOUR = 3_600_000;
const now = Date.now();

const games: { title: string; hue: number }[] = [
  { title: "Elden Ring", hue: 38 },
  { title: "Fortnite", hue: 265 },
  { title: "Counter-Strike 2", hue: 200 },
  { title: "Forza Horizon 5", hue: 150 },
];

function thumb(hue: number, seed: number): string {
  const h2 = (hue + 40 + seed * 17) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="270" viewBox="0 0 480 270">
  <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
  <stop offset="0" stop-color="hsl(${hue},55%,28%)"/><stop offset="1" stop-color="hsl(${h2},60%,12%)"/></linearGradient></defs>
  <rect width="480" height="270" fill="url(#g)"/>
  <circle cx="${120 + ((seed * 53) % 260)}" cy="${80 + ((seed * 31) % 110)}" r="${40 + (seed % 4) * 12}" fill="hsla(${h2},80%,70%,0.18)"/>
  <path d="M0 ${200 - (seed % 5) * 8} L120 ${150 + (seed % 3) * 10} L230 190 L340 ${130 + (seed % 4) * 9} L480 170 L480 270 L0 270Z" fill="hsla(${hue},40%,6%,0.7)"/>
  </svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

let nextId = 1;
const media: MediaItem[] = [];
for (let i = 0; i < 26; i++) {
  const g = i % 7 === 6 ? null : games[i % games.length];
  const kind = i % 9 === 4 ? "session" : i % 5 === 3 ? "screenshot" : "clip";
  const created = now - i * 5.3 * HOUR - (i % 3) * 17 * 60_000;
  const d = new Date(created);
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}_${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}`;
  const label = kind === "clip" ? "Clip" : kind === "session" ? "Session" : "Screenshot";
  const ext = kind === "screenshot" ? "png" : kind === "session" ? "mkv" : "mp4";
  const durationMs = kind === "screenshot" ? 0 : kind === "session" ? (25 + i * 3) * 60_000 : (30 + (i % 4) * 30) * 1000;
  media.push({
    id: nextId++,
    path: `D:\\Videos\\game-capture\\${g?.title ?? "Uncategorized"}\\${stamp}_${label}.${ext}`,
    kind,
    gameTitle: g?.title ?? null,
    executable: g ? `${g.title.replace(/\W/g, "")}.exe` : null,
    createdMs: created,
    durationMs,
    width: 2560,
    height: 1440,
    sizeBytes: kind === "screenshot" ? 4_200_000 : Math.round((durationMs / 1000) * 3_100_000),
    mtimeMs: created,
    bookmarks:
      kind === "screenshot"
        ? []
        : [
            { offsetMs: Math.round(durationMs * 0.3) },
            ...(i % 2 ? [{ offsetMs: Math.round(durationMs * 0.72), label: "Clutch" }] : []),
          ],
    audioTracks: kind === "screenshot" ? [] : ["Master", "Game", "Microphone", "Discord"],
    videoCodec: kind === "screenshot" ? "png" : "h264",
    thumbnail: thumb(g?.hue ?? 0, i),
  });
}

let settings: Settings = {
  storageRoot: "D:\\Videos\\game-capture",
  extraLibraryFolders: [],
  video: {
    captureMode: "window",
    focusScreen: true,
    allowDesktopCapture: false,
    resolution: "native",
    fps: 60,
    encoder: "auto",
    codec: "h264",
    rateControl: { mode: "vbr", bitrateKbps: 25000, maxKbps: 40000 },
    container: "mp4",
    monitorIndex: 0,
    captureCursor: false,
    zeroCopy: true,
  },
  replay: {
    enabled: true,
    durationSecs: 120,
    storage: "auto",
    maxMemoryMb: 1024,
    diskDir: null,
    autoStart: true,
    quality: { resolution: "1080p", fps: 60, rateControl: { mode: "vbr", bitrateKbps: 20000, maxKbps: 32000 } },
  },
  audio: {
    mixdownEnabled: true,
    game: { enabled: true, volume: 1, muted: false },
    mic: {
      enabled: true,
      volume: 1,
      muted: false,
      deviceId: null,
      noiseSuppression: true,
      noiseSuppressionStrength: 12,
      noiseGate: true,
      noiseGateDb: -45,
    },
    voice: { name: "Discord", executables: ["Discord.exe"], enabled: true, volume: 0.8, muted: false, includeInMix: true },
    extraTracks: [{ name: "Spotify", executables: ["Spotify.exe"], enabled: true, volume: 0.5, muted: false, includeInMix: false }],
    systemAudioFallback: null,
  },
  hotkeys: { saveReplay: "Alt+F10", toggleRecording: "Alt+F9", addBookmark: "Alt+F8", screenshot: "Alt+F12" },
  screenshot: { format: "png", jpegQuality: 92 },
  system: { disableGameMode: true, autostart: true, closeToTray: true, showNotifications: true },
  games: {
    autoDetect: true,
    custom: [{ displayName: "Dolphin (GameCube)", executable: "Dolphin.exe", path: "C:\\Emulators\\Dolphin\\Dolphin.exe" }],
    excluded: [],
  },
  recording: { autoRecordSessions: false },
  overlay: { enabled: true, position: "topLeft", sound: true, volume: 0.6 },
};

let status: EngineStatus = {
  state: "buffering",
  game: { pid: 4242, exeName: "eldenring.exe", displayName: "Elden Ring" },
  recordingSecs: 0,
  replayEnabled: true,
  buffer: { chunks: 92, bytes: 287_000_000, bufferedSecs: 92, windowSecs: 122, onDisk: false },
  encoder: "h264_nvenc",
  encoderLabel: "NVIDIA NVENC H.264",
  zeroCopy: true,
  outputSize: [2560, 1440],
  fps: 60,
  lastError: null,
  ffmpegFound: true,
  pendingRestart: false,
  encodersProbed: true,
  captureSource: "Game window",
  focusLost: false,
  desktopCapture: false,
};

setInterval(() => {
  if (status.state === "recording") status = { ...status, recordingSecs: status.recordingSecs + 1 };
  if (status.buffer && status.buffer.bufferedSecs < status.buffer.windowSecs) {
    status = { ...status, buffer: { ...status.buffer, bufferedSecs: status.buffer.bufferedSecs + 1 } };
  }
  emit("status", status);
}, 1000);

function filterMedia(f: MediaFilter): MediaItem[] {
  let r = media.filter((m) => {
    if (f.game) {
      if (f.game === "Uncategorized" ? m.gameTitle !== null : m.gameTitle?.toLowerCase() !== f.game.toLowerCase())
        return false;
    }
    if (f.kind && m.kind !== f.kind) return false;
    if (f.search) {
      const s = f.search.toLowerCase();
      if (!(m.gameTitle ?? "").toLowerCase().includes(s) && !m.path.toLowerCase().includes(s)) return false;
    }
    if (f.fromMs != null && m.createdMs < f.fromMs) return false;
    if (f.toMs != null && m.createdMs > f.toMs) return false;
    if (f.minDurationMs != null && m.durationMs < f.minDurationMs) return false;
    if (f.maxDurationMs != null && m.durationMs > f.maxDurationMs) return false;
    return true;
  });
  const by: Record<string, (a: MediaItem, b: MediaItem) => number> = {
    newest: (a, b) => b.createdMs - a.createdMs,
    oldest: (a, b) => a.createdMs - b.createdMs,
    longest: (a, b) => b.durationMs - a.durationMs,
    shortest: (a, b) => a.durationMs - b.durationMs,
    largest: (a, b) => b.sizeBytes - a.sizeBytes,
    game: (a, b) => (a.gameTitle ?? "~").localeCompare(b.gameTitle ?? "~") || b.createdMs - a.createdMs,
  };
  r = [...r].sort(by[f.sort ?? "newest"]);
  return r;
}

function gameSummaries(): GameSummary[] {
  const map = new Map<string, GameSummary>();
  for (const m of media) {
    const t = m.gameTitle ?? "Uncategorized";
    const g = map.get(t) ?? { title: t, count: 0, totalBytes: 0, latestMs: 0, hasIcon: m.gameTitle !== null };
    g.count++;
    g.totalBytes += m.sizeBytes;
    g.latestMs = Math.max(g.latestMs, m.createdMs);
    map.set(t, g);
  }
  return [...map.values()].sort((a, b) => b.latestMs - a.latestMs);
}

function icon(title: string): string {
  const g = games.find((x) => x.title === title);
  const hue = g?.hue ?? 0;
  const letter = title[0] ?? "?";
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" rx="14" fill="hsl(${hue},55%,35%)"/><text x="32" y="43" font-family="Segoe UI,Arial" font-size="30" font-weight="700" text-anchor="middle" fill="white">${letter}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

let levelPhase = 0;
function levels(): TrackLevel[] {
  levelPhase += 0.35;
  const wave = (k: number) => Math.max(0, Math.min(1, 0.35 + 0.3 * Math.sin(levelPhase * k) + Math.random() * 0.25));
  return [
    { title: "Game", peak: settings.audio.game.muted ? 0 : wave(1.1) * settings.audio.game.volume, volume: settings.audio.game.volume, muted: settings.audio.game.muted, sources: 1, gateOpen: null },
    { title: "Microphone", peak: settings.audio.mic.muted ? 0 : wave(2.3) * 0.6, volume: settings.audio.mic.volume, muted: settings.audio.mic.muted, sources: 1, gateOpen: settings.audio.mic.noiseGate ? Math.sin(levelPhase * 2.3) > -0.2 : null },
    { title: "Discord", peak: wave(0.7) * 0.5, volume: settings.audio.voice.volume, muted: settings.audio.voice.muted, sources: 1, gateOpen: null },
    { title: "Spotify", peak: wave(0.4) * 0.4, volume: 0.5, muted: false, sources: 0, gateOpen: null },
  ];
}

function notice(message: string, error = false) {
  setTimeout(() => emit("notice", { message, error }), 150);
}

const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
  get_settings: () => structuredClone(settings),
  save_settings: (a) => {
    settings = structuredClone(a.settings as Settings);
    return structuredClone(settings);
  },
  get_status: () => status,
  list_media: (a) => filterMedia(a.filter as MediaFilter),
  get_media: (a) => media.find((m) => m.id === a.id) ?? null,
  list_games: () => gameSummaries(),
  game_icon: (a) => (a.title === "Uncategorized" ? null : icon(a.title as string)),
  delete_media: (a) => {
    const i = media.findIndex((m) => m.id === a.id);
    if (i >= 0) media.splice(i, 1);
  },
  reveal_media: () => notice("(mock) Explorer would open with the file selected"),
  open_media_external: () => notice("(mock) File would open in the default player"),
  copy_media: () => notice("Copied to clipboard"),
  open_storage_folder: () => notice("(mock) Library folder would open"),
  rescan_library: () => ({ added: 0, updated: 0, removed: 0, unchanged: media.length, thumbnailsRebuilt: 0, errors: [] }),
  list_processes: () => [
    { pid: 4242, exeName: "eldenring.exe", exePath: "D:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game\\eldenring.exe", windowTitle: "ELDEN RING™", displayName: "ELDEN RING™" },
    { pid: 5120, exeName: "RetroArch.exe", exePath: "C:\\RetroArch\\RetroArch.exe", windowTitle: "RetroArch", displayName: "RetroArch" },
    { pid: 6001, exeName: "Discord.exe", exePath: "C:\\Users\\me\\AppData\\Local\\Discord\\Discord.exe", windowTitle: "#general - Discord", displayName: "Discord" },
  ],
  process_icon: () => null,
  prepare_audio_tracks: (a) => {
    const m = media.find((x) => x.id === a.id);
    if (!m || m.audioTracks.length < 2) return [];
    return m.audioTracks.map((title, index) => ({ index, title, path: "" }));
  },
  overlay_ready: () => undefined,
  preview_overlay: () => notice("(mock) An in-game popup would appear now"),
  export_clip: (a) => {
    const m = media.find((x) => x.id === a.id)!;
    const r = a.request as { startMs: number; endMs: number; overwrite: boolean; asClip?: boolean };
    if (r.asClip) {
      const clip: MediaItem = { ...m, id: nextId++, kind: "clip", durationMs: r.endMs - r.startMs, createdMs: m.createdMs + r.startMs, path: m.path.replace(/_Session\.(\w+)$/, "_Clip.$1"), bookmarks: [] };
      media.unshift(clip);
      setTimeout(() => emit("library-changed", {}), 50);
      return clip;
    }
    const out = { ...m, durationMs: r.endMs - r.startMs, mtimeMs: Date.now() };
    if (r.overwrite) Object.assign(m, out);
    else media.unshift({ ...out, id: nextId++, path: m.path.replace(/(\.\w+)$/, "_edit$1") });
    setTimeout(() => emit("library-changed", {}), 50);
    return r.overwrite ? m : media[0];
  },
  add_custom_game: (a) => {
    const path = a.exePath as string;
    const exe = path.split("\\").pop() ?? path;
    settings.games.custom = [
      ...settings.games.custom.filter((g) => g.executable.toLowerCase() !== exe.toLowerCase()),
      { displayName: (a.displayName as string) || exe.replace(/\.exe$/i, ""), executable: exe, path },
    ];
    return structuredClone(settings);
  },
  list_encoders: () =>
    (["nvenc", "amf", "qsv", "software"] as const).flatMap((b) =>
      (["h264", "hevc", "av1"] as const).map((c) => ({
        backend: b,
        codec: c,
        name: `${b === "software" ? { h264: "libx264", hevc: "libx265", av1: "libsvtav1" }[c] : `${c}_${b}`}`,
        label: `${{ nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel QuickSync", software: "Software (CPU)" }[b]} ${{ h264: "H.264", hevc: "HEVC", av1: "AV1" }[c]}`,
        compiled: true,
        working: b === "nvenc" || b === "software" || (b === "qsv" && c !== "av1"),
      })),
    ),
  reprobe_encoders: () => undefined,
  list_monitors: () => [
    { adapterIdx: 0, outputIdx: 0, globalIdx: 0, deviceName: "\\\\.\\DISPLAY1", adapterName: "NVIDIA GeForce RTX 4070", x: 0, y: 0, width: 2560, height: 1440, primary: true },
    { adapterIdx: 0, outputIdx: 1, globalIdx: 1, deviceName: "\\\\.\\DISPLAY2", adapterName: "NVIDIA GeForce RTX 4070", x: 2560, y: 0, width: 1920, height: 1080, primary: false },
  ],
  list_audio_devices: () => [
    { id: "{mic-1}", name: "Microphone (Shure MV7)", isDefault: true },
    { id: "{mic-2}", name: "Headset Microphone (Arctis 7)", isDefault: false },
  ],
  get_audio_levels: () => levels(),
  set_track_volume: () => undefined,
  validate_hotkey: (a) => {
    const t = String(a.text).trim();
    if (!t || /^(ctrl|alt|shift|win)(\+(ctrl|alt|shift|win))*$/i.test(t)) throw new Error("hotkey needs a non-modifier key");
    return t;
  },
  hotkey_errors: () => [],
  action: (a) => {
    switch (a.name) {
      case "saveReplay":
        notice("Saving last 120 s…");
        setTimeout(() => notice("Clip saved: 2026-11-04_13-37-00_Clip.mp4"), 900);
        break;
      case "toggleRecording":
        status = status.state === "recording"
          ? { ...status, state: "buffering", recordingSecs: 0 }
          : { ...status, state: "recording", recordingSecs: 0 };
        notice(status.state === "recording" ? "Recording started" : "Recording stopped – saving…");
        break;
      case "bookmark":
        notice("Bookmark added");
        break;
      case "screenshot":
        notice("Screenshot saved: 2026-11-04_14-12-05_Screenshot.png");
        break;
      case "pause":
        status = { ...status, state: "paused", buffer: null };
        break;
      case "resume":
        status = { ...status, state: "buffering", buffer: { chunks: 0, bytes: 0, bufferedSecs: 0, windowSecs: 122, onDisk: false } };
        break;
    }
    emit("status", status);
  },
  system_info: () => ({
    version: "0.1.0",
    windowsBuild: 22631,
    processLoopback: true,
    gameModeEnabled: false,
    autostartEnabled: true,
    ffmpegPath: "C:\\Program Files\\game-capture\\binaries\\ffmpeg.exe",
    settingsPath: "C:\\Users\\me\\AppData\\Roaming\\com.warionator.game-capture\\settings.json",
    libraryItems: media.length,
    platform: "windows (mock)",
  }),
};

export const mock = {
  async invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
    const h = handlers[cmd];
    if (!h) throw new Error(`mock: unknown command ${cmd}`);
    await new Promise((r) => setTimeout(r, 20));
    return h(args) as T;
  },
  listen(event: string, handler: Handler): Promise<() => void> {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event)!.add(handler);
    return Promise.resolve(() => listeners.get(event)?.delete(handler));
  },
  fileUrl(path: string): string | undefined {
    // Only generated placeholders are viewable in the browser mock. For UI
    // work, set `window.__GC_MOCK_VIDEO__` to a sample video URL.
    if (path.startsWith("data:")) return path;
    const sample = (globalThis as { __GC_MOCK_VIDEO__?: string }).__GC_MOCK_VIDEO__;
    return sample && /\.(mp4|mkv)$/i.test(path) ? sample : undefined;
  },
};
