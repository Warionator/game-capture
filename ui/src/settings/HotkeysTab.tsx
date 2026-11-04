import { Keyboard, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";
import type { Settings } from "../lib/types";
import { Field, IconButton, Section } from "../components/ui";

const KEY_NAMES: Record<string, string> = {
  " ": "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Escape: "Escape",
  PrintScreen: "PrintScreen",
  "+": "NumAdd",
};

/** Turns a keydown into "Ctrl+Shift+F10" (or null for lone modifiers). */
function chordFromEvent(e: KeyboardEvent): string | null {
  if (["Control", "Shift", "Alt", "Meta", "AltGraph"].includes(e.key)) return null;
  let key = KEY_NAMES[e.key] ?? e.key;
  if (e.code.startsWith("Numpad") && /^Numpad\d$/.test(e.code)) key = `Num${e.code.slice(6)}`;
  else if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (key.length === 1) key = key.toUpperCase();
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter(Boolean);
  return [...mods, key].join("+");
}

function HotkeyInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [listening, setListening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!listening) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
        setListening(false);
        return;
      }
      const chord = chordFromEvent(e);
      if (!chord) return;
      api
        .validateHotkey(chord)
        .then((canonical) => {
          setError(null);
          onChange(canonical);
          setListening(false);
        })
        .catch((err) => setError(String(err)));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [listening, onChange]);

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-1">
        <button
          onClick={() => setListening((l) => !l)}
          className={cx(
            "flex h-9 min-w-44 items-center justify-center gap-2 rounded-md border px-3 font-mono text-sm",
            listening ? "border-accent bg-sky-500/10 text-sky-200" : "border-line-strong bg-raised text-fg hover:bg-hover",
          )}
        >
          <Keyboard size={14} className="text-faint" />
          {listening ? "Press keys…" : value || "Not set"}
        </button>
        <IconButton label="Clear hotkey" onClick={() => onChange("")} disabled={!value}>
          <X size={14} />
        </IconButton>
      </div>
      {error && <span className="text-xs text-red-300">{error}</span>}
    </div>
  );
}

export function HotkeysTab() {
  const { settings, update } = useStore();
  const [errors, setErrors] = useState<string[]>([]);
  useEffect(() => {
    api.hotkeyErrors().then(setErrors).catch(() => {});
  }, [settings?.hotkeys]);
  if (!settings) return null;
  const rows: { key: keyof Settings["hotkeys"]; label: string; hint: string }[] = [
    { key: "saveReplay", label: "Save replay clip", hint: "Writes the replay buffer to disk immediately." },
    { key: "toggleRecording", label: "Start / stop recording", hint: "Continuous session recording." },
    { key: "addBookmark", label: "Add bookmark", hint: "Marks the current moment; shown as chapters and timeline markers." },
    { key: "screenshot", label: "Take screenshot", hint: "Grabs the current frame straight from the GPU framebuffer." },
  ];
  const values = Object.values(settings.hotkeys).filter(Boolean).map((v) => v.toLowerCase());
  return (
    <Section
      title="Global hotkeys"
      description="Work while a game has focus. If a game runs as administrator, run game-capture as administrator too."
    >
      {rows.map((r) => {
        const v = settings.hotkeys[r.key];
        const dup = v && values.filter((x) => x === v.toLowerCase()).length > 1;
        return (
          <Field key={r.key} label={r.label} hint={dup ? <span className="text-amber-300">Also used by another action</span> : r.hint}>
            <HotkeyInput value={v} onChange={(nv) => update((x) => void (x.hotkeys[r.key] = nv))} />
          </Field>
        );
      })}
      {errors.length > 0 && (
        <div className="py-3 text-xs text-amber-300">
          {errors.map((e) => (
            <div key={e}>{e}</div>
          ))}
        </div>
      )}
    </Section>
  );
}
