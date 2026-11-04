import { Bookmark, Clapperboard, Film, Image as ImageIcon } from "lucide-react";
import { useState } from "react";
import { fileUrl } from "../lib/api";
import { cx, formatDuration } from "../lib/format";
import type { MediaItem } from "../lib/types";

export function KindIcon({ kind, size = 12 }: { kind: MediaItem["kind"]; size?: number }) {
  if (kind === "screenshot") return <ImageIcon size={size} />;
  if (kind === "session") return <Clapperboard size={size} />;
  return <Film size={size} />;
}

export function MediaCard({
  item,
  selected,
  onOpen,
  onSelect,
  onContextMenu,
  showGame,
}: {
  item: MediaItem;
  selected: boolean;
  onOpen: () => void;
  onSelect: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
  showGame: boolean;
}) {
  const [broken, setBroken] = useState(false);
  const thumb = fileUrl(item.thumbnail);
  const time = new Date(item.createdMs).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const date = new Date(item.createdMs).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return (
    <button
      onClick={onSelect}
      onDoubleClick={onOpen}
      onKeyDown={(e) => e.key === "Enter" && onOpen()}
      onContextMenu={(e) => {
        e.preventDefault();
        onSelect();
        onContextMenu(e);
      }}
      className={cx(
        "group flex flex-col overflow-hidden rounded-lg border bg-panel text-left transition-colors",
        selected ? "border-accent ring-1 ring-accent" : "border-line hover:border-line-strong",
      )}
    >
      <div className="relative aspect-video w-full overflow-hidden bg-raised">
        {thumb && !broken ? (
          <img
            src={thumb}
            alt=""
            loading="lazy"
            draggable={false}
            onError={() => setBroken(true)}
            className="size-full object-cover transition-transform duration-300 group-hover:scale-[1.03]"
          />
        ) : (
          <div className="flex size-full items-center justify-center text-faint">
            <KindIcon kind={item.kind} size={28} />
          </div>
        )}
        <div className="absolute top-1.5 left-1.5 flex items-center gap-1 rounded bg-black/65 px-1.5 py-0.5 text-[11px] text-white/90">
          <KindIcon kind={item.kind} size={11} />
          {item.kind === "session" ? "Session" : item.kind === "screenshot" ? "Screenshot" : "Clip"}
        </div>
        {item.kind !== "screenshot" && (
          <div className="absolute right-1.5 bottom-1.5 rounded bg-black/70 px-1.5 py-0.5 font-mono text-[11px] text-white tabular-nums">
            {formatDuration(item.durationMs)}
          </div>
        )}
        {item.bookmarks.length > 0 && (
          <div className="absolute bottom-1.5 left-1.5 flex items-center gap-0.5 rounded bg-black/65 px-1.5 py-0.5 text-[11px] text-amber-300">
            <Bookmark size={10} fill="currentColor" />
            {item.bookmarks.length}
          </div>
        )}
      </div>
      <div className="flex items-center justify-between gap-2 px-2.5 py-2">
        <span className="truncate text-[13px] text-fg">{showGame ? (item.gameTitle ?? "Uncategorized") : time}</span>
        <span className="shrink-0 text-[11px] text-faint">{showGame ? `${date}, ${time}` : date}</span>
      </div>
    </button>
  );
}
