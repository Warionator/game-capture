import { Bookmark, Play } from "lucide-react";
import { formatBytes, formatDate, formatDuration } from "../lib/format";
import type { MediaItem } from "../lib/types";
import { Modal } from "./ui";

export function DetailsDialog({
  item,
  onClose,
  onPlayAt,
}: {
  item: MediaItem;
  onClose: () => void;
  onPlayAt: (ms: number) => void;
}) {
  const rows: [string, string][] = [
    ["Game", item.gameTitle ?? "Uncategorized"],
    ["Executable", item.executable ?? "—"],
    ["Type", item.kind[0].toUpperCase() + item.kind.slice(1)],
    ["Captured", formatDate(item.createdMs)],
    ...(item.kind !== "screenshot" ? ([["Duration", formatDuration(item.durationMs)]] as [string, string][]) : []),
    ["Resolution", item.width && item.height ? `${item.width} × ${item.height}` : "—"],
    ["Codec", item.videoCodec?.toUpperCase() ?? "—"],
    ["Size", formatBytes(item.sizeBytes)],
    ...(item.audioTracks.length
      ? ([["Audio tracks", item.audioTracks.map((t, i) => `${i + 1}. ${t}`).join("   ")]] as [string, string][])
      : []),
  ];
  const marks = [...item.bookmarks].sort((a, b) => a.offsetMs - b.offsetMs);
  return (
    <Modal title="File details" onClose={onClose}>
      <dl className="grid grid-cols-[110px_1fr] gap-x-4 gap-y-2 px-5 py-4 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="break-words text-fg">{v}</dd>
          </div>
        ))}
        <dt className="text-muted">Path</dt>
        <dd className="font-mono text-xs break-all text-fg select-text">{item.path}</dd>
      </dl>
      {item.kind !== "screenshot" && (
        <div className="border-t border-line px-5 py-4">
          <h3 className="mb-2 flex items-center gap-1.5 text-sm font-semibold">
            <Bookmark size={14} className="text-amber-400" /> Bookmarks
          </h3>
          {marks.length === 0 ? (
            <p className="text-sm text-faint">No bookmarks. Press the bookmark hotkey while capturing to mark moments.</p>
          ) : (
            <ul className="space-y-1">
              {marks.map((b, i) => (
                <li key={i}>
                  <button
                    onClick={() => onPlayAt(Math.max(0, b.offsetMs - 2000))}
                    className="flex w-full items-center gap-3 rounded-md px-2 py-1.5 text-left text-sm hover:bg-hover"
                  >
                    <Play size={12} className="text-accent" />
                    <span className="font-mono text-xs text-muted tabular-nums">{formatDuration(b.offsetMs)}</span>
                    <span>{b.label ?? `Bookmark ${i + 1}`}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Modal>
  );
}
