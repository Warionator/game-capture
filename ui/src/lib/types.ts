// Mirrors the serde (camelCase) shapes of the Rust backend.

export type Resolution = "native" | "2160p" | "1440p" | "1080p" | "720p";
export type EncoderPreference = "auto" | "nvenc" | "amf" | "qsv" | "software";
export type Codec = "h264" | "hevc" | "av1";
export type Container = "mp4" | "mkv";
export type BufferStorage = "auto" | "memory" | "disk";
export type ImageFormat = "png" | "jpeg";
export type MediaKind = "clip" | "session" | "screenshot";
export type Backend = "nvenc" | "amf" | "qsv" | "software";

export type RateControl =
  | { mode: "cbr"; bitrateKbps: number }
  | { mode: "vbr"; bitrateKbps: number; maxKbps: number }
  | { mode: "quality"; level: number };

export interface AudioSource {
  enabled: boolean;
  volume: number;
  muted: boolean;
}

export interface AppTrack extends AudioSource {
  name: string;
  executables: string[];
  includeInMix: boolean;
}

export interface CustomGame {
  displayName: string;
  executable: string;
  path: string | null;
}

export interface Settings {
  storageRoot: string;
  video: {
    resolution: Resolution;
    fps: number;
    encoder: EncoderPreference;
    codec: Codec;
    rateControl: RateControl;
    container: Container;
    monitorIndex: number;
    captureCursor: boolean;
    zeroCopy: boolean;
  };
  replay: {
    enabled: boolean;
    durationSecs: number;
    storage: BufferStorage;
    maxMemoryMb: number;
    diskDir: string | null;
    autoStart: boolean;
  };
  audio: {
    mixdownEnabled: boolean;
    game: AudioSource;
    mic: AudioSource & { deviceId: string | null; noiseSuppression: boolean };
    voice: AppTrack;
    extraTracks: AppTrack[];
    systemAudioFallback: boolean;
  };
  hotkeys: {
    saveReplay: string;
    toggleRecording: string;
    addBookmark: string;
    screenshot: string;
  };
  screenshot: { format: ImageFormat; jpegQuality: number };
  system: {
    disableGameMode: boolean;
    autostart: boolean;
    closeToTray: boolean;
    showNotifications: boolean;
  };
  games: { autoDetect: boolean; custom: CustomGame[]; excluded: string[] };
}

export interface Bookmark {
  offsetMs: number;
  label?: string | null;
}

export interface MediaItem {
  id: number;
  path: string;
  kind: MediaKind;
  gameTitle: string | null;
  executable: string | null;
  createdMs: number;
  durationMs: number;
  width: number | null;
  height: number | null;
  sizeBytes: number;
  mtimeMs: number;
  bookmarks: Bookmark[];
  audioTracks: string[];
  videoCodec: string | null;
  thumbnail: string | null;
}

export type SortOrder = "newest" | "oldest" | "longest" | "shortest" | "largest" | "game";

export interface MediaFilter {
  game?: string | null;
  kind?: MediaKind | null;
  search?: string | null;
  fromMs?: number | null;
  toMs?: number | null;
  minDurationMs?: number | null;
  maxDurationMs?: number | null;
  sort?: SortOrder;
  limit?: number | null;
  offset?: number | null;
}

export interface GameSummary {
  title: string;
  count: number;
  totalBytes: number;
  latestMs: number;
  hasIcon: boolean;
}

export type CaptureState = "idle" | "starting" | "buffering" | "recording" | "paused" | "error";

export interface RingStats {
  chunks: number;
  bytes: number;
  bufferedSecs: number;
  windowSecs: number;
  onDisk: boolean;
}

export interface EngineStatus {
  state: CaptureState;
  game: { pid: number; exeName: string; displayName: string } | null;
  recordingSecs: number;
  replayEnabled: boolean;
  buffer: RingStats | null;
  encoder: string | null;
  encoderLabel: string | null;
  zeroCopy: boolean;
  outputSize: [number, number] | null;
  fps: number;
  lastError: string | null;
  ffmpegFound: boolean;
  pendingRestart: boolean;
  encodersProbed: boolean;
}

export interface EncoderInfo {
  backend: Backend;
  codec: Codec;
  name: string;
  label: string;
  compiled: boolean;
  working: boolean;
}

export interface MonitorInfo {
  adapterIdx: number;
  outputIdx: number;
  globalIdx: number;
  deviceName: string;
  adapterName: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
}

export interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

export interface TrackLevel {
  title: string;
  peak: number;
  volume: number;
  muted: boolean;
  sources: number;
}

export interface WindowedProcess {
  pid: number;
  exeName: string;
  exePath: string | null;
  windowTitle: string;
  displayName: string | null;
}

export interface ScanReport {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  thumbnailsRebuilt: number;
  errors: string[];
}

export interface SystemInfo {
  version: string;
  windowsBuild: number | null;
  processLoopback: boolean;
  gameModeEnabled: boolean | null;
  autostartEnabled: boolean;
  ffmpegPath: string | null;
  settingsPath: string;
  libraryItems: number;
  platform: string;
}

export interface Notice {
  error: boolean;
  message: string;
}

export type Action =
  | "saveReplay"
  | "toggleRecording"
  | "bookmark"
  | "screenshot"
  | "startCapture"
  | "stopCapture"
  | "pause"
  | "resume";
