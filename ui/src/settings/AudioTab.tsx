import { Mic, Plus, Trash2, Volume2, VolumeX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";
import type { AppTrack, AudioDevice, AudioSource, Settings, TrackLevel } from "../lib/types";
import { Badge, Button, Field, IconButton, Section, Select, TextInput, Toggle } from "../components/ui";

/** Polls peak levels and applies a fall-off so meters move smoothly. */
function useLevels(): Record<string, number> {
  const [levels, setLevels] = useState<Record<string, number>>({});
  const held = useRef<Record<string, number>>({});
  useEffect(() => {
    let live = true;
    const tick = () =>
      api
        .getAudioLevels()
        .then((ls: TrackLevel[]) => {
          if (!live) return;
          const next: Record<string, number> = {};
          for (const l of ls) next[l.title] = Math.max(l.peak, (held.current[l.title] ?? 0) * 0.8);
          held.current = next;
          setLevels(next);
        })
        .catch(() => {});
    const t = window.setInterval(tick, 80);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, []);
  return levels;
}

function Meter({ peak }: { peak: number | undefined }) {
  const db = peak && peak > 0 ? 20 * Math.log10(peak) : -60;
  const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
  return (
    <div className="relative h-1.5 w-full overflow-hidden rounded bg-line" role="meter" aria-valuenow={Math.round(db)} aria-valuemin={-60} aria-valuemax={0} aria-label="Level">
      <div
        className={cx("h-full transition-[width] duration-75", db > -3 ? "bg-rec" : db > -12 ? "bg-warn" : "bg-ok")}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

function TrackRow({
  index,
  title,
  icon,
  source,
  peak,
  onChange,
  children,
  removable,
  onRemove,
}: {
  index: number;
  title: string;
  icon?: React.ReactNode;
  source: AudioSource;
  peak: number | undefined;
  onChange: (s: Partial<AudioSource>) => void;
  children?: React.ReactNode;
  removable?: boolean;
  onRemove?: () => void;
}) {
  return (
    <div className={cx("border-b border-line py-3.5 last:border-b-0", !source.enabled && "opacity-60")}>
      <div className="flex items-center gap-3">
        <Toggle label={`Enable ${title}`} checked={source.enabled} onChange={(b) => onChange({ enabled: b })} />
        <span className="w-5 text-center font-mono text-xs text-faint">{index}</span>
        <span className="flex w-36 items-center gap-1.5 truncate text-sm font-medium">
          {icon}
          {title}
        </span>
        <IconButton label={source.muted ? "Unmute" : "Mute"} active={source.muted} onClick={() => onChange({ muted: !source.muted })}>
          {source.muted ? <VolumeX size={15} className="text-red-300" /> : <Volume2 size={15} />}
        </IconButton>
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <input
            type="range"
            aria-label={`${title} volume`}
            min={0}
            max={200}
            step={1}
            value={Math.round(source.volume * 100)}
            disabled={!source.enabled}
            onChange={(e) => onChange({ volume: Number(e.target.value) / 100 })}
            className="h-1.5 w-full cursor-pointer"
          />
          <Meter peak={source.enabled && !source.muted ? peak : 0} />
        </div>
        <span className="w-12 text-right font-mono text-xs text-muted tabular-nums">{Math.round(source.volume * 100)}%</span>
        {removable && (
          <IconButton label={`Remove ${title}`} onClick={onRemove}>
            <Trash2 size={14} />
          </IconButton>
        )}
      </div>
      {children && <div className="mt-2.5 ml-[4.75rem] flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-muted">{children}</div>}
    </div>
  );
}

function ExeInput({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const [text, setText] = useState(value.join(", "));
  useEffect(() => setText(value.join(", ")), [value]);
  return (
    <label className="flex items-center gap-2">
      Executables
      <TextInput
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() =>
          onChange(
            text
              .split(",")
              .map((s) => s.trim())
              .filter(Boolean),
          )
        }
        placeholder="Discord.exe"
        compact
        className="w-56 font-mono"
      />
    </label>
  );
}

export function AudioTab() {
  const { settings, update, status } = useStore();
  const levels = useLevels();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  useEffect(() => {
    api.listAudioDevices().then(setDevices).catch(() => {});
  }, []);
  if (!settings) return null;
  const a = settings.audio;

  // Persist + apply live.
  const change = (title: string, pick: (s: Settings) => AudioSource, patch: Partial<AudioSource>) => {
    update((x) => Object.assign(pick(x), patch));
    if (patch.volume !== undefined || patch.muted !== undefined) {
      const cur = { ...pick(settings), ...patch };
      void api.setTrackVolume(title, cur.volume, cur.muted).catch(() => {});
    }
  };

  let n = a.mixdownEnabled ? 2 : 1;
  const idx = () => n++;
  const running = status?.state === "buffering" || status?.state === "recording";
  const voiceTitle = a.voice.name || a.voice.executables[0] || "Voice";

  return (
    <>
      <Section
        title="Audio tracks"
        description="Each source is recorded to its own track. Game and app audio are isolated per process, so notification sounds stay out."
        actions={running ? <Badge tone="ok">Live levels</Badge> : <Badge>Levels appear while capturing</Badge>}
      >
        <Field
          label="Track 1 · Master mix"
          hint="A combined mix of every source marked “in mix”, for instant sharing. Isolated tracks follow it."
        >
          <Toggle label="Master mix" checked={a.mixdownEnabled} onChange={(b) => update((x) => void (x.audio.mixdownEnabled = b))} />
        </Field>
        <TrackRow
          index={idx()}
          title="Game"
          source={a.game}
          peak={levels["Game"]}
          onChange={(p) => change("Game", (x) => x.audio.game, p)}
        />
        <TrackRow
          index={idx()}
          title="Microphone"
          icon={<Mic size={13} className="text-faint" />}
          source={a.mic}
          peak={levels["Microphone"]}
          onChange={(p) => change("Microphone", (x) => x.audio.mic, p)}
        >
          <label className="flex items-center gap-2">
            Device
            <Select<string>
              label="Microphone device"
              compact
              value={a.mic.deviceId ?? ""}
              onChange={(v) => update((x) => void (x.audio.mic.deviceId = v || null))}
              options={[
                { value: "", label: "Default communications device" },
                ...devices.map((d) => ({ value: d.id, label: d.name + (d.isDefault ? " (default)" : "") })),
              ]}
            />
          </label>
          <label className="flex items-center gap-2">
            <Toggle
              label="Noise suppression"
              checked={a.mic.noiseSuppression}
              onChange={(b) => update((x) => void (x.audio.mic.noiseSuppression = b))}
            />
            Noise suppression
          </label>
        </TrackRow>
        <TrackRow
          index={idx()}
          title={voiceTitle}
          source={a.voice}
          peak={levels[voiceTitle]}
          onChange={(p) => change(voiceTitle, (x) => x.audio.voice, p)}
        >
          <ExeInput value={a.voice.executables} onChange={(v) => update((x) => void (x.audio.voice.executables = v))} />
          <label className="flex items-center gap-2">
            <Toggle
              label="Include in master mix"
              checked={a.voice.includeInMix}
              onChange={(b) => update((x) => void (x.audio.voice.includeInMix = b))}
            />
            In master mix
          </label>
        </TrackRow>
        {a.extraTracks.map((t, i) => {
          const title = t.name || t.executables[0] || `App ${i + 1}`;
          return (
            <TrackRow
              key={i}
              index={idx()}
              title={title}
              source={t}
              peak={levels[title]}
              onChange={(p) => change(title, (x) => x.audio.extraTracks[i], p)}
              removable
              onRemove={() => update((x) => void x.audio.extraTracks.splice(i, 1))}
            >
              <label className="flex items-center gap-2">
                Name
                <TextInput
                  defaultValue={t.name}
                  onBlur={(e) => update((x) => void (x.audio.extraTracks[i].name = e.target.value))}
                  compact
                  className="w-32"
                />
              </label>
              <ExeInput value={t.executables} onChange={(v) => update((x) => void (x.audio.extraTracks[i].executables = v))} />
              <label className="flex items-center gap-2">
                <Toggle
                  label="Include in master mix"
                  checked={t.includeInMix}
                  onChange={(b) => update((x) => void (x.audio.extraTracks[i].includeInMix = b))}
                />
                In master mix
              </label>
            </TrackRow>
          );
        })}
        <div className="py-3">
          <Button
            size="sm"
            onClick={() =>
              update((x) =>
                x.audio.extraTracks.push({
                  name: "Spotify",
                  executables: ["Spotify.exe"],
                  enabled: true,
                  volume: 1,
                  muted: false,
                  includeInMix: false,
                } satisfies AppTrack),
              )
            }
          >
            <Plus size={14} /> Add application track
          </Button>
        </div>
      </Section>
      <Section title="Compatibility">
        <Field
          label="System audio fallback"
          hint="Windows builds older than 19041 cannot isolate a process. Record all desktop audio as the game track instead of silence."
        >
          <Toggle
            label="System audio fallback"
            checked={a.systemAudioFallback}
            onChange={(b) => update((x) => void (x.audio.systemAudioFallback = b))}
          />
        </Field>
      </Section>
    </>
  );
}
