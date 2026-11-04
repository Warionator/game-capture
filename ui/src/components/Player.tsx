import {
  ArrowLeftToLine,
  ArrowRightToLine,
  AudioLines,
  Bookmark,
  ExternalLink,
  FolderOpen,
  Loader2,
  Maximize,
  Minimize,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Save,
  Scissors,
  Trash2,
  Volume2,
  VolumeX,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, fileUrl, setWindowFullscreen } from "../lib/api";
import { cx, fileName, formatDate, formatDuration } from "../lib/format";
import { useStore } from "../lib/store";
import type { MediaItem, TrackFile, TrackGain } from "../lib/types";
import { Button, IconButton, Modal } from "./ui";

const MIN_CLIP_MS = 1000;

type TrackState = Record<number, { volume: number; muted: boolean }>;

/** Plays the isolated tracks of a clip in sync with the (muted) video. */
function useTrackPlayback(
  video: React.RefObject<HTMLVideoElement | null>,
  tracks: TrackFile[] | null,
  active: boolean,
  state: TrackState,
  master: number,
) {
  const els = useRef<HTMLAudioElement[]>([]);

  useEffect(() => {
    const v = video.current;
    if (!active || !tracks || !v) return;
    const audios = tracks.map((t) => {
      const a = new Audio(fileUrl(t.path));
      a.preload = "auto";
      return a;
    });
    els.current = audios;
    const sync = (force: boolean) =>
      audios.forEach((a) => {
        if (force || Math.abs(a.currentTime - v.currentTime) > 0.15) a.currentTime = v.currentTime;
      });
    const onPlay = () => {
      sync(true);
      audios.forEach((a) => void a.play().catch(() => {}));
    };
    const onPause = () => audios.forEach((a) => a.pause());
    const onSeeked = () => sync(true);
    const onTime = () => !v.paused && sync(false);
    const onRate = () => audios.forEach((a) => (a.playbackRate = v.playbackRate));
    v.addEventListener("play", onPlay);
    v.addEventListener("pause", onPause);
    v.addEventListener("seeked", onSeeked);
    v.addEventListener("timeupdate", onTime);
    v.addEventListener("ratechange", onRate);
    if (!v.paused) onPlay();
    else sync(true);
    return () => {
      v.removeEventListener("play", onPlay);
      v.removeEventListener("pause", onPause);
      v.removeEventListener("seeked", onSeeked);
      v.removeEventListener("timeupdate", onTime);
      v.removeEventListener("ratechange", onRate);
      audios.forEach((a) => {
        a.pause();
        a.removeAttribute("src");
        a.load();
      });
      els.current = [];
    };
  }, [video, tracks, active]);

  useEffect(() => {
    if (!tracks) return;
    els.current.forEach((a, i) => {
      const s = state[tracks[i].index] ?? { volume: 1, muted: false };
      a.volume = Math.max(0, Math.min(1, s.volume * master));
      a.muted = s.muted;
    });
  }, [state, master, tracks, active]);

  return els;
}

