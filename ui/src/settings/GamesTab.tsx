import { FileSearch, Gamepad2, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, pickExecutable } from "../lib/api";
import { useStore } from "../lib/store";
import type { WindowedProcess } from "../lib/types";
import { Badge, Button, Field, IconButton, Section, TextInput, Toggle } from "../components/ui";
import { GameIcon } from "../components/Sidebar";

export function GamesTab() {
  const { settings, update, status, notify } = useStore();
  const [procs, setProcs] = useState<WindowedProcess[] | null>(null);
  const [exclude, setExclude] = useState("");

  const refresh = useCallback(() => {
    api.listProcesses().then(setProcs).catch((e) => notify(String(e), true));
  }, [notify]);
  useEffect(refresh, [refresh]);

  if (!settings) return null;
  const g = settings.games;
  const add = (path: string, name?: string) =>
    api
      .addCustomGame(path, name)
      .then((s) => {
        update((x) => void (x.games = s.games));
        notify(`Added ${s.games.custom.at(-1)?.displayName ?? "game"}`);
      })
      .catch((e) => notify(String(e), true));

  return (
    <>
      <Section title="Detection" description="Running processes are checked for an active DirectX/Vulkan renderer, fullscreen windows and launcher install folders.">
        <Field label="Detect games automatically">
          <Toggle label="Auto detect" checked={g.autoDetect} onChange={(b) => update((x) => void (x.games.autoDetect = b))} />
        </Field>
        <Field label="Currently detected">
          {status?.game ? (
            <span className="flex items-center gap-2 text-sm">
              <GameIcon title={status.game.displayName} size={18} />
              {status.game.displayName}
              <span className="font-mono text-xs text-faint">{status.game.exeName}</span>
            </span>
          ) : (
            <span className="text-sm text-faint">No game running</span>
          )}
        </Field>
      </Section>

      <Section
        title="Your games"
        description="Always treated as games, even when not detected automatically (emulators, launchers, windowed titles)."
        actions={
          <Button
            size="sm"
            onClick={async () => {
              const p = await pickExecutable();
              if (p) void add(p);
            }}
          >
            <FileSearch size={14} /> Browse for .exe…
          </Button>
        }
      >
        {g.custom.length === 0 && <p className="py-4 text-sm text-faint">No games added manually.</p>}
        {g.custom.map((c, i) => (
          <div key={c.executable} className="flex items-center gap-3 border-b border-line py-2.5 last:border-b-0">
            <GameIcon title={c.displayName} size={22} />
            <TextInput
              defaultValue={c.displayName}
              aria-label="Display name"
              onBlur={(e) => update((x) => void (x.games.custom[i].displayName = e.target.value))}
              className="h-8 w-56"
            />
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-faint" title={c.path ?? c.executable}>
              {c.path ?? c.executable}
            </span>
            <IconButton label={`Remove ${c.displayName}`} onClick={() => update((x) => void x.games.custom.splice(i, 1))}>
              <Trash2 size={14} />
            </IconButton>
          </div>
        ))}
      </Section>

      <Section
        title="Add a running program"
        description="Pick a window that is open right now."
        actions={
          <Button size="sm" variant="ghost" onClick={refresh}>
            <RefreshCw size={13} /> Refresh
          </Button>
        }
      >
        {procs === null ? (
          <p className="py-4 text-sm text-faint">Loading…</p>
        ) : procs.length === 0 ? (
          <p className="py-4 text-sm text-faint">No windowed programs found.</p>
        ) : (
          procs.map((p) => {
            const known = g.custom.some((c) => c.executable.toLowerCase() === p.exeName.toLowerCase());
            return (
              <div key={p.pid} className="flex items-center gap-3 border-b border-line py-2 last:border-b-0">
                <Gamepad2 size={16} className="shrink-0 text-faint" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm">{p.displayName ?? p.windowTitle}</div>
                  <div className="truncate font-mono text-[11px] text-faint">{p.exeName}</div>
                </div>
                {known ? (
                  <Badge tone="ok">Added</Badge>
                ) : (
                  <Button size="sm" disabled={!p.exePath} onClick={() => p.exePath && add(p.exePath, p.displayName ?? undefined)}>
                    <Plus size={13} /> Add
                  </Button>
                )}
              </div>
            );
          })
        )}
      </Section>

      <Section title="Never treat as games" description="Executables to ignore even if they look like games.">
        <div className="flex flex-wrap gap-1.5 py-3">
          {g.excluded.length === 0 && <span className="text-sm text-faint">None</span>}
          {g.excluded.map((e, i) => (
            <span key={e} className="inline-flex items-center gap-1 rounded bg-hover py-0.5 pr-1 pl-2 font-mono text-xs">
              {e}
              <button aria-label={`Remove ${e}`} onClick={() => update((x) => void x.games.excluded.splice(i, 1))} className="text-faint hover:text-fg">
                ×
              </button>
            </span>
          ))}
        </div>
        <form
          className="flex gap-2 pb-3.5"
          onSubmit={(e) => {
            e.preventDefault();
            const v = exclude.trim();
            if (v) update((x) => void x.games.excluded.push(v));
            setExclude("");
          }}
        >
          <TextInput value={exclude} onChange={(e) => setExclude(e.target.value)} placeholder="launcher.exe" className="w-56 font-mono text-xs" />
          <Button size="md" type="submit">
            Add
          </Button>
        </form>
      </Section>
    </>
  );
}
