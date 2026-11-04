import { Bookmark, ExternalLink, FolderOpen, Pause, Play, Volume2, VolumeX } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, fileUrl } from "../lib/api";
import { fileName, formatDate, formatDuration } from "../lib/format";
import type { MediaItem } from "../lib/types";
import { Button, IconButton, Modal } from "./ui";

/** Built-in preview: hardware-decoded <video> (WebView2) with a timeline
 *  that shows bookmark markers. */
export function Player({ item, startMs, onClose }: { item: MediaItem; startMs?: number; onClose: () => void }) {
  const video = useRef<HTMLVideoElement>(null);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(item.durationMs / 1000);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [failed, setFailed] = useState(false);
  const src = fileUrl(item.path);
  const isImage = item.kind === "screenshot";

  const seek = useCallback((secs: number) => {
    const v = video.current;
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(secs, v.duration || secs));
    setTime(v.currentTime);
  }, []);

  useEffect(() => {
    if (isImage) return;
    const onKey = (e: KeyboardEvent) => {
      const v = video.current;
      if (!v || (e.target as HTMLElement).closest("input")) return;
      if (e.key === " ") {
        e.preventDefault();
        if (v.paused) void v.play();
        else v.pause();
      } else if (e.key === "ArrowRight") seek(v.currentTime + 5);
      else if (e.key === "ArrowLeft") seek(v.currentTime - 5);
      else if (e.key.toLowerCase() === "b") {
        const next = [...item.bookmarks].sort((a, b) => a.offsetMs - b.offsetMs).find((b) => b.offsetMs / 1000 > v.currentTime + 0.5);
        if (next) seek(next.offsetMs / 1000 - 2);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isImage, item.bookmarks, seek]);

  const pct = duration > 0 ? (time / duration) * 100 : 0;
  const title = `${item.gameTitle ?? "Uncategorized"} · ${formatDate(item.createdMs)}`;

  return (
    <Modal title={title} onClose={onClose} wide>
      <div className="bg-black">
        {isImage ? (
          <img src={src} alt={fileName(item.path)} className="mx-auto max-h-[70vh] w-auto object-contain" />
        ) : failed ? (
          <div className="flex aspect-video max-h-[70vh] w-full flex-col items-center justify-center gap-3 text-center">
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
            className="mx-auto max-h-[70vh] w-full"
            onClick={() => {
              const v = video.current;
              if (v) v.paused ? void v.play() : v.pause();
            }}
            onLoadedMetadata={(e) => {
              const v = e.currentTarget;
              if (Number.isFinite(v.duration)) setDuration(v.duration);
              if (startMs) v.currentTime = startMs / 1000;
            }}
            onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onError={() => setFailed(true)}
          />
        )}
      </div>

      {!isImage && !failed && (
        <div className="space-y-2 border-t border-line px-4 pt-3 pb-2">
          <div
            className="group relative h-6 cursor-pointer"
            onMouseDown={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              seek(((e.clientX - r.left) / r.width) * duration);
            }}
          >
            <div className="absolute top-1/2 right-0 left-0 h-1.5 -translate-y-1/2 rounded bg-line" />
            <div className="absolute top-1/2 left-0 h-1.5 -translate-y-1/2 rounded bg-accent" style={{ width: `${pct}%` }} />
            <div
              className="absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white shadow"
              style={{ left: `${pct}%` }}
            />
            {item.bookmarks.map((b, i) => (
              <button
                key={i}
                title={`${b.label ?? `Bookmark ${i + 1}`} · ${formatDuration(b.offsetMs)}`}
                onMouseDown={(e) => {
                  e.stopPropagation();
                  seek(b.offsetMs / 1000);
                }}
                className="absolute top-0 h-6 w-2 -translate-x-1/2"
                style={{ left: `${duration ? (b.offsetMs / 1000 / duration) * 100 : 0}%` }}
              >
                <span className="mx-auto block h-full w-0.5 rounded bg-amber-400" />
              </button>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <IconButton
              label={playing ? "Pause (Space)" : "Play (Space)"}
              onClick={() => {
                const v = video.current;
                if (v) v.paused ? void v.play() : v.pause();
              }}
            >
              {playing ? <Pause size={16} /> : <Play size={16} />}
            </IconButton>
            <IconButton
              label={muted ? "Unmute" : "Mute"}
              onClick={() => {
                const v = video.current;
                if (v) {
                  v.muted = !v.muted;
                  setMuted(v.muted);
                }
              }}
            >
              {muted ? <VolumeX size={16} /> : <Volume2 size={16} />}
            </IconButton>
            <span className="font-mono text-xs text-muted tabular-nums">
              {formatDuration(time * 1000)} / {formatDuration(duration * 1000)}
            </span>
            {item.bookmarks.length > 0 && (
              <span className="ml-2 flex items-center gap-1 text-xs text-faint">
                <Bookmark size={12} className="text-amber-400" /> {item.bookmarks.length} bookmark
                {item.bookmarks.length > 1 ? "s" : ""} · press B to jump
              </span>
            )}
          </div>
        </div>
      )}
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
      </div>
    </Modal>
  );
}