function Timeline({
  duration,
  time,
  trim,
  setTrim,
  bookmarks,
  seek,
  preview,
  editable,
  onClip,
  clipBusy,
}: {
  duration: number;
  time: number;
  trim: [number, number];
  setTrim: (t: [number, number]) => void;
  bookmarks: MediaItem["bookmarks"];
  /** Seek (kept inside the crop while cropping). */
  seek: (secs: number) => void;
  /** Unclamped seek used while dragging a crop handle. */
  preview: (secs: number) => void;
  editable: boolean;
  /** Sessions: cut the selection into a new clip. */
  onClip?: () => void;
  clipBusy?: boolean;
}) {
  const bar = useRef<HTMLDivElement>(null);
  const drag = useRef<"start" | "end" | "seek" | null>(null);
  const durMs = duration * 1000;
  const pct = (ms: number) => (durMs > 0 ? Math.max(0, Math.min(100, (ms / durMs) * 100)) : 0);
  const msAt = (clientX: number) => {
    const r = bar.current!.getBoundingClientRect();
    return Math.max(0, Math.min(durMs, ((clientX - r.left) / r.width) * durMs));
  };
  const move = (clientX: number) => {
    const ms = msAt(clientX);
    // Dragging a handle also previews that frame.
    if (drag.current === "start") {
      const s = Math.min(ms, trim[1] - MIN_CLIP_MS);
      setTrim([s, trim[1]]);
      preview(s / 1000);
    } else if (drag.current === "end") {
      const e = Math.max(ms, trim[0] + MIN_CLIP_MS);
      setTrim([trim[0], e]);
      preview(e / 1000);
    }
    else if (drag.current === "seek") seek(ms / 1000);
  };
  const start = (kind: "start" | "end" | "seek") => (e: React.PointerEvent) => {
    e.stopPropagation();
    e.preventDefault();
    drag.current = kind;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    if (kind === "seek") move(e.clientX);
  };
  const handlers = {
    onPointerMove: (e: React.PointerEvent) => drag.current && move(e.clientX),
    onPointerUp: () => (drag.current = null),
    onPointerCancel: () => (drag.current = null),
  };
  const trimmed = editable && (trim[0] > 0 || trim[1] < durMs - 1);

  return (
    <div ref={bar} className="relative h-9 cursor-pointer touch-none select-none" onPointerDown={start("seek")} {...handlers}>
      {onClip && trimmed && (
        <div
          className="absolute bottom-full z-20 mb-2 flex -translate-x-1/2 flex-col items-center"
          style={{ left: `${(pct(trim[0]) + pct(trim[1])) / 2}%` }}
        >
          <button
            onPointerDown={(e) => e.stopPropagation()}
            onClick={onClip}
            disabled={clipBusy}
            title="Create a new clip from the selection"
            className="flex items-center gap-1 rounded-lg border border-white/80 bg-panel px-2.5 py-1 text-xs font-semibold text-white shadow-lg hover:bg-hover disabled:opacity-60"
          >
            {clipBusy ? <Loader2 size={13} className="animate-spin" /> : <Plus size={13} />} Clip
          </button>
          <span className="h-2 w-px bg-white/80" />
        </div>
      )}
      <div className="absolute top-1/2 right-0 left-0 h-2 -translate-y-1/2 rounded bg-line" />
      {/* Kept selection */}
      <div
        className={cx("absolute top-1/2 h-2 -translate-y-1/2 rounded", trimmed ? "bg-sky-500/35" : "bg-line-strong")}
        style={{ left: `${pct(trim[0])}%`, width: `${pct(trim[1]) - pct(trim[0])}%` }}
      />
      {/* Played part (inside the crop while cropping). */}
      <div
        className="absolute top-1/2 h-2 -translate-y-1/2 rounded-l bg-accent"
        style={{
          left: `${trimmed ? pct(trim[0]) : 0}%`,
          width: `${Math.max(0, pct(time * 1000) - (trimmed ? pct(trim[0]) : 0))}%`,
        }}
      />
      {bookmarks.map((b, i) => (
        <button
          key={i}
          title={`${b.label ?? `Bookmark ${i + 1}`} · ${formatDuration(b.offsetMs)}`}
          onPointerDown={(e) => {
            e.stopPropagation();
            seek(b.offsetMs / 1000);
          }}
          className="absolute top-1 h-7 w-2 -translate-x-1/2"
          style={{ left: `${pct(b.offsetMs)}%` }}
        >
          <span className="mx-auto block h-full w-0.5 rounded bg-amber-400" />
        </button>
      ))}
      <div
        className="pointer-events-none absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white shadow"
        style={{ left: `${pct(time * 1000)}%` }}
      />
      {editable &&
        (["start", "end"] as const).map((k) => (
          <div
            key={k}
            role="slider"
            aria-label={k === "start" ? "Trim start" : "Trim end"}
            aria-valuenow={Math.round((k === "start" ? trim[0] : trim[1]) / 1000)}
            title={`${k === "start" ? "Start" : "End"}: ${formatDuration(k === "start" ? trim[0] : trim[1])} – drag to trim`}
            onPointerDown={start(k)}
            {...handlers}
            className="absolute top-0 z-10 flex h-9 w-3.5 -translate-x-1/2 cursor-ew-resize items-center justify-center rounded-sm bg-sky-400 shadow-lg hover:bg-sky-300"
            style={{ left: `${pct(k === "start" ? trim[0] : trim[1])}%` }}
          >
            <span className="h-4 w-0.5 rounded bg-slate-900/60" />
          </div>
        ))}
    </div>
  );
}

/** Built-in viewer and simple editor: trim by dragging the handles, adjust
 *  individual audio tracks, save over the original or as a copy. */
