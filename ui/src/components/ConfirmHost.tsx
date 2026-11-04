import { AlertTriangle } from "lucide-react";
import { useEffect, useRef } from "react";
import { useStore } from "../lib/store";
import { Button } from "./ui";

/** Renders the pending `confirm()` dialog in the app's own style. */
export function ConfirmHost() {
  const { pendingConfirm: c } = useStore();
  const ok = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!c) return;
    ok.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        c.resolve(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [c]);
  if (!c) return null;
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 p-6" onMouseDown={() => c.resolve(false)}>
      <div
        role="alertdialog"
        aria-modal="true"
        aria-label={c.title}
        onMouseDown={(e) => e.stopPropagation()}
        className="w-full max-w-md rounded-xl border border-line-strong bg-panel p-5 shadow-2xl"
      >
        <div className="flex gap-3">
          {c.danger && (
            <span className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-full bg-red-500/15 text-red-300">
              <AlertTriangle size={18} />
            </span>
          )}
          <div className="min-w-0">
            <h2 className="text-[15px] font-semibold">{c.title}</h2>
            <p className="mt-1.5 text-sm leading-relaxed break-words text-muted">{c.message}</p>
          </div>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => c.resolve(false)}>
            Cancel
          </Button>
          <Button ref={ok} variant={c.danger ? "danger" : "primary"} onClick={() => c.resolve(true)}>
            {c.confirmLabel ?? "OK"}
          </Button>
        </div>
      </div>
    </div>
  );
}
