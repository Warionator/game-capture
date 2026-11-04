// Typed bridge to the Rust backend. Outside Tauri (plain `npm run dev` in a
// browser) every call is served by an in-memory mock so the UI can be
// developed and reviewed without Windows.

import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Action,
  AudioDevice,
  EncoderInfo,
  EngineStatus,
  GameSummary,
  MediaFilter,
  MediaItem,
  MonitorInfo,
  Notice,
  ScanReport,
  ExportRequest,
  Settings,
  SystemInfo,
  TrackFile,
  TrackLevel,
  WindowedProcess,
} from "./types";
import { mock } from "./mock";

export const inTauri = isTauri();

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) return invoke<T>(cmd, args);
  return mock.invoke<T>(cmd, args ?? {});
}

export const api = {
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  getStatus: () => call<EngineStatus | null>("get_status"),
  listMedia: (filter: MediaFilter) => call<MediaItem[]>("list_media", { filter }),
  getMedia: (id: number) => call<MediaItem | null>("get_media", { id }),
  listGames: () => call<GameSummary[]>("list_games"),
  gameIcon: (title: string) => call<string | null>("game_icon", { title }),
  deleteMedia: (id: number) => call<void>("delete_media", { id }),
  revealMedia: (id: number) => call<void>("reveal_media", { id }),
  openMediaExternal: (id: number) => call<void>("open_media_external", { id }),
  copyMedia: (id: number) => call<void>("copy_media", { id }),
  openStorageFolder: () => call<void>("open_storage_folder"),
  rescanLibrary: () => call<ScanReport | null>("rescan_library"),
  listProcesses: () => call<WindowedProcess[]>("list_processes"),
  listAudioApps: () => call<import("./types").AudioApp[]>("list_audio_apps"),
  prepareAudioTracks: (id: number) => call<TrackFile[]>("prepare_audio_tracks", { id }),
  exportClip: (id: number, request: ExportRequest) => call<MediaItem>("export_clip", { id, request }),
  processIcon: (path: string) => call<string | null>("process_icon", { path }),
  addCustomGame: (exePath: string, displayName?: string) =>
    call<Settings>("add_custom_game", { exePath, displayName: displayName ?? null }),
  listEncoders: () => call<EncoderInfo[]>("list_encoders"),
  reprobeEncoders: () => call<void>("reprobe_encoders"),
  listMonitors: () => call<MonitorInfo[]>("list_monitors"),
  listAudioDevices: () => call<AudioDevice[]>("list_audio_devices"),
  getAudioLevels: () => call<TrackLevel[]>("get_audio_levels"),
  setTrackVolume: (title: string, volume: number, muted: boolean) =>
    call<void>("set_track_volume", { title, volume, muted }),
  validateHotkey: (text: string) => call<string>("validate_hotkey", { text }),
  hotkeyErrors: () => call<string[]>("hotkey_errors"),
  action: (name: Action) => call<void>("action", { name }),
  systemInfo: () => call<SystemInfo>("system_info"),
  overlayReady: () => call<void>("overlay_ready"),
  previewOverlay: () => call<void>("preview_overlay"),
};

export interface EventMap {
  status: EngineStatus;
  notice: Notice;
  "media-added": MediaItem;
  "library-changed": ScanReport;
  "scan-progress": boolean;
  navigate: string;
  game: unknown;
  "overlay-toast": { toast: import("./types").OverlayToast; corner: import("./types").OverlayPosition };
}

export async function on<K extends keyof EventMap>(
  event: K,
  handler: (payload: EventMap[K]) => void,
): Promise<UnlistenFn> {
  if (inTauri) return listen<EventMap[K]>(event, (e) => handler(e.payload));
  return mock.listen(event, handler as (p: unknown) => void);
}

/** URL for a local file (thumbnail, video) usable in <img>/<video>. */
export function fileUrl(path: string | null | undefined, version?: number): string | undefined {
  if (!path) return undefined;
  if (!inTauri) return mock.fileUrl(path);
  const url = convertFileSrc(path);
  // Cache-bust after a file was overwritten in place (clip editor).
  return version ? `${url}?v=${version}` : url;
}

export async function pickFolder(title: string): Promise<string | null> {
  if (!inTauri) return window.prompt(title, "D:\\Videos\\game-capture");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, title });
  return typeof r === "string" ? r : null;
}

export async function pickExecutable(): Promise<string | null> {
  if (!inTauri) return window.prompt("Executable path", "C:\\Games\\MyGame\\game.exe");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({
    title: "Choose a game executable",
    filters: [{ name: "Programs", extensions: ["exe"] }],
  });
  return typeof r === "string" ? r : null;
}

/** Toggles OS fullscreen for the app window (the player uses it). */
export async function setWindowFullscreen(on: boolean): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setFullscreen(on);
}
