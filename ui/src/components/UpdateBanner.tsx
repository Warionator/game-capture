import { ArrowUpCircle, ExternalLink, RotateCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api, on } from "../lib/api";
import { useStore } from "../lib/store";
import type { UpdateInfo } from "../lib/types";
import { Button, IconButton } from "./ui";

/** Live update state from the backend (initial fetch + `update` events). */
export function useUpdateInfo(): UpdateInfo | null {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  useEffect(() => {
    api.updateStatus().then(setInfo).catch(() => {});
    const sub = on("update", setInfo);
    return () => void sub.then((u) => u());
  }, []);
  return info;
}

/** Asks first when a restart would interrupt a recording, then installs. */
export function useRestartToUpdate() {
  const { status, confirm, notify } = useStore();
  return async (version: string) => {
    if (status?.state === "recording") {
      const ok = await confirm({
        title: "Restart to update?",
        message: `A recording is in progress. It will be saved, then game-capture restarts to install version ${version}.`,
        confirmLabel: "Save and restart",
      });
      if (!ok) return;
    }
    // On success the app exits here; the installer restarts it.
    api.installUpdate().catch((e) => notify(`Update failed: ${e}`, true));
  };
}

/**
 * Slim bar at the top of the window, shown only when an update is ready
 * (installed copies) or available to download (portable copies).
 * Dismissing hides it for that version until the app restarts.
 */
export function UpdateBanner() {
  const info = useUpdateInfo();
  const restart = useRestartToUpdate();
  const { notify } = useStore();
  const [dismissed, setDismissed] = useState<string | null>(null);

  const phase = info?.phase;
  if (!phase || (phase.state !== "ready" && phase.state !== "available")) return null;
  if (dismissed === phase.version) return null;

  const ready = phase.state === "ready";
  return (
    <div
      role="status"
      className="flex h-10 shrink-0 items-center gap-3 border-b border-sky-500/25 bg-sky-500/10 px-4 text-sm"
    >
      <ArrowUpCircle size={16} className="shrink-0 text-accent" />
      <span className="min-w-0 truncate text-fg">
        {ready ? (
          <>
            Version <b>{phase.version}</b> is ready to install.
          </>
        ) : (
          <>
            Version <b>{phase.version}</b> is available.
          </>
        )}
      </span>
      <div className="ml-auto flex items-center gap-1.5">
        <Button
          size="sm"
          variant="ghost"
          onClick={() => api.openReleasePage().catch((e) => notify(String(e), true))}
        >
          {ready ? "What's new" : "Release notes"} <ExternalLink size={12} />
        </Button>
        {ready ? (
          <Button size="sm" variant="primary" onClick={() => restart(phase.version)}>
            <RotateCw size={13} /> Restart to update
          </Button>
        ) : (
          <Button
            size="sm"
            variant="primary"
            onClick={() => api.openReleasePage().catch((e) => notify(String(e), true))}
          >
            Download
          </Button>
        )}
        <IconButton label="Dismiss" onClick={() => setDismissed(phase.version)}>
          <X size={14} />
        </IconButton>
      </div>
    </div>
  );
}
