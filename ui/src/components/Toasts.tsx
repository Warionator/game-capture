import { AlertTriangle, CheckCircle2, X } from "lucide-react";
import { cx } from "../lib/format";
import { useStore } from "../lib/store";

export function Toasts() {
  const { toasts, dismiss } = useStore();
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-96 flex-col gap-2" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cx(
            "pointer-events-auto flex items-start gap-2.5 rounded-lg border bg-raised px-3 py-2.5 text-sm shadow-xl",
            t.error ? "border-red-500/40" : "border-line-strong",
          )}
        >
          {t.error ? (
            <AlertTriangle size={16} className="mt-0.5 shrink-0 text-red-400" />
          ) : (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-accent" />
          )}
          <span className="min-w-0 flex-1 break-words">{t.message}</span>
          <button onClick={() => dismiss(t.id)} className="text-faint hover:text-fg" aria-label="Dismiss">
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
