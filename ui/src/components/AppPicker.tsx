import { AppWindow, FileSearch, Globe, RefreshCw, Search, Volume2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, pickExecutable } from "../lib/api";
import { cx } from "../lib/format";
import type { AudioApp, WebAppRef } from "../lib/types";
import { Badge, Button, Modal, TextInput } from "./ui";

export interface PickedApp {
  name: string;
  exeName: string;
  webApp: WebAppRef | null;
}

/** `spotify.exe` → `Spotify` (matches the backend's track naming). */
export function appLabel(exe: string): string {
  const stem = exe.trim().replace(/\.exe$/i, "").split(/[\\/]/).pop() ?? "";
  return stem ? stem[0].toUpperCase() + stem.slice(1) : "App";
}

function Row({ app, onPick }: { app: AudioApp; onPick: () => void }) {
  return (
    <button
      onClick={onPick}
      className="flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left hover:bg-hover focus-visible:bg-hover"
    >
      {app.icon ? (
        <img src={app.icon} alt="" className="size-7 shrink-0 rounded" />
      ) : (
        <span className="flex size-7 shrink-0 items-center justify-center rounded bg-hover text-faint">
          {app.webApp ? <Globe size={15} /> : <AppWindow size={15} />}
        </span>
      )}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm">{app.name}</span>
        <span className="block truncate text-[11px] text-faint">
          {app.webApp ? `Installed web app · ${app.webApp.browser}` : app.exeName}
          {!app.hasWindow && " · running in the background"}
        </span>
      </span>
      {app.playing && (
        <Badge tone="ok">
          <Volume2 size={10} /> Playing
        </Badge>
      )}
    </button>
  );
}

/** Lists running apps and installed web apps to record into their own track. */
export function AppPicker({
  title,
  onPick,
  onClose,
}: {
  title: string;
  onPick: (app: PickedApp) => void;
  onClose: () => void;
}) {
  const [apps, setApps] = useState<AudioApp[] | null>(null);
  const [query, setQuery] = useState("");
  const load = useCallback(() => {
    setApps(null);
    api
      .listAudioApps()
      .then(setApps)
      .catch(() => setApps([]));
  }, []);
  useEffect(load, [load]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (apps ?? []).filter((a) => !q || a.name.toLowerCase().includes(q) || a.exeName.toLowerCase().includes(q));
  }, [apps, query]);
  const native = filtered.filter((a) => !a.webApp);
  const web = filtered.filter((a) => a.webApp);
  const pick = (a: AudioApp) => {
    onPick({ name: a.name, exeName: a.exeName, webApp: a.webApp });
    onClose();
  };

  return (
    <Modal title={title} onClose={onClose}>
      <div className="flex items-center gap-2 border-b border-line px-4 py-3">
        <div className="relative flex-1">
          <Search size={14} className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-faint" />
          <TextInput
            autoFocus
            compact
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search running apps"
            aria-label="Search running apps"
            className="w-full pl-8"
          />
        </div>
        <Button size="sm" variant="ghost" onClick={load} title="Refresh the list">
          <RefreshCw size={13} />
        </Button>
      </div>
      <div className="max-h-[55vh] overflow-y-auto px-2 py-2">
        {apps === null ? (
          <p className="px-2 py-6 text-center text-sm text-faint">Looking for running apps…</p>
        ) : filtered.length === 0 ? (
          <p className="px-2 py-6 text-center text-sm text-faint">
            {query ? "No running app matches." : "No apps found. Start the app you want to record, then refresh."}
          </p>
        ) : (
          <>
            {native.length > 0 && (
              <>
                <div className="px-2.5 pt-1 pb-1 text-[11px] font-semibold tracking-wider text-faint uppercase">Apps</div>
                {native.map((a) => (
                  <Row key={a.exeName} app={a} onPick={() => pick(a)} />
                ))}
              </>
            )}
            {web.length > 0 && (
              <>
                <div className="px-2.5 pt-3 pb-1 text-[11px] font-semibold tracking-wider text-faint uppercase">
                  Installed web apps
                </div>
                {web.map((a) => (
                  <Row key={a.webApp!.appId} app={a} onPick={() => pick(a)} />
                ))}
                <p className="px-2.5 pt-1 pb-2 text-[11px] leading-relaxed text-faint">
                  Web apps play sound through their browser, so their track also contains anything else that browser
                  plays (other tabs and web apps).
                </p>
              </>
            )}
          </>
        )}
      </div>
      <div className={cx("flex items-center justify-between gap-2 border-t border-line px-4 py-2.5")}>
        <span className="text-xs text-faint">App not running?</span>
        <Button
          size="sm"
          variant="ghost"
          onClick={async () => {
            const path = await pickExecutable();
            if (!path) return;
            const exe = path.split(/[\\/]/).pop() ?? path;
            onPick({ name: appLabel(exe), exeName: exe, webApp: null });
            onClose();
          }}
        >
          <FileSearch size={14} /> Browse for an .exe…
        </Button>
      </div>
    </Modal>
  );
}
