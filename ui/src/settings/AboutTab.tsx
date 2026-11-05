import { useEffect, useState } from "react";
import { RotateCw } from "lucide-react";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import type { SystemInfo, UpdateInfo } from "../lib/types";
import { Badge, Button, Field, Section, Toggle } from "../components/ui";
import { useRestartToUpdate, useUpdateInfo } from "../components/UpdateBanner";

function lastChecked(ms: number | null): string {
  if (!ms) return "";
  return ` · checked ${new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

function UpdateStatusLine({ info }: { info: UpdateInfo }) {
  const p = info.phase;
  switch (p.state) {
    case "checking":
      return <span className="text-muted">Checking for updates…</span>;
    case "downloading":
      return (
        <span className="text-muted">
          Downloading version {p.version}
          {p.percent !== null ? ` · ${p.percent}%` : "…"}
        </span>
      );
    case "ready":
      return <Badge tone="accent">Version {p.version} is ready to install</Badge>;
    case "available":
      return <Badge tone="accent">Version {p.version} is available</Badge>;
    case "upToDate":
      return (
        <span className="text-muted">
          You're on the latest version{lastChecked(info.lastCheckedMs)}
        </span>
      );
    case "error":
      return <Badge tone="warn">Couldn't check for updates: {p.message}</Badge>;
    default:
      return <span className="text-muted">Not checked yet</span>;
  }
}

function UpdatesSection() {
  const { settings, update, notify } = useStore();
  const info = useUpdateInfo();
  const restart = useRestartToUpdate();
  if (!settings || !info) return null;
  const p = info.phase;
  const busy = p.state === "checking" || p.state === "downloading";
  const portable = info.kind === "portable";
  return (
    <Section
      title="Updates"
      description={
        portable
          ? "Portable copy: new versions are announced here and at the top of the window; download them from the release page."
          : "New versions download in the background. Nothing changes until you choose Restart to update."
      }
      actions={
        <Button size="sm" disabled={busy} onClick={() => api.checkForUpdates().catch((e) => notify(String(e), true))}>
          <RotateCw size={13} className={busy ? "animate-spin" : undefined} /> Check now
        </Button>
      }
    >
      <Field label={`Version ${info.currentVersion}`}>
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <UpdateStatusLine info={info} />
          {p.state === "ready" && (
            <Button size="sm" variant="primary" onClick={() => restart(p.version)}>
              Restart to update
            </Button>
          )}
          {p.state === "available" && (
            <Button size="sm" variant="primary" onClick={() => api.openReleasePage().catch((e) => notify(String(e), true))}>
              Download
            </Button>
          )}
        </div>
      </Field>
      <Field label="Check automatically" hint="On startup and every 6 hours.">
        <Toggle
          label="Check for updates automatically"
          checked={settings.system.checkForUpdates}
          onChange={(b) => update((x) => void (x.system.checkForUpdates = b))}
        />
      </Field>
    </Section>
  );
}

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
    <>
      <UpdatesSection />
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
    </>
  );
}
