import { Info, Mic, Plus, Trash2, Volume2, VolumeX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";
import type { AppTrack, AudioDevice, AudioSource, Settings, TrackLevel } from "../lib/types";
import { Badge, Button, Field, IconButton, Section, Select, TextInput, Tip, Toggle } from "../components/ui";

/** Track name for an executable, matching the backend: `spotify.exe` → `Spotify`. */
function appLabel(exe: string): string {
  const stem = exe.trim().replace(/\.exe$/i, "");
  return stem ? stem[0].toUpperCase() + stem.slice(1) : "App";
}

const MIX_HINT =
  "On: this app is also mixed into Track 1 (the combined “Master” track you hear by default and when sharing). Off: it is only recorded on its own track, e.g. music you want to keep out of shared clips but still have available when editing.";

interface Levels {
  peaks: Record<string, number>;
  gateOpen: Record<string, boolean | null>;
}

/** Polls peak levels and applies a fall-off so meters move smoothly. */
function useLevels(): Levels {
  const [levels, setLevels] = useState<Levels>({ peaks: {}, gateOpen: {} });
  const held = useRef<Record<string, number>>({});
  useEffect(() => {
    let live = true;
    const tick = () =>
      api
        .getAudioLevels()
        .then((ls: TrackLevel[]) => {
          if (!live) return;
          const next: Record<string, number> = {};
          const gate: Record<string, boolean | null> = {};
          for (const l of ls) {
            next[l.title] = Math.max(l.peak, (held.current[l.title] ?? 0) * 0.8);
            gate[l.title] = l.gateOpen;
          }
          held.current = next;
          setLevels({ peaks: next, gateOpen: gate });
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

const METER_FLOOR_DB = -80;
const dbToPct = (db: number) => Math.max(0, Math.min(100, ((db - METER_FLOOR_DB) / -METER_FLOOR_DB) * 100));

/** Level meter (dBFS). With a noise gate, a marker shows the threshold and the
 *  bar dims while the gate is closed. */
function Meter({ peak, gateDb, gateOpen }: { peak: number | undefined; gateDb?: number | null; gateOpen?: boolean | null }) {
  const db = peak && peak > 0 ? 20 * Math.log10(peak) : METER_FLOOR_DB;
  const closed = gateDb != null && gateOpen === false;
  return (
    <div
      className="relative h-1.5 w-full rounded bg-line"
      role="meter"
      aria-valuenow={Math.round(db)}
      aria-valuemin={METER_FLOOR_DB}
      aria-valuemax={0}
      aria-label="Level"
    >
      <div
        className={cx(
          "h-full rounded transition-[width] duration-75",
          closed ? "bg-faint/60" : db > -3 ? "bg-rec" : db > -12 ? "bg-warn" : "bg-ok",
        )}
        style={{ width: `${dbToPct(db)}%` }}
      />
      {gateDb != null && (
        <div
          className="absolute -top-1 h-3.5 w-0.5 -translate-x-1/2 rounded bg-sky-300"
          style={{ left: `${dbToPct(gateDb)}%` }}
          title={`Gate threshold ${gateDb} dB`}
        />
      )}
    </div>
  );
}

function TrackRow({
  index,
  title,
  icon,
  source,
  peak,
  gateDb,
  gateOpen,
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
  gateDb?: number | null;
  gateOpen?: boolean | null;
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
          <Meter peak={source.enabled && !source.muted ? peak : 0} gateDb={gateDb} gateOpen={gateOpen} />
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

function MicControl({
  label,
  hint,
  enabled,
  onToggle,
  value,
  min,
  max,
  format,
  onValue,
  status,
}: {
  label: string;
  hint: string;
  enabled: boolean;
  onToggle: (b: boolean) => void;
  value: number;
  min: number;
  max: number;
  format: (v: number) => string;
  onValue: (v: number) => void;
  status?: "open" | "closed";
}) {
  return (
    <div>
      <div className="flex items-center gap-3">
        <Toggle label={label} checked={enabled} onChange={onToggle} />
        <span className="w-32 shrink-0 text-xs font-medium text-fg">{label}</span>
        <input
          type="range"
          aria-label={`${label} level`}
          min={min}
          max={max}
          step={1}
          value={value}
          disabled={!enabled}
          onChange={(e) => onValue(Number(e.target.value))}
          className="h-1.5 min-w-0 flex-1 cursor-pointer disabled:opacity-40"
        />
        <span className="w-14 shrink-0 text-right font-mono text-xs tabular-nums">{format(value)}</span>
        <span className="w-14 shrink-0 text-right">
          {status && <Badge tone={status === "open" ? "ok" : "default"}>{status === "open" ? "open" : "gated"}</Badge>}
        </span>
      </div>
      <p className="mt-1 ml-12 text-[11px] leading-relaxed text-faint">{hint}</p>
    </div>
  );
}

export function AudioTab() {
  const { settings, update, status } = useStore();
  const levels = useLevels();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loopback, setLoopback] = useState<boolean | null>(null);
  useEffect(() => {
    api.listAudioDevices().then(setDevices).catch(() => {});
    api.systemInfo().then((i) => setLoopback(i.processLoopback)).catch(() => {});
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
  const voiceTitle = a.voice.name.trim() || (a.voice.executables[0] ? appLabel(a.voice.executables[0]) : "Voice");

  return (
    <>
      <Section
        title="Audio tracks"
        description="Each source is recorded to its own track. Game and app audio are isolated per process, so notification sounds stay out."
        actions={<Badge tone="ok">{running ? "Live levels" : "Live levels (preview)"}</Badge>}
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
          peak={levels.peaks["Game"]}
          onChange={(p) => change("Game", (x) => x.audio.game, p)}
        />
        <TrackRow
          index={idx()}
          title="Microphone"
          icon={<Mic size={13} className="text-faint" />}
          source={a.mic}
          peak={levels.peaks["Microphone"]}
          gateDb={a.mic.noiseGate ? a.mic.noiseGateDb : null}
          gateOpen={levels.gateOpen["Microphone"]}
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
          <div className="flex w-full flex-col gap-2.5 pt-1">
            <MicControl
              label="Noise suppression"
              hint="Removes steady background noise (fans, hum). Stronger values can make voices sound thin. Restarts the buffer when changed."
              enabled={a.mic.noiseSuppression}
              onToggle={(b) => update((x) => void (x.audio.mic.noiseSuppression = b))}
              value={a.mic.noiseSuppressionStrength}
              min={1}
              max={40}
              format={(v) => `${v} dB`}
              onValue={(v) => update((x) => void (x.audio.mic.noiseSuppressionStrength = v))}
            />
            <MicControl
              label="Noise gate"
              hint="Mutes the mic while it is quieter than the threshold (the blue marker on the meter). Set it just above your room noise. Applies live."
              enabled={a.mic.noiseGate}
              onToggle={(b) => update((x) => void (x.audio.mic.noiseGate = b))}
              value={a.mic.noiseGateDb}
              min={-80}
              max={-10}
              format={(v) => `${v} dB`}
              onValue={(v) => update((x) => void (x.audio.mic.noiseGateDb = v))}
              status={
                a.mic.noiseGate && levels.gateOpen["Microphone"] != null
                  ? levels.gateOpen["Microphone"]
                    ? "open"
                    : "closed"
                  : undefined
              }
            />
          </div>
        </TrackRow>
        <TrackRow
          index={idx()}
          title={voiceTitle}
          source={a.voice}
          peak={levels.peaks[voiceTitle]}
          onChange={(p) => change(voiceTitle, (x) => x.audio.voice, p)}
        >
          <ExeInput value={a.voice.executables} onChange={(v) => update((x) => void (x.audio.voice.executables = v))} />
          <Tip text={MIX_HINT} wide>
            <label className="flex cursor-help items-center gap-2">
              <Toggle
                label="Include in master mix"
                checked={a.voice.includeInMix}
                onChange={(b) => update((x) => void (x.audio.voice.includeInMix = b))}
              />
              In master mix
              <Info size={12} className="text-faint" />
            </label>
          </Tip>
        </TrackRow>
        {a.extraTracks.map((t, i) => {
          const title = t.name.trim() || (t.executables[0] ? appLabel(t.executables[0]) : `App ${i + 1}`);
          return (
            <TrackRow
              key={i}
              index={idx()}
              title={title}
              source={t}
              peak={levels.peaks[title]}
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
              <Tip text={MIX_HINT} wide>
                <label className="flex cursor-help items-center gap-2">
                  <Toggle
                    label="Include in master mix"
                    checked={t.includeInMix}
                    onChange={(b) => update((x) => void (x.audio.extraTracks[i].includeInMix = b))}
                  />
                  In master mix
                  <Info size={12} className="text-faint" />
                </label>
              </Tip>
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
          hint={
            loopback
              ? "Not needed: this Windows version records each app separately."
              : "This Windows build (older than 19041) can't isolate an app's audio. Record all desktop audio as the game track instead of silence."
          }
        >
          <Toggle
            label="System audio fallback"
            disabled={loopback !== false}
            checked={loopback === false ? (a.systemAudioFallback ?? true) : false}
            onChange={(b) => update((x) => void (x.audio.systemAudioFallback = b))}
          />
        </Field>
      </Section>
    </>
  );
}
