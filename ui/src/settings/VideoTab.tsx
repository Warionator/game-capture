import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import type { Codec, EncoderInfo, EncoderPreference, MonitorInfo, RateControl, Resolution } from "../lib/types";
import { Badge, Button, Field, Section, Select, Slider, Toggle } from "../components/ui";

const mbps = (kbps: number) => `${(kbps / 1000).toFixed(kbps < 10_000 ? 1 : 0)} Mbps`;

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
  const rc = v.rateControl;
  const setRc = (r: RateControl) => update((x) => void (x.video.rateControl = r));

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
        <Field label="Resolution" hint="Downscaling happens on the GPU when supported. Never upscales.">
          <Select<Resolution>
            label="Resolution"
            value={v.resolution}
            onChange={(val) => update((x) => void (x.video.resolution = val))}
            options={[
              { value: "native", label: "Source (native)" },
              { value: "2160p", label: "4K (2160p)" },
              { value: "1440p", label: "1440p" },
              { value: "1080p", label: "1080p" },
              { value: "720p", label: "720p" },
            ]}
          />
        </Field>
        <Field label="Frame rate">
          <Select<number>
            label="Frame rate"
            value={v.fps}
            onChange={(val) => update((x) => void (x.video.fps = val))}
            options={[
              { value: 30, label: "30 fps" },
              { value: 60, label: "60 fps" },
              { value: 120, label: "120 fps" },
              { value: 144, label: "144 fps" },
              { value: 0, label: "Source (uncapped)" },
            ]}
          />
        </Field>
        <Field label="Rate control" hint="CBR suits streaming-style uploads; VBR saves space; constant quality targets a visual level.">
          <Select<RateControl["mode"]>
            label="Rate control"
            value={rc.mode}
            onChange={(mode) => {
              if (mode === "cbr") setRc({ mode, bitrateKbps: rc.mode === "quality" ? 25_000 : rc.bitrateKbps });
              if (mode === "vbr") {
                const b = rc.mode === "quality" ? 25_000 : rc.bitrateKbps;
                setRc({ mode, bitrateKbps: b, maxKbps: Math.min(100_000, Math.round(b * 1.6)) });
              }
              if (mode === "quality") setRc({ mode, level: 21 });
            }}
            options={[
              { value: "cbr", label: "CBR – constant bitrate" },
              { value: "vbr", label: "VBR – variable bitrate" },
              { value: "quality", label: "Constant quality (CQP / CRF)" },
            ]}
          />
        </Field>
        {rc.mode !== "quality" ? (
          <>
            <Field label={rc.mode === "vbr" ? "Target bitrate" : "Bitrate"} stacked>
              <Slider
                label="Bitrate"
                min={1000}
                max={100_000}
                step={500}
                value={rc.bitrateKbps}
                format={mbps}
                onChange={(b) =>
                  setRc(rc.mode === "vbr" ? { ...rc, bitrateKbps: b, maxKbps: Math.max(rc.maxKbps, b) } : { mode: "cbr", bitrateKbps: b })
                }
              />
            </Field>
            {rc.mode === "vbr" && (
              <Field label="Peak bitrate" stacked>
                <Slider
                  label="Peak bitrate"
                  min={1000}
                  max={100_000}
                  step={500}
                  value={rc.maxKbps}
                  format={mbps}
                  onChange={(m) => setRc({ ...rc, maxKbps: m, bitrateKbps: Math.min(rc.bitrateKbps, m) })}
                />
              </Field>
            )}
          </>
        ) : (
          <Field label="Quality level" hint="Lower is better quality and larger files. 18–23 is visually lossless for most games." stacked>
            <Slider
              label="Quality level"
              min={10}
              max={40}
              value={rc.level}
              format={(q) => `QP ${q}`}
              onChange={(level) => setRc({ mode: "quality", level })}
            />
          </Field>
        )}
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
