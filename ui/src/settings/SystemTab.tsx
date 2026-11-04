import { useStore } from "../lib/store";
import { Field, Section, Toggle } from "../components/ui";

export function SystemTab() {
  const { settings, update } = useStore();
  if (!settings) return null;
  const s = settings.system;
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
      <Section title="Window">
        <Field label="Close to tray" hint="Closing the window keeps game-capture running in the notification area.">
          <Toggle label="Close to tray" checked={s.closeToTray} onChange={(b) => update((x) => void (x.system.closeToTray = b))} />
        </Field>
        <Field label="Show notifications" hint="Toasts when clips, recordings and screenshots are saved.">
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
