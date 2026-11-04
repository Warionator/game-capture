import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { SystemInfo } from "../lib/types";
import { Badge, Section } from "../components/ui";

export function AboutTab() {
  const [info, setInfo] = useState<SystemInfo | null>(null);
  useEffect(() => {
    api.systemInfo().then(setInfo).catch(() => {});
  }, []);
  if (!info) return null;
  const rows: [string, React.ReactNode][] = [
    ["Version", info.version],
    ["Platform", info.platform],
    ["Windows build", info.windowsBuild ?? "—"],
    [
      "Per-app audio",
      info.processLoopback ? <Badge tone="ok">Supported</Badge> : <Badge tone="warn">Needs Windows 10 2004 (19041) or newer</Badge>,
    ],
    [
      "Windows Game Mode",
      info.gameModeEnabled === null ? "—" : info.gameModeEnabled ? <Badge tone="warn">On</Badge> : <Badge tone="ok">Off</Badge>,
    ],
    ["Start with Windows", info.autostartEnabled ? "Registered" : "Not registered"],
    ["FFmpeg", info.ffmpegPath ?? <Badge tone="rec">Not found</Badge>],
    ["Settings file", info.settingsPath],
    ["Indexed files", info.libraryItems],
  ];
  return (
    <Section title="About game-capture" description="Lightweight clipping and recording – no accounts, no uploads, no overlays.">
      <dl className="grid grid-cols-[150px_1fr] gap-x-4 gap-y-2.5 py-4 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="font-mono text-xs break-all text-fg">{v}</dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}
