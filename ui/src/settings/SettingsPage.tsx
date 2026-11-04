import { AudioLines, Cpu, FolderCog, Gamepad2, History, Info, Keyboard, MonitorCog } from "lucide-react";
import { useState } from "react";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";
import { AboutTab } from "./AboutTab";
import { AudioTab } from "./AudioTab";
import { GamesTab } from "./GamesTab";
import { GeneralTab } from "./GeneralTab";
import { HotkeysTab } from "./HotkeysTab";
import { ReplayTab } from "./ReplayTab";
import { SystemTab } from "./SystemTab";
import { VideoTab } from "./VideoTab";

const TABS = [
  { id: "general", label: "General", icon: FolderCog, body: GeneralTab },
  { id: "video", label: "Video", icon: Cpu, body: VideoTab },
  { id: "replay", label: "Replay buffer", icon: History, body: ReplayTab },
  { id: "audio", label: "Audio", icon: AudioLines, body: AudioTab },
  { id: "hotkeys", label: "Hotkeys", icon: Keyboard, body: HotkeysTab },
  { id: "games", label: "Games", icon: Gamepad2, body: GamesTab },
  { id: "system", label: "System", icon: MonitorCog, body: SystemTab },
  { id: "about", label: "About", icon: Info, body: AboutTab },
] as const;

export function SettingsPage() {
  const { settings, status } = useStore();
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("general");
  const Body = TABS.find((t) => t.id === tab)!.body;
  return (
    <div className="flex min-h-0 flex-1">
      <nav className="w-48 shrink-0 space-y-0.5 border-r border-line p-3">
        <h1 className="px-2 pb-2 text-lg font-semibold">Settings</h1>
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={cx(
              "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm",
              tab === t.id ? "bg-hover text-fg" : "text-muted hover:bg-hover/60 hover:text-fg",
            )}
          >
            <t.icon size={15} className={tab === t.id ? "text-accent" : ""} />
            {t.label}
          </button>
        ))}
      </nav>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl space-y-4 p-6">
          {status?.pendingRestart && (
            <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
              Some changes apply after the current recording stops.
            </div>
          )}
          {settings ? <Body /> : <div className="text-sm text-muted">Loading…</div>}
          <p className="pt-2 text-center text-[11px] text-faint">Changes are saved automatically.</p>
        </div>
      </div>
    </div>
  );
}
