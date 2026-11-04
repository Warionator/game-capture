import { FolderOpen, Gamepad2, Library, Settings as SettingsIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { cx, formatBytes } from "../lib/format";
import type { GameSummary } from "../lib/types";

export type View = "library" | "settings";

const iconCache = new Map<string, string | null>();

export function GameIcon({ title, size = 20 }: { title: string; size?: number }) {
  const [src, setSrc] = useState<string | null | undefined>(iconCache.get(title));
  useEffect(() => {
    if (iconCache.has(title)) {
      setSrc(iconCache.get(title));
      return;
    }
    let live = true;
    api
      .gameIcon(title)
      .then((s) => {
        iconCache.set(title, s);
        if (live) setSrc(s);
      })
      .catch(() => iconCache.set(title, null));
    return () => {
      live = false;
    };
  }, [title]);
  if (src) return <img src={src} alt="" width={size} height={size} className="shrink-0 rounded" />;
  return (
    <span
      className="flex shrink-0 items-center justify-center rounded bg-hover text-faint"
      style={{ width: size, height: size }}
    >
      <Gamepad2 size={size * 0.6} />
    </span>
  );
}

export function Sidebar({
  view,
  setView,
  games,
  selectedGame,
  setSelectedGame,
}: {
  view: View;
  setView: (v: View) => void;
  games: GameSummary[];
  selectedGame: string | null;
  setSelectedGame: (g: string | null) => void;
}) {
  const total = games.reduce((n, g) => n + g.count, 0);
  const bytes = games.reduce((n, g) => n + g.totalBytes, 0);
  return (
    <aside className="flex w-60 shrink-0 flex-col border-r border-line bg-panel">
      <div className="flex h-14 items-center gap-2.5 border-b border-line px-4">
        <img src="/icon.png" alt="" className="size-6" />
        <span className="text-[15px] font-semibold tracking-tight">game-capture</span>
      </div>
      <nav className="space-y-0.5 p-2">
        <NavItem
          active={view === "library" && selectedGame === null}
          onClick={() => {
            setView("library");
            setSelectedGame(null);
          }}
          icon={<Library size={16} />}
          label="All captures"
          count={total}
        />
      </nav>
      <div className="px-4 pt-3 pb-1.5 text-[11px] font-semibold tracking-wider text-faint uppercase">Games</div>
      <div className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-2">
        {games.length === 0 && <div className="px-2 py-1 text-xs text-faint">No captures yet</div>}
        {games.map((g) => (
          <NavItem
            key={g.title}
            active={view === "library" && selectedGame === g.title}
            onClick={() => {
              setView("library");
              setSelectedGame(g.title);
            }}
            icon={<GameIcon title={g.title} size={18} />}
            label={g.title}
            count={g.count}
          />
        ))}
      </div>
      <div className="border-t border-line p-2">
        <button
          onClick={() => api.openStorageFolder()}
          className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-xs text-muted hover:bg-hover hover:text-fg"
        >
          <FolderOpen size={14} />
          <span className="flex-1">Open library folder</span>
          <span className="text-faint">{formatBytes(bytes)}</span>
        </button>
        <NavItem
          active={view === "settings"}
          onClick={() => setView("settings")}
          icon={<SettingsIcon size={16} />}
          label="Settings"
        />
      </div>
    </aside>
  );
}

function NavItem({
  active,
  onClick,
  icon,
  label,
  count,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  count?: number;
}) {
  return (
    <button
      onClick={onClick}
      className={cx(
        "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm transition-colors",
        active ? "bg-hover text-fg" : "text-muted hover:bg-hover/60 hover:text-fg",
      )}
    >
      <span className={cx("shrink-0", active ? "text-accent" : "")}>{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count !== undefined && <span className="text-xs text-faint tabular-nums">{count}</span>}
    </button>
  );
}
