import { useStore } from "../lib/store";
import { formatBytes, formatSeconds } from "../lib/format";
import type { BufferStorage } from "../lib/types";
import { Field, Section, Select, Slider, Toggle } from "../components/ui";

// Slider stops: fine-grained for short windows, coarse for long ones.
const STOPS = [15, 30, 45, 60, 90, 120, 180, 240, 300, 420, 600, 900, 1200];

function peakKbps(rc: { mode: string; bitrateKbps?: number; maxKbps?: number }): number {
  if (rc.mode === "cbr") return rc.bitrateKbps ?? 0;
  if (rc.mode === "vbr") return rc.maxKbps ?? 0;
  return 60_000;
}

export function ReplayTab() {
  const { settings, update } = useStore();
  if (!settings) return null;
  const r = settings.replay;
  const idx = Math.max(0, STOPS.findIndex((s) => s >= r.durationSecs));
  const estimate = ((peakKbps(settings.video.rateControl) + 1000) * 1000) / 8 * r.durationSecs * 1.1;
  const fitsRam = estimate <= r.maxMemoryMb * 1024 * 1024;
  const where = r.storage === "memory" ? "RAM" : r.storage === "disk" ? "disk" : fitsRam ? "RAM" : "disk";

  return (
    <Section title="Replay buffer" description="Continuously keeps the last moments so you can save them after they happen.">
      <Field label="Enable replay buffer">
        <Toggle label="Enable replay buffer" checked={r.enabled} onChange={(b) => update((x) => void (x.replay.enabled = b))} />
      </Field>
      <Field label="Start automatically when a game is detected">
        <Toggle label="Auto start" checked={r.autoStart} onChange={(b) => update((x) => void (x.replay.autoStart = b))} />
      </Field>
      <Field label="Replay length" hint={`Up to ${formatBytes(estimate)} at the current bitrate, kept in ${where}.`} stacked>
        <Slider
          label="Replay length"
          min={0}
          max={STOPS.length - 1}
          value={idx}
          format={(i) => formatSeconds(STOPS[i])}
          onChange={(i) => update((x) => void (x.replay.durationSecs = STOPS[i]))}
        />
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
  );
}
