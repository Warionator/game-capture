import {
  Clipboard,
  ExternalLink,
  FolderOpen,
  Info,
  Library as LibraryIcon,
  Play,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, on } from "../lib/api";
import { cx, fileName, formatBytes, formatDay } from "../lib/format";
import { useStore } from "../lib/store";
import type { MediaFilter, MediaItem, MediaKind, SortOrder } from "../lib/types";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { DetailsDialog } from "./DetailsDialog";
import { MediaCard } from "./MediaCard";
import { Player } from "./Player";
import { GameIcon } from "./Sidebar";
import { Button, Empty, Select, TextInput } from "./ui";

type DateRange = "any" | "today" | "week" | "month";
type DurationRange = "any" | "short" | "medium" | "long";
type GroupBy = "none" | "game" | "date";

const DAY = 86_400_000;

function buildFilter(
  game: string | null,
  kind: MediaKind | null,
  search: string,
  date: DateRange,
  dur: DurationRange,
  sort: SortOrder,
): MediaFilter {
  const f: MediaFilter = { game, kind, search: search.trim() || null, sort };
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  if (date === "today") f.fromMs = start.getTime();
  if (date === "week") f.fromMs = Date.now() - 7 * DAY;
  if (date === "month") f.fromMs = Date.now() - 30 * DAY;
  if (dur === "short") f.maxDurationMs = 60_000;
  if (dur === "medium") {
    f.minDurationMs = 60_000;
    f.maxDurationMs = 600_000;
  }
  if (dur === "long") f.minDurationMs = 600_000;
  return f;
}

