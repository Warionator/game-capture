import { Bookmark, Camera, Circle, EyeOff, Gamepad2, Monitor, Pause, Play, Save, Square, Zap } from "lucide-react";
import { api } from "../lib/api";
import { cx, formatBytes, formatDuration } from "../lib/format";
import { useStore } from "../lib/store";
import type { Action, EngineStatus } from "../lib/types";
import { Badge, Button, IconButton } from "./ui";

function stateLabel(s: EngineStatus): { text: string; tone: string } {
  switch (s.state) {
    case "recording":
      return { text: "Recording", tone: "text-red-300" };
    case "buffering":
      return { text: s.replayEnabled ? "Replay buffer on" : "Capturing", tone: "text-sky-300" };
    case "starting":
      return { text: "Starting…", tone: "text-amber-300" };
    case "paused":
      return { text: "Paused", tone: "text-muted" };
    case "error":
      return { text: "Capture error", tone: "text-red-300" };
    default:
      return { text: "Waiting for a game", tone: "text-muted" };
  }
}

export function StatusBar() {
  const { status, settings, notify } = useStore();
  const run = (a: Action) => api.action(a).catch((e) => notify(String(e), true));
  if (!status) return <div className="h-14 border-b border-line bg-panel" />;

  const label = stateLabel(status);
  const active = status.state === "buffering" || status.state === "recording";
  const recording = status.state === "recording";
  const buf = status.buffer;
  const fill = buf && status.replayEnabled ? Math.min(1, buf.bufferedSecs / Math.max(1, buf.windowSecs - 2)) : 0;
  const hk = settings?.hotkeys;

  return (
    <div className="flex h-14 shrink-0 items-center gap-4 border-b border-line bg-panel px-4">
      <div className="flex min-w-0 items-center gap-3">
        <span
          className={cx(
            "size-2.5 shrink-0 rounded-full",
            recording ? "rec-dot bg-rec" : active ? "bg-accent" : status.state === "error" ? "bg-rec" : "bg-faint",
          )}
        />
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-sm">
            <span className={cx("font-semibold", label.tone)}>{label.text}</span>
            {recording && (
              <span className="font-mono text-sm text-red-200 tabular-nums">
                {formatDuration(status.recordingSecs * 1000)}
              </span>
            )}
            <span className="flex min-w-0 items-center gap-1.5 text-muted">
              {status.game ? <Gamepad2 size={14} /> : active ? <Monitor size={14} /> : null}
              <span className="truncate">{status.game?.displayName ?? (active ? "Desktop" : "")}</span>
            </span>
            {active && status.focusLost && (
              <span title="You're tabbed out – the recording shows the “Game not in focus” screen">
                <Badge tone="warn">
                  <EyeOff size={10} /> Not in focus
                </Badge>
              </span>
            )}
          </div>
          <div className="mt-0.5 flex items-center gap-2 text-[11px] text-faint">
            {status.encoderLabel ? (
              <>
                <span>{status.encoderLabel}</span>
                {status.captureSource && <span>· {status.captureSource}</span>}
                {status.outputSize && (
                  <span>
                    {status.outputSize[0]}×{status.outputSize[1]} · {status.fps ? `${status.fps} fps` : "uncapped"}
                  </span>
                )}
                {status.zeroCopy && (
                  <Badge tone="accent">
                    <Zap size={10} /> zero-copy
                  </Badge>
                )}
              </>
            ) : status.lastError ? (
              <span className="truncate text-red-300/80" title={status.lastError}>
                {status.lastError}
              </span>
            ) : !status.ffmpegFound ? (
              <span className="text-amber-300/80">FFmpeg not found – reinstall to enable capture</span>
            ) : (
              <span>Capture starts automatically when a game is detected</span>
            )}
          </div>
        </div>
      </div>

      {buf && status.replayEnabled && (
        <div className="hidden w-44 shrink-0 lg:block" title={`${formatBytes(buf.bytes)} buffered ${buf.onDisk ? "on disk" : "in RAM"}`}>
          <div className="flex justify-between text-[11px] text-faint">
            <span>Replay buffer</span>
            <span className="tabular-nums">
              {formatDuration(Math.min(buf.bufferedSecs, buf.windowSecs - 2) * 1000)} / {formatDuration((buf.windowSecs - 2) * 1000)}
            </span>
          </div>
          <div className="mt-1 h-1 overflow-hidden rounded bg-line">
            <div className="h-full bg-accent transition-[width] duration-700" style={{ width: `${fill * 100}%` }} />
          </div>
        </div>
      )}

      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        <Button
          variant="primary"
          size="sm"
          disabled={!active || !status.replayEnabled}
          onClick={() => run("saveReplay")}
          title={`Save replay (${hk?.saveReplay || "no hotkey"})`}
        >
          <Save size={14} /> Save replay
        </Button>
        <Button
          variant={recording ? "record" : "secondary"}
          size="sm"
          onClick={() => run("toggleRecording")}
          title={`${recording ? "Stop" : "Start"} recording (${hk?.toggleRecording || "no hotkey"})`}
        >
          {recording ? <Square size={12} fill="currentColor" /> : <Circle size={12} fill="currentColor" className="text-rec" />}
          {recording ? "Stop" : "Record"}
        </Button>
        <IconButton label={`Add bookmark (${hk?.addBookmark || "no hotkey"})`} disabled={!active} onClick={() => run("bookmark")}>
          <Bookmark size={16} />
        </IconButton>
        <IconButton label={`Screenshot (${hk?.screenshot || "no hotkey"})`} onClick={() => run("screenshot")}>
          <Camera size={16} />
        </IconButton>
        <div className="mx-1 h-6 w-px bg-line" />
        {status.state === "paused" ? (
          <IconButton label="Resume capture" onClick={() => run("resume")}>
            <Play size={16} />
          </IconButton>
        ) : (
          <IconButton label="Pause capture" onClick={() => run("pause")}>
            <Pause size={16} />
          </IconButton>
        )}
        {!active && status.state !== "paused" && status.state !== "starting" && (
          <Button size="sm" variant="ghost" onClick={() => run("startCapture")} title="Start the replay buffer on the desktop">
            <Monitor size={14} /> Capture desktop
          </Button>
        )}
        {active && !status.game && !recording && (
          <Button size="sm" variant="ghost" onClick={() => run("stopCapture")}>
            Stop
          </Button>
        )}
      </div>
    </div>
  );
}
