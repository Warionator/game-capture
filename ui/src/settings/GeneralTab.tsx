import { FolderOpen, RefreshCw } from "lucide-react";
import { api, pickFolder } from "../lib/api";
import { useStore } from "../lib/store";
import { Button, Field, Section, Select, Slider } from "../components/ui";

export function GeneralTab() {
  const { settings, update, notify } = useStore();
  if (!settings) return null;
  const s = settings;
  return (
    <>
      <Section title="Library" description="Captures are stored as <library>/<Game>/<date>_<type>.<ext>.">
        <Field label="Library folder" hint="Moving files between game folders or restoring a backup here is picked up automatically." stacked>
          <div className="flex gap-2">
            <div className="flex h-9 min-w-0 flex-1 items-center rounded-md border border-line-strong bg-raised px-3 font-mono text-xs text-fg">
              <span className="truncate">{s.storageRoot}</span>
            </div>
            <Button
              onClick={async () => {
                const dir = await pickFolder("Choose the library folder");
                if (dir) update((x) => void (x.storageRoot = dir));
              }}
            >
              <FolderOpen size={14} /> Change…
            </Button>
            <Button
              variant="ghost"
              title="Rescan now"
              onClick={() =>
                api
                  .rescanLibrary()
                  .then(
                    (r) =>
                      r &&
                      notify(
                        `Scan complete: ${r.added} new, ${r.updated} updated, ${r.removed} removed, ${r.thumbnailsRebuilt} thumbnails rebuilt`,
                      ),
                  )
                  .catch((e) => notify(String(e), true))
              }
            >
              <RefreshCw size={14} />
            </Button>
          </div>
        </Field>
      </Section>
      <Section title="Screenshots">
        <Field label="Format" hint="PNG is lossless; JPEG files are much smaller.">
          <Select
            label="Screenshot format"
            value={s.screenshot.format}
            onChange={(v) => update((x) => void (x.screenshot.format = v))}
            options={[
              { value: "png", label: "PNG (lossless)" },
              { value: "jpeg", label: "JPEG" },
            ]}
          />
        </Field>
        {s.screenshot.format === "jpeg" && (
          <Field label="JPEG quality" stacked>
            <Slider
              label="JPEG quality"
              min={50}
              max={100}
              value={s.screenshot.jpegQuality}
              onChange={(v) => update((x) => void (x.screenshot.jpegQuality = v))}
            />
          </Field>
        )}
      </Section>
    </>
  );
}
