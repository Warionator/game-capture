import { Eye } from "lucide-react";
import { api } from "../lib/api";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";
import type { OverlayPosition } from "../lib/types";
import { Button, Field, Section, Slider, Toggle } from "../components/ui";

const CORNERS: { value: OverlayPosition; label: string; cls: string }[] = [
  { value: "topLeft", label: "Top left", cls: "top-1.5 left-1.5" },
  { value: "topRight", label: "Top right", cls: "top-1.5 right-1.5" },
  { value: "bottomLeft", label: "Bottom left", cls: "bottom-1.5 left-1.5" },
  { value: "bottomRight", label: "Bottom right", cls: "bottom-1.5 right-1.5" },
];

/** A little screen where the user clicks the corner for popups. */
function CornerPicker({ value, onChange }: { value: OverlayPosition; onChange: (v: OverlayPosition) => void }) {
  return (
    <div className="relative h-24 w-40 rounded-md border border-line-strong bg-raised" role="radiogroup" aria-label="Popup position">
      {CORNERS.map((c) => (
        <button
          key={c.value}
          role="radio"
          aria-checked={value === c.value}
          aria-label={c.label}
          title={c.label}
          onClick={() => onChange(c.value)}
          className={cx(
            "absolute h-5 w-14 rounded-sm border transition-colors",
            c.cls,
            value === c.value ? "border-accent bg-sky-500/40" : "border-line-strong bg-hover hover:border-muted",
          )}
        />
      ))}
    </div>
  );
}

export function SystemTab() {
  const { settings, update } = useStore();
  if (!settings) return null;
  const s = settings.system;
  const o = settings.overlay;
  return (
    <>
      <Section title="Windows integration">
        <Field
          label="Disable Windows Game Mode"
          hint="Disabling Windows Game Mode prevents Windows GPU scheduling from starving background video capture buffers."
        >
          <Toggle
            label="Disable Windows Game Mode"
            checked={s.disableGameMode}
            onChange={(b) => update((x) => void (x.system.disableGameMode = b))}
          />
        </Field>
        <Field label="Start with Windows" hint="Starts minimized to the notification area so the replay buffer is ready when you play.">
          <Toggle label="Start with Windows" checked={s.autostart} onChange={(b) => update((x) => void (x.system.autostart = b))} />
        </Field>
      </Section>
      <Section
        title="In-game popups"
        description="Short popups with a chime when a game starts being captured, a clip is saved, a recording starts or stops… They never take focus and never appear in your clips."
        actions={
          <Button size="sm" variant="ghost" onClick={() => api.previewOverlay()}>
            <Eye size={14} /> Preview
          </Button>
        }
      >
        <Field label="Show popups">
          <Toggle label="Show popups" checked={o.enabled} onChange={(b) => update((x) => void (x.overlay.enabled = b))} />
        </Field>
        <Field label="Position" hint="Which corner of the game's screen popups appear in.">
          <CornerPicker value={o.position} onChange={(p) => update((x) => void (x.overlay.position = p))} />
        </Field>
        <Field label="Play a chime">
          <Toggle label="Chime" checked={o.sound} onChange={(b) => update((x) => void (x.overlay.sound = b))} />
        </Field>
        {o.sound && (
          <Field label="Chime volume" stacked>
            <Slider
              label="Chime volume"
              min={0}
              max={100}
              value={Math.round(o.volume * 100)}
              format={(v) => `${v}%`}
              onChange={(v) => update((x) => void (x.overlay.volume = v / 100))}
            />
          </Field>
        )}
        <p className="pb-3.5 text-[11px] text-faint">
          Games in exclusive fullscreen draw over every window; use borderless or windowed fullscreen to see popups.
        </p>
      </Section>
      <Section title="Window">
        <Field label="Close to tray" hint="Closing the window keeps game-capture running in the notification area.">
          <Toggle label="Close to tray" checked={s.closeToTray} onChange={(b) => update((x) => void (x.system.closeToTray = b))} />
        </Field>
        <Field label="Show notifications in the app" hint="Toasts in this window when clips, recordings and screenshots are saved.">
          <Toggle
            label="Show notifications"
            checked={s.showNotifications}
            onChange={(b) => update((x) => void (x.system.showNotifications = b))}
          />
        </Field>
      </Section>
    </>
  );
}
