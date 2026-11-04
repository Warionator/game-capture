import { useEffect, useState } from "react";
import { useStore } from "../lib/store";
import { cx, formatBytes, formatSeconds } from "../lib/format";
import type { BufferStorage } from "../lib/types";
import { Field, Section, Select, Slider, TextInput, Toggle } from "../components/ui";
import { QualityFields } from "./QualityFields";

const MIN_SECS = 15;
const MAX_SECS = 20 * 60;
// Slider stops: fine-grained for short windows, coarse for long ones.
const STOPS = [15, 30, 45, 60, 90, 120, 180, 240, 300, 420, 600, 900, 1200];

/** Parses "4:30", "1:02:00", "90", "90s", "2m", "2m30s", "1.5m" into seconds. */
export function parseDuration(text: string): number | null {
  const t = text.trim().toLowerCase().replace(/\s+/g, "");
  if (!t) return null;
  if (/^\d+(:\d{1,2}){1,2}$/.test(t)) {
    const parts = t.split(":").map(Number);
    if (parts.slice(1).some((p) => p >= 60)) return null;
    return parts.reduce((acc, p) => acc * 60 + p, 0);
  }
  const m = /^(?:(\d+(?:\.\d+)?)h)?(?:(\d+(?:\.\d+)?)m(?:in)?)?(?:(\d+(?:\.\d+)?)s?)?$/.exec(t);
  if (!m || (!m[1] && !m[2] && !m[3])) return null;
  return Math.round(Number(m[1] ?? 0) * 3600 + Number(m[2] ?? 0) * 60 + Number(m[3] ?? 0));
}

function clock(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function peakKbps(rc: { mode: string; bitrateKbps?: number; maxKbps?: number }): number {
  if (rc.mode === "cbr") return rc.bitrateKbps ?? 0;
  if (rc.mode === "vbr") return rc.maxKbps ?? 0;
  return 60_000;
}

function DurationInput({ value, onChange }: { value: number; onChange: (secs: number) => void }) {
  const [text, setText] = useState(clock(value));
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setText(clock(value)), [value]);
  const commit = () => {
    const secs = parseDuration(text);
    if (secs === null) {
      setError("Use a time like 4:30, 90s or 2m");
      return;
    }
    if (secs < MIN_SECS || secs > MAX_SECS) {
      setError("Between 0:15 and 20:00");
      return;
    }
    setError(null);
    onChange(secs);
    setText(clock(secs));
  };
  return (
    <div className="flex flex-col items-end gap-1">
      <TextInput
        aria-label="Replay length (m:ss)"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setError(null);
        }}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
        className={cx("w-24 text-center font-mono", error && "border-red-500/60")}
      />
      {error && <span className="text-[11px] text-red-300">{error}</span>}
    </div>
  );
}

export function ReplayTab() {
  const { settings, update } = useStore();
  if (!settings) return null;
  const r = settings.replay;
  // Nearest slider stop for arbitrary typed values.
  const idx = STOPS.reduce((best, s, i) => (Math.abs(s - r.durationSecs) < Math.abs(STOPS[best] - r.durationSecs) ? i : best), 0);
  const estimate = (((peakKbps(r.quality.rateControl) + 1000) * 1000) / 8) * r.durationSecs * 1.1;
  const fitsRam = estimate <= r.maxMemoryMb * 1024 * 1024;
  const where = r.storage === "memory" ? "in RAM" : r.storage === "disk" ? "on disk" : fitsRam ? "in RAM" : "on disk";
  const setSecs = (secs: number) => update((x) => void (x.replay.durationSecs = secs));

  return (
    <>
      <Section title="Replay buffer" description="Continuously keeps the last moments so you can save them after they happen.">
        <Field label="Enable replay buffer">
          <Toggle label="Enable replay buffer" checked={r.enabled} onChange={(b) => update((x) => void (x.replay.enabled = b))} />
        </Field>
        <Field label="Start automatically when a game is detected">
          <Toggle label="Auto start" checked={r.autoStart} onChange={(b) => update((x) => void (x.replay.autoStart = b))} />
        </Field>
        <Field
          label="Replay length"
          hint={`${formatSeconds(r.durationSecs)} – up to ${formatBytes(estimate)} at the current bitrate, kept ${where}. Type any length from 0:15 to 20:00.`}
          stacked
        >
          <div className="flex items-start gap-4">
            <Slider
              label="Replay length"
              className="flex-1 pt-2"
              min={0}
              max={STOPS.length - 1}
              value={idx}
              format={(i) => clock(STOPS[i])}
              onChange={(i) => setSecs(STOPS[i])}
            />
            <DurationInput value={r.durationSecs} onChange={setSecs} />
          </div>
        </Field>
        <Field
          label="Buffer storage"
          hint="RAM has zero disk wear. Long or high-bitrate buffers spill to a temporary folder – an NVMe drive is ideal."
        >
          <Select<BufferStorage>
            label="Buffer storage"
            value={r.storage}
            onChange={(v) => update((x) => void (x.replay.storage = v))}
            options={[
              { value: "auto", label: "Automatic" },
              { value: "memory", label: "RAM only" },
              { value: "disk", label: "Disk" },
            ]}
          />
        </Field>
        <Field label="RAM budget" hint="Maximum memory the buffer may use before falling back to disk." stacked>
          <Slider
            label="RAM budget"
            min={256}
            max={8192}
            step={256}
            value={r.maxMemoryMb}
            format={(m) => (m >= 1024 ? `${(m / 1024).toFixed(m % 1024 ? 2 : 0)} GB` : `${m} MB`)}
            onChange={(m) => update((x) => void (x.replay.maxMemoryMb = m))}
          />
        </Field>
      </Section>
      <Section title="Clip quality" description="Used for saved replays. Session recordings have their own quality in the Recording tab.">
        <QualityFields target="replay" />
      </Section>
    </>
  );
}