export function Gallery({ game, onLibraryChanged }: { game: string | null; onLibraryChanged: () => void }) {
  const { notify, confirm } = useStore();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [kind, setKind] = useState<MediaKind | null>(null);
  const [search, setSearch] = useState("");
  const [date, setDate] = useState<DateRange>("any");
  const [dur, setDur] = useState<DurationRange>("any");
  const [sort, setSort] = useState<SortOrder>("newest");
  const [groupBy, setGroupBy] = useState<GroupBy>(game ? "date" : "game");
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const toggle = (id: number) =>
    setChecked((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const [menu, setMenu] = useState<{ x: number; y: number; item: MediaItem } | null>(null);
  const [playing, setPlaying] = useState<{ item: MediaItem; startMs?: number } | null>(null);
  const [details, setDetails] = useState<MediaItem | null>(null);
  const [scanning, setScanning] = useState(false);

  useEffect(() => setGroupBy(game ? "date" : "game"), [game]);

  const filter = useMemo(() => buildFilter(game, kind, search, date, dur, sort), [game, kind, search, date, dur, sort]);

  const reload = useCallback(() => {
    api
      .listMedia(filter)
      .then((r) => {
        setItems(r);
        setLoading(false);
      })
      .catch((e) => notify(String(e), true));
  }, [filter, notify]);

  useEffect(() => {
    const t = window.setTimeout(reload, search ? 150 : 0);
    return () => window.clearTimeout(t);
  }, [reload, search]);

  useEffect(() => {
    const subs = [
      on("media-added", () => {
        reload();
        onLibraryChanged();
      }),
      on("library-changed", () => {
        reload();
        onLibraryChanged();
      }),
      on("scan-progress", setScanning),
    ];
    return () => subs.forEach((p) => p.then((u) => u()));
  }, [reload, onLibraryChanged]);

  const remove = useCallback(
    async (item: MediaItem) => {
      const ok = await confirm({
        title: "Delete this capture?",
        message: `"${fileName(item.path)}" will be removed from disk permanently.`,
        confirmLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
      try {
        await api.deleteMedia(item.id);
        setItems((xs) => xs.filter((x) => x.id !== item.id));
        onLibraryChanged();
        notify("File deleted");
      } catch (e) {
        notify(String(e), true);
      }
    },
    [notify, confirm, onLibraryChanged],
  );

  // Ticked items that are still listed (filters may hide some).
  const checkedItems = useMemo(() => items.filter((i) => checked.has(i.id)), [items, checked]);

  const removeChecked = useCallback(async () => {
    const list = checkedItems;
    if (list.length === 0) return;
    if (list.length === 1) {
      await remove(list[0]);
      setChecked(new Set());
      return;
    }
    const bytes = list.reduce((n, i) => n + i.sizeBytes, 0);
    const ok = await confirm({
      title: `Delete ${list.length} captures?`,
      message: `${list.length} files (${formatBytes(bytes)}) will be removed from disk permanently.`,
      confirmLabel: `Delete ${list.length}`,
      danger: true,
    });
    if (!ok) return;
    const failed: string[] = [];
    const gone = new Set<number>();
    for (const it of list) {
      try {
        await api.deleteMedia(it.id);
        gone.add(it.id);
      } catch (e) {
        failed.push(`${fileName(it.path)}: ${e}`);
      }
    }
    setItems((xs) => xs.filter((x) => !gone.has(x.id)));
    setChecked(new Set());
    onLibraryChanged();
    if (failed.length) notify(`${failed.length} file(s) could not be deleted: ${failed[0]}`, true);
    else notify(`Deleted ${list.length} files`);
  }, [checkedItems, confirm, notify, onLibraryChanged, remove]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (playing || details || (e.target as HTMLElement).closest("input, select, textarea, [role=dialog], [role=alertdialog]")) return;
      if (e.key === "Delete" && checkedItems.length) void removeChecked();
      if (e.key === "Escape") setChecked(new Set());
      if (e.key.toLowerCase() === "a" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        setChecked(new Set(items.map((i) => i.id)));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [items, checkedItems, removeChecked, playing, details]);

  const groups = useMemo(() => {
    if (groupBy === "none") return [{ key: "all", label: "", items }];
    const map = new Map<string, MediaItem[]>();
    for (const it of items) {
      const key = groupBy === "game" ? (it.gameTitle ?? "Uncategorized") : new Date(it.createdMs).toDateString();
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(it);
    }
    return [...map.entries()].map(([key, list]) => ({
      key,
      label: groupBy === "game" ? key : formatDay(list[0].createdMs),
      items: list,
    }));
  }, [items, groupBy]);

  const menuItems = (item: MediaItem): MenuItem[] => {
    const guard = (p: Promise<unknown>, ok?: string) =>
      p.then(() => ok && notify(ok)).catch((e) => notify(String(e), true));
    return [
      { label: item.kind === "screenshot" ? "View" : "Play", icon: <Play size={14} />, onSelect: () => setPlaying({ item }) },
      { label: "Open file location", icon: <FolderOpen size={14} />, onSelect: () => guard(api.revealMedia(item.id)) },
      { label: "Open in default player", icon: <ExternalLink size={14} />, onSelect: () => guard(api.openMediaExternal(item.id)) },
      {
        label: "Copy file to clipboard",
        icon: <Clipboard size={14} />,
        onSelect: () => guard(api.copyMedia(item.id), "Copied – paste it into Explorer, Discord…"),
      },
      { label: "Details & bookmarks", icon: <Info size={14} />, onSelect: () => setDetails(item) },
      { label: "Delete file", icon: <Trash2 size={14} />, danger: true, separatorBefore: true, shortcut: "Del", onSelect: () => remove(item) },
    ];
  };

  const totalBytes = items.reduce((n, i) => n + i.sizeBytes, 0);
  const kinds: { value: MediaKind | null; label: string }[] = [
    { value: null, label: "All" },
    { value: "clip", label: "Clips" },
    { value: "session", label: "Sessions" },
    { value: "screenshot", label: "Screenshots" },
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-2 border-b border-line px-5 py-2.5">
        <div className="mr-2 flex min-w-0 items-center gap-2.5">
          {game && <GameIcon title={game} size={24} />}
          <h1 className="truncate text-lg font-semibold">{game ?? "All captures"}</h1>
          <span className="shrink-0 text-xs text-faint">
            {items.length} item{items.length === 1 ? "" : "s"} · {formatBytes(totalBytes)}
          </span>
        </div>
        <div className="flex rounded-md border border-line-strong bg-raised p-0.5">
          {kinds.map((k) => (
            <button
              key={k.label}
              onClick={() => setKind(k.value)}
              className={cx(
                "rounded px-2.5 py-1 text-xs font-medium",
                kind === k.value ? "bg-hover text-fg" : "text-muted hover:text-fg",
              )}
            >
              {k.label}
            </button>
          ))}
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <div className="relative">
            <Search size={14} className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-faint" />
            <TextInput
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search"
              aria-label="Search captures"
              compact
              className="w-44 pl-8"
            />
          </div>
          <Select<DateRange>
            compact
            label="Date"
            value={date}
            onChange={setDate}
            options={[
              { value: "any", label: "Any date" },
              { value: "today", label: "Today" },
              { value: "week", label: "Last 7 days" },
              { value: "month", label: "Last 30 days" },
            ]}
          />
          <Select<DurationRange>
            compact
            label="Duration"
            value={dur}
            onChange={setDur}
            options={[
              { value: "any", label: "Any length" },
              { value: "short", label: "Under 1 min" },
              { value: "medium", label: "1 – 10 min" },
              { value: "long", label: "Over 10 min" },
            ]}
          />
          <Select<SortOrder>
            compact
            label="Sort"
            value={sort}
            onChange={setSort}
            options={[
              { value: "newest", label: "Newest first" },
              { value: "oldest", label: "Oldest first" },
              { value: "longest", label: "Longest" },
              { value: "shortest", label: "Shortest" },
              { value: "largest", label: "Largest files" },
              { value: "game", label: "Game A–Z" },
            ]}
          />
          <Select<GroupBy>
            compact
            label="Group by"
            value={groupBy}
            onChange={setGroupBy}
            options={[
              { value: "game", label: "Group: game" },
              { value: "date", label: "Group: date" },
              { value: "none", label: "No grouping" },
            ]}
          />
          <Button
            variant="ghost"
            size="sm"
            title="Rescan the library folder"
            disabled={scanning}
            onClick={() =>
              api
                .rescanLibrary()
                .then((r) => r && notify(`Library scanned: ${r.added} added, ${r.removed} removed`))
                .catch((e) => notify(String(e), true))
            }
          >
            <RefreshCw size={14} className={scanning ? "animate-spin" : ""} />
          </Button>
        </div>
      </div>

      {checkedItems.length > 0 && (
        <div className="flex items-center gap-3 border-b border-line bg-sky-500/10 px-5 py-2">
          <span className="text-sm font-medium">
            {checkedItems.length} selected
            <span className="ml-2 text-xs font-normal text-muted">
              {formatBytes(checkedItems.reduce((n, i) => n + i.sizeBytes, 0))}
            </span>
          </span>
          <Button size="sm" variant="ghost" onClick={() => setChecked(new Set(items.map((i) => i.id)))}>
            Select all
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setChecked(new Set())}>
            Clear
          </Button>
          <Button
            size="sm"
            variant="danger"
            className="ml-auto"
            onClick={() => void removeChecked()}
            title="Delete the selected files (Del)"
            aria-label={`Delete ${checkedItems.length} selected`}
          >
            <Trash2 size={14} /> Delete
          </Button>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {!loading && items.length === 0 ? (
          <Empty icon={<LibraryIcon size={40} />} title={search || kind || date !== "any" || dur !== "any" ? "Nothing matches these filters" : "No captures yet"}>
            {search || kind || date !== "any" || dur !== "any"
              ? "Try clearing the search or widening the date and duration filters."
              : "Start a game – the replay buffer turns on automatically. Press your Save Replay hotkey to keep the last moments."}
          </Empty>
        ) : (
          groups.map((g) => (
            <section key={g.key} className="mb-6">
              {g.label && (
                <h2 className="mb-2.5 flex items-center gap-2 text-sm font-semibold text-muted">
                  {groupBy === "game" && <GameIcon title={g.label} size={16} />}
                  {g.label}
                  <span className="font-normal text-faint">{g.items.length}</span>
                </h2>
              )}
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                {g.items.map((it) => (
                  <div key={it.id}>
                    <MediaCard
                      item={it}
                      checked={checked.has(it.id)}
                      selecting={checkedItems.length > 0}
                      showGame={groupBy !== "game" && !game}
                      onToggle={() => toggle(it.id)}
                      onOpen={() => setPlaying({ item: it })}
                      onContextMenu={(e) => setMenu({ x: e.clientX, y: e.clientY, item: it })}
                    />
                  </div>
                ))}
              </div>
            </section>
          ))
        )}
      </div>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.item)} onClose={() => setMenu(null)} />}
      {playing && (
        <Player
          key={playing.item.id}
          item={playing.item}
          startMs={playing.startMs}
          onClose={() => {
            setPlaying(null);
            reload();
          }}
          onDeleted={(id) => {
            setPlaying(null);
            setItems((xs) => xs.filter((x) => x.id !== id));
            onLibraryChanged();
          }}
        />
      )}
      {details && (
        <DetailsDialog
          item={details}
          onClose={() => setDetails(null)}
          onPlayAt={(ms) => {
            setDetails(null);
            setPlaying({ item: details, startMs: ms });
          }}
        />
      )}
    </div>
  );
}