export function Player({
  item: initial,
  startMs,
  onClose,
  onDeleted,
}: {
  item: MediaItem;
  startMs?: number;
  onClose: () => void;
  onDeleted: (id: number) => void;
}) {
  const { notify, confirm } = useStore();
  const [item, setItem] = useState(initial);
  const video = useRef<HTMLVideoElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [clipping, setClipping] = useState(false);
  const tracksPromise = useRef<Promise<TrackFile[]> | null>(null);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(initial.durationMs / 1000);
  const [playing, setPlaying] = useState(false);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [failed, setFailed] = useState(false);
  const [released, setReleased] = useState(false);
  const [trim, setTrim] = useState<[number, number]>([0, initial.durationMs]);
  const [tracks, setTracks] = useState<TrackFile[] | null>(null);
  const [trackMode, setTrackMode] = useState(false);
  const [loadingTracks, setLoadingTracks] = useState(false);
  const [trackState, setTrackState] = useState<TrackState>({});
  const [saveMenu, setSaveMenu] = useState(false);
  const [saving, setSaving] = useState(false);

  const isImage = item.kind === "screenshot";
  const src = released ? undefined : fileUrl(item.path, item.mtimeMs);
  const durMs = duration * 1000;
  const multiTrack = item.audioTracks.length > 1;

  useTrackPlayback(video, tracks, trackMode, trackState, volume);

  // Master (video element) audio: muted while individual tracks play.
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    v.volume = volume;
    v.muted = trackMode || muted;
  }, [volume, muted, trackMode]);

  const cropping = durMs > 0 && (trim[0] > 0 || trim[1] < durMs - 1);

  const preview = useCallback((secs: number) => {
    const v = video.current;
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(secs, v.duration || secs));
    setTime(v.currentTime);
  }, []);

  /** Seeks; while cropping the playhead stays inside the crop. */
  const seek = useCallback(
    (secs: number) => preview(cropping ? Math.max(trim[0] / 1000, Math.min(secs, trim[1] / 1000)) : secs),
    [preview, cropping, trim],
  );

  const togglePlay = useCallback(() => {
    const v = video.current;
    if (!v) return;
    if (v.paused) {
      // Play inside the crop, from its start once the end is reached.
      if (v.currentTime * 1000 < trim[0] || v.currentTime * 1000 >= trim[1] - 50) v.currentTime = trim[0] / 1000;
      void v.play();
    } else v.pause();
  }, [trim]);

  const setCropAt = (edge: "start" | "end") => {
    const v = video.current;
    if (!v) return;
    v.pause();
    const ms = v.currentTime * 1000;
    if (edge === "start") setTrim(([, e]) => [Math.min(ms, e - MIN_CLIP_MS), e]);
    else setTrim(([st]) => [st, Math.max(ms, st + MIN_CLIP_MS)]);
  };

  // Prefetch the separate tracks so "Audio tracks" opens instantly.
  const loadTracks = useCallback(() => {
    if (!tracksPromise.current) {
      tracksPromise.current = api.prepareAudioTracks(item.id);
      tracksPromise.current.then(setTracks).catch(() => (tracksPromise.current = null));
    }
    return tracksPromise.current;
  }, [item.id]);
  useEffect(() => {
    tracksPromise.current = null;
    if (item.audioTracks.length > 1) void loadTracks().catch(() => {});
  }, [item.id, item.mtimeMs, item.audioTracks.length, loadTracks]);

  // Fullscreen: the player fills the screen (OS window + element).
  useEffect(() => {
    const onChange = () => {
      const fs = document.fullscreenElement === stage.current && !!stage.current;
      setFullscreen(fs);
      if (!document.fullscreenElement) void setWindowFullscreen(false);
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => {
      document.removeEventListener("fullscreenchange", onChange);
      if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
      void setWindowFullscreen(false);
    };
  }, []);
  const toggleFullscreen = async () => {
    if (document.fullscreenElement) {
      await document.exitFullscreen().catch(() => {});
      await setWindowFullscreen(false);
    } else if (stage.current) {
      await setWindowFullscreen(true);
      await stage.current.requestFullscreen().catch(() => {});
    }
  };

  useEffect(() => {
    if (isImage) return;
    const onKey = (e: KeyboardEvent) => {
      const v = video.current;
      if (!v || (e.target as HTMLElement).closest("input, button")) return;
      if (e.key === " ") {
        e.preventDefault();
        togglePlay();
      } else if (e.key === "ArrowRight") seek(v.currentTime + 5);
      else if (e.key === "ArrowLeft") seek(v.currentTime - 5);
      else if (e.key.toLowerCase() === "f") void toggleFullscreen();
      else if (e.key === "[") setCropAt("start");
      else if (e.key === "]") setCropAt("end");
      else if (e.key.toLowerCase() === "b") {
        const next = [...item.bookmarks]
          .sort((a, b) => a.offsetMs - b.offsetMs)
          .find((b) => b.offsetMs / 1000 > v.currentTime + 0.5);
        if (next) seek(next.offsetMs / 1000 - 2);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isImage, item.bookmarks, seek, togglePlay]);

  const openTracks = async () => {
    if (trackMode) return;
    let t = tracks;
    if (!t) {
      setLoadingTracks(true);
      try {
        t = await loadTracks();
      } catch (e) {
        notify(`Couldn't load the audio tracks: ${e}`, true);
        return;
      } finally {
        setLoadingTracks(false);
      }
    }
    if (t.length === 0) {
      notify("This clip has no separate audio tracks", true);
      return;
    }
    setTrackMode(true);
  };

  const gains: TrackGain[] | null = useMemo(() => {
    if (!trackMode || !tracks) return null;
    const g = tracks.map((t) => ({ index: t.index, volume: trackState[t.index]?.volume ?? 1, muted: trackState[t.index]?.muted ?? false }));
    return g.some((x) => x.muted || Math.abs(x.volume - 1) > 0.005) ? g : null;
  }, [trackMode, tracks, trackState]);

  const trimmed = cropping;
  const edited = !isImage && (trimmed || gains !== null);

  /** Stop using the file so Windows lets us replace or delete it. */
  const release = () => {
    const v = video.current;
    if (v) {
      v.pause();
      v.removeAttribute("src");
      v.load();
    }
    setTrackMode(false);
    setReleased(true);
  };

  const save = async (overwrite: boolean) => {
    setSaveMenu(false);
    if (
      overwrite &&
      !(await confirm({
        title: "Overwrite the original?",
        message: "The original file is replaced by the edited version. This can't be undone.",
        confirmLabel: "Overwrite",
        danger: true,
      }))
    )
      return;
    setSaving(true);
    if (overwrite) release();
    try {
      const saved = await api.exportClip(item.id, {
        startMs: Math.round(trim[0]),
        endMs: Math.round(Math.min(trim[1], durMs || trim[1])),
        gains,
        overwrite,
      });
      if (overwrite) {
        setItem(saved);
        setTrim([0, saved.durationMs]);
        setDuration(saved.durationMs / 1000);
        setTracks(null);
        setTrackState({});
        setTime(0);
        notify("Clip updated");
      } else {
        notify(`Saved as ${fileName(saved.path)}`);
      }
    } catch (e) {
      notify(String(e), true);
    } finally {
      setSaving(false);
      setReleased(false);
    }
  };

  /** Sessions: cut the selection out into a new clip in the same folder. */
  const makeClip = async () => {
    setClipping(true);
    try {
      const clip = await api.exportClip(item.id, {
        startMs: Math.round(trim[0]),
        endMs: Math.round(Math.min(trim[1], durMs || trim[1])),
        gains,
        overwrite: false,
        asClip: true,
      });
      notify(`Clip created: ${fileName(clip.path)}`);
    } catch (e) {
      notify(String(e), true);
    } finally {
      setClipping(false);
    }
  };

  const remove = async () => {
    const ok = await confirm({
      title: "Delete this capture?",
      message: `"${fileName(item.path)}" will be removed from disk permanently.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    release();
    try {
      await api.deleteMedia(item.id);
      notify("File deleted");
      onDeleted(item.id);
    } catch (e) {
      setReleased(false);
      notify(String(e), true);
    }
  };

  const title = `${item.gameTitle ?? "Uncategorized"} · ${formatDate(item.createdMs)}`;

  return (
    <Modal title={title} onClose={onClose} wide>
      <div ref={stage} className={cx(fullscreen && "flex h-full flex-col bg-black")}>
      <div className={cx("relative bg-black", fullscreen && "flex min-h-0 flex-1 items-center")}>
        {isImage ? (
          <img src={src} alt={fileName(item.path)} className="mx-auto max-h-[68vh] w-auto object-contain" />
        ) : failed ? (
          <div className="flex aspect-video max-h-[56vh] w-full flex-col items-center justify-center gap-3 text-center">
            <p className="max-w-lg text-sm text-muted">
              The built-in player can't preview this file.
              {item.videoCodec === "hevc" || item.videoCodec === "av1"
                ? ` ${item.videoCodec.toUpperCase()} playback needs the matching video extension from the Microsoft Store.`
                : " It may have been moved, or its container isn't supported by the preview."}
            </p>
            <Button onClick={() => api.openMediaExternal(item.id)}>
              <ExternalLink size={14} /> Open in external player
            </Button>
          </div>
        ) : (
          <video
            ref={video}
            src={src}
            autoPlay
            className={cx(
              "mx-auto aspect-video w-full",
              fullscreen ? "h-full max-h-none" : trackMode ? "max-h-[46vh]" : "max-h-[56vh]",
            )}
            onClick={togglePlay}
            onLoadedMetadata={(e) => {
              const v = e.currentTarget;
              if (Number.isFinite(v.duration)) {
                setDuration(v.duration);
                setTrim(([s, en]) => [s, en >= item.durationMs - 1 || en <= 0 ? v.duration * 1000 : en]);
              }
              if (startMs) v.currentTime = startMs / 1000;
            }}
            onTimeUpdate={(e) => {
              const v = e.currentTarget;
              setTime(v.currentTime);
              // Crop preview ends at the end handle and rewinds to the start.
              if (!v.paused && trimmed && v.currentTime * 1000 >= trim[1]) {
                v.pause();
                v.currentTime = trim[0] / 1000;
              }
            }}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onError={() => !released && setFailed(true)}
          />
        )}
        {saving && (
          <div className="absolute inset-0 flex items-center justify-center gap-2 bg-black/60 text-sm">
            <Loader2 size={16} className="animate-spin" /> Saving…
          </div>
        )}
      </div>

      {!isImage && !failed && (
        <div className={cx("space-y-2 border-t border-line px-4 pb-2", item.kind === "session" && trimmed ? "pt-10" : "pt-2", fullscreen && "bg-panel")}>
          <Timeline
            duration={duration}
            time={time}
            trim={trim}
            setTrim={setTrim}
            bookmarks={item.bookmarks}
            seek={seek}
            preview={preview}
            editable
            onClip={item.kind === "session" ? () => void makeClip() : undefined}
            clipBusy={clipping}
          />
          <div className="flex flex-wrap items-center gap-2">
            <IconButton label={playing ? "Pause (Space)" : "Play (Space)"} onClick={togglePlay}>
              {playing ? <Pause size={16} /> : <Play size={16} />}
            </IconButton>
            <IconButton label={muted ? "Unmute" : "Mute"} onClick={() => setMuted((m) => !m)}>
              {muted || volume === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </IconButton>
            <input
              type="range"
              aria-label="Volume"
              min={0}
              max={100}
              value={Math.round(volume * 100)}
              onChange={(e) => {
                setVolume(Number(e.target.value) / 100);
                setMuted(false);
              }}
              className="h-1.5 w-24 cursor-pointer"
            />
            <span className="font-mono text-xs text-muted tabular-nums">
              {formatDuration(time * 1000)} / {formatDuration(durMs)}
            </span>
            <div className="ml-2 flex items-center gap-1">
              <Button size="sm" variant="ghost" onClick={() => setCropAt("start")} title="Start the crop here ( [ )">
                <ArrowRightToLine size={14} /> Start crop
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setCropAt("end")} title="End the crop here ( ] )">
                <ArrowLeftToLine size={14} /> End crop
              </Button>
            </div>
            {item.bookmarks.length > 0 && (
              <span className="hidden items-center gap-1 text-xs text-faint xl:flex" title="Press B to jump to the next bookmark">
                <Bookmark size={12} className="text-amber-400" /> {item.bookmarks.length}
              </span>
            )}
            <div className="ml-auto flex items-center gap-2">
              {trimmed && (
                <span className="flex items-center gap-1 text-xs text-sky-300">
                  <Scissors size={12} /> {formatDuration(trim[1] - trim[0])} selected
                </span>
              )}
              <IconButton label={fullscreen ? "Exit fullscreen (F)" : "Fullscreen (F)"} onClick={() => void toggleFullscreen()}>
                {fullscreen ? <Minimize size={16} /> : <Maximize size={16} />}
              </IconButton>
              {multiTrack && (
                <Button
                  size="sm"
                  variant={trackMode ? "secondary" : "ghost"}
                  onClick={() => (trackMode ? setTrackMode(false) : void openTracks())}
                  disabled={loadingTracks}
                  title={trackMode ? "Go back to the combined mix" : "Adjust game, mic and voice separately"}
                >
                  {loadingTracks ? <Loader2 size={14} className="animate-spin" /> : <AudioLines size={14} />}
                  {trackMode ? "Use combined mix" : "Audio tracks"}
                </Button>
              )}
              {edited && (
                <Button
                  size="sm"
                  variant="ghost"
                  title="Undo trim and track changes"
                  onClick={() => {
                    setTrim([0, durMs]);
                    setTrackState({});
                  }}
                >
                  <RotateCcw size={13} /> Reset
                </Button>
              )}
              <div className="relative">
                <Button size="sm" variant="primary" disabled={!edited || saving} onClick={() => setSaveMenu((o) => !o)}>
                  <Save size={14} /> Save…
                </Button>
                {saveMenu && (
                  <div
                    className="absolute right-0 bottom-full z-20 mb-2 w-60 rounded-lg border border-line-strong bg-raised p-1 shadow-2xl"
                    onMouseDown={(e) => e.stopPropagation()}
                  >
                    <button
                      onClick={() => void save(false)}
                      className="block w-full rounded-md px-3 py-2 text-left text-sm hover:bg-hover"
                    >
                      Save as a copy
                      <span className="block text-xs text-faint">Keeps the original untouched</span>
                    </button>
                    <button
                      onClick={() => void save(true)}
                      className="block w-full rounded-md px-3 py-2 text-left text-sm hover:bg-hover"
                    >
                      Overwrite original
                      <span className="block text-xs text-faint">Replaces this clip</span>
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
          {trackMode && tracks && tracks.length > 0 && (
            <div className="grid gap-x-6 gap-y-2 rounded-md border border-line bg-raised/50 px-3 py-2.5 sm:grid-cols-2">
              <div className="col-span-full text-xs text-faint">
                Playing the individual tracks (the combined mix is muted). These levels are kept when you save.
              </div>
              {tracks.map((t) => {
                const st = trackState[t.index] ?? { volume: 1, muted: false };
                const set = (p: Partial<typeof st>) => setTrackState((s) => ({ ...s, [t.index]: { ...st, ...p } }));
                return (
                  <div key={t.index} className={cx("flex items-center gap-2", st.muted && "opacity-60")}>
                    <IconButton label={st.muted ? `Unmute ${t.title}` : `Mute ${t.title}`} active={st.muted} onClick={() => set({ muted: !st.muted })}>
                      {st.muted ? <VolumeX size={15} className="text-red-300" /> : <Volume2 size={15} />}
                    </IconButton>
                    <span className="w-24 truncate text-sm">{t.title}</span>
                    <input
                      type="range"
                      aria-label={`${t.title} volume`}
                      min={0}
                      max={100}
                      value={Math.round(st.volume * 100)}
                      onChange={(e) => set({ volume: Number(e.target.value) / 100, muted: false })}
                      className="h-1.5 min-w-0 flex-1 cursor-pointer"
                    />
                    <span className="w-10 text-right font-mono text-xs text-muted tabular-nums">{Math.round(st.volume * 100)}%</span>
                  </div>
                );
              })}
            </div>
          )}
          {trimmed && (
            <p className="text-[11px] text-faint">
              Cropping: playback stays between the handles. Cuts are lossless, so they start on the nearest keyframe (up to
              a second before the start).
            </p>
          )}
        </div>
      )}
      </div>
      <div className="flex items-center gap-2 border-t border-line px-4 py-2.5">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-faint" title={item.path}>
          {item.path}
        </span>
        <Button size="sm" variant="ghost" onClick={() => api.revealMedia(item.id)}>
          <FolderOpen size={14} /> Show in folder
        </Button>
        <Button size="sm" variant="ghost" onClick={() => api.openMediaExternal(item.id)}>
          <ExternalLink size={14} /> Open externally
        </Button>
        <Button size="sm" variant="danger" onClick={() => void remove()} disabled={saving}>
          <Trash2 size={14} /> Delete
        </Button>
      </div>
    </Modal>
  );
}
