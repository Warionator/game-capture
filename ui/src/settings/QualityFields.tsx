import { useStore } from "../lib/store";
import type { Quality, RateControl, Resolution, Settings } from "../lib/types";
import { Field, Select, Slider } from "../components/ui";

const mbps = (kbps: number) => `${(kbps / 1000).toFixed(kbps < 10_000 ? 1 : 0)} Mbps`;

/** The quality settings an output uses: clips (replay buffer) and session
 *  recordings are configured independently. */
function qualityOf(s: Settings, target: "replay" | "recording"): Quality {
  return target === "replay" ? s.replay.quality : s.video;
}

/** Resolution, frame rate and bitrate controls for clips or recordings. */
export function QualityFields({ target }: { target: "replay" | "recording" }) {
  const { settings, update } = useStore();
  if (!settings) return null;
  const v = qualityOf(settings, target);
  const set = (fn: (q: Quality) => void) => update((x) => fn(qualityOf(x, target)));
  const rc = v.rateControl;
  const setRc = (r: RateControl) => set((q) => void (q.rateControl = r));
  return (
    <>
      <Field label="Resolution" hint="Downscaling happens on the GPU when supported. Never upscales.">
        <Select<Resolution>
          label="Resolution"
          value={v.resolution}
          onChange={(val) => set((q) => void (q.resolution = val))}
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
          onChange={(val) => set((q) => void (q.fps = val))}
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
    </>
  );
}
