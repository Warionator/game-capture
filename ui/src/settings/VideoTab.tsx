import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import type { CaptureMode, Codec, EncoderInfo, EncoderPreference, MonitorInfo } from "../lib/types";
import { QualityFields } from "./QualityFields";
import { Badge, Button, Field, Section, Select, Toggle } from "../components/ui";

export function VideoTab() {
  const { settings, update, status } = useStore();
  const [encoders, setEncoders] = useState<EncoderInfo[]>([]);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);

  useEffect(() => {
    api.listEncoders().then(setEncoders).catch(() => {});
    api.listMonitors().then(setMonitors).catch(() => {});
  }, [status?.encodersProbed]);

  if (!settings) return null;
  const v = settings.video;
  const works = (b: EncoderPreference, c: Codec) =>
    b === "auto" ? encoders.some((e) => e.codec === c && e.working) : encoders.some((e) => e.backend === b && e.codec === c && e.working);
  const probed = encoders.length > 0;

  const encoderOpts: { value: EncoderPreference; label: string }[] = [
    { value: "auto", label: "Automatic (best available)" },
    { value: "nvenc", label: "NVIDIA NVENC" },
    { value: "amf", label: "AMD AMF" },
    { value: "qsv", label: "Intel QuickSync" },
    { value: "software", label: "Software (x264 / x265 / SVT-AV1)" },
  ];

  return (
    <>
      <Section
        title="Encoder"
        description="Frames go from the GPU framebuffer straight into the hardware encoder; no game hooks are injected."
        actions={
          <Button size="sm" variant="ghost" onClick={() => api.reprobeEncoders()} title="Test encoders again">
            <RefreshCw size={13} /> Re-test
          </Button>
        }
      >
        <Field label="Hardware encoder" hint={status?.encoderLabel ? `In use now: ${status.encoderLabel}` : undefined}>
          <Select
            label="Encoder"
            value={v.encoder}
            onChange={(val) => update((x) => void (x.video.encoder = val))}
            options={encoderOpts.map((o) => ({
              ...o,
              label: probed && o.value !== "auto" && !works(o.value, v.codec) ? `${o.label} – unavailable` : o.label,
            }))}
          />
        </Field>
        <Field label="Codec" hint="H.264 plays everywhere. HEVC and AV1 give smaller files at the same quality.">
          <Select<Codec>
            label="Codec"
            value={v.codec}
            onChange={(val) => update((x) => void (x.video.codec = val))}
            options={[
              { value: "h264", label: "H.264 / AVC" },
              { value: "hevc", label: `HEVC / H.265${probed && !works(v.encoder, "hevc") ? " – unavailable" : ""}` },
              { value: "av1", label: `AV1${probed && !works(v.encoder, "av1") ? " – unavailable" : ""}` },
            ]}
          />
        </Field>
        {probed && (
          <div className="flex flex-wrap gap-1.5 border-b border-line py-3">
            {encoders
              .filter((e) => e.working)
              .map((e) => (
                <Badge key={e.name} tone={e.backend === "software" ? "default" : "ok"}>
                  {e.label}
                </Badge>
              ))}
          </div>
        )}
        <Field
          label="Zero-copy GPU path"
          hint="Keep frames in video memory from capture to encoder. Falls back automatically if the driver rejects it."
        >
          <Toggle label="Zero-copy" checked={v.zeroCopy} onChange={(b) => update((x) => void (x.video.zeroCopy = b))} />
        </Field>
      </Section>

      <Section title="Quality">
        <QualityFields />
        <Field label="Container" hint="MKV survives crashes mid-write; MP4 is the most compatible for sharing.">
          <Select
            label="Container"
            value={v.container}
            onChange={(val) => update((x) => void (x.video.container = val))}
            options={[
              { value: "mp4", label: "MP4" },
              { value: "mkv", label: "MKV (Matroska)" },
            ]}
          />
        </Field>
      </Section>

      <Section title="Capture source">
        <Field
          label="Capture games by"
          hint="Game window records only the game itself, so other apps and notifications never appear. Monitor records the whole screen the game is on."
        >
          <Select<CaptureMode>
            label="Capture method"
            value={v.captureMode}
            onChange={(val) => update((x) => void (x.video.captureMode = val))}
            options={[
              { value: "window", label: "Game window (recommended)" },
              { value: "monitor", label: "Entire monitor" },
            ]}
          />
        </Field>
        <Field
          label="Show “Game not in focus” screen"
          hint="While you're tabbed out of the game, the recording shows a placeholder screen instead of your desktop or other apps."
        >
          <Toggle label="Focus screen" checked={v.focusScreen} onChange={(b) => update((x) => void (x.video.focusScreen = b))} />
        </Field>
        <Field label="Monitor" hint="Used when no game window is detected; games are captured on the monitor they run on.">
          <Select<number>
            label="Monitor"
            value={v.monitorIndex}
            onChange={(val) => update((x) => void (x.video.monitorIndex = val))}
            options={
              monitors.length
                ? monitors.map((m) => ({
                    value: m.globalIdx,
                    label: `Display ${m.globalIdx + 1}${m.primary ? " (primary)" : ""} – ${m.width}×${m.height}`,
                  }))
                : [{ value: 0, label: "Primary display" }]
            }
          />
        </Field>
        <Field label="Capture mouse cursor">
          <Toggle label="Capture cursor" checked={v.captureCursor} onChange={(b) => update((x) => void (x.video.captureCursor = b))} />
        </Field>
      </Section>
    </>
  );
}
