import { FolderOpen, FolderPlus, RefreshCw, Trash2 } from "lucide-react";
import { api, pickFolder } from "../lib/api";
import { useStore } from "../lib/store";
import { Button, Field, IconButton, Section, Select, Slider } from "../components/ui";

export function GeneralTab() {
  const { settings, update, notify } = useStore();
  if (!settings) return null;
  const s = settings;
  const sameFolder = (a: string, b: string) =>
    a.replace(/[\\/]+$/, "").toLowerCase() === b.replace(/[\\/]+$/, "").toLowerCase();
  return (
    <>
      <Section title="Library" description="Captures are stored as <library>/<Game>/<date>_<type>.<ext>.">
        <Field
          label="Library folder"
          hint="New captures are saved here. Moving files between game folders or restoring a backup here is picked up automatically."
          stacked
        >
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
      <Section
        title="Additional folders"
        description="Show clips from other recorders (Medal, ShadowPlay, OBS…) in the gallery. Files there are only read – never moved or renamed."
        actions={
          <Button
            size="sm"
            onClick={async () => {
              const dir = await pickFolder("Add a folder to the gallery");
              if (!dir) return;
              if (sameFolder(dir, s.storageRoot) || s.extraLibraryFolders.some((f) => sameFolder(f, dir))) {
                notify("That folder is already in the gallery");
                return;
              }
              update((x) => void x.extraLibraryFolders.push(dir));
            }}
          >
            <FolderPlus size={14} /> Add folder…
          </Button>
        }
      >
        {s.extraLibraryFolders.length === 0 ? (
          <p className="py-4 text-sm text-faint">
            None. Only the library folder above is shown in the gallery.
          </p>
        ) : (
          s.extraLibraryFolders.map((f, i) => (
            <div key={f} className="flex items-center gap-3 border-b border-line py-2.5 last:border-b-0">
              <FolderOpen size={15} className="shrink-0 text-faint" />
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={f}>
                {f}
              </span>
              <IconButton
                label={`Remove ${f} from the gallery`}
                onClick={() => update((x) => void x.extraLibraryFolders.splice(i, 1))}
              >
                <Trash2 size={14} />
              </IconButton>
            </div>
          ))
        )}
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
