// The in-game popup window (transparent, click-through, never focused).
import { Bookmark, Camera, CheckCircle2, Circle, Gamepad2, Square, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { api, on } from "./lib/api";
import { cx } from "./lib/format";
import type { OverlayPosition, OverlayToast, ToastKind } from "./lib/types";

const ICONS: Record<ToastKind, { icon: React.ReactNode; tone: string }> = {
  ready: { icon: <Gamepad2 size={20} />, tone: "text-sky-300 bg-sky-500/15" },
  saved: { icon: <CheckCircle2 size={20} />, tone: "text-green-300 bg-green-500/15" },
  recordStart: { icon: <Circle size={18} fill="currentColor" />, tone: "text-red-400 bg-red-500/15" },
  recordStop: { icon: <Square size={16} fill="currentColor" />, tone: "text-slate-200 bg-white/10" },
  bookmark: { icon: <Bookmark size={18} fill="currentColor" />, tone: "text-amber-300 bg-amber-500/15" },
  screenshot: { icon: <Camera size={19} />, tone: "text-sky-300 bg-sky-500/15" },
  error: { icon: <TriangleAlert size={19} />, tone: "text-red-300 bg-red-500/15" },
};

export function Overlay() {
  const [toast, setToast] = useState<OverlayToast | null>(null);
  const [corner, setCorner] = useState<OverlayPosition>("topLeft");
  const [seq, setSeq] = useState(0);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    document.documentElement.classList.add("overlay");
    const sub = on("overlay-toast", ({ toast, corner }) => {
      setToast(toast);
      setCorner(corner);
      setSeq((n) => n + 1);
      setVisible(true);
    });
    sub.then(() => api.overlayReady().catch(() => {}));
    return () => void sub.then((u) => u());
  }, []);

  // Fade out shortly before the window is hidden by the backend.
  useEffect(() => {
    if (!visible) return;
    const t = window.setTimeout(() => setVisible(false), 3200);
    return () => window.clearTimeout(t);
  }, [seq, visible]);

  if (!toast) return null;
  const { icon, tone } = ICONS[toast.kind] ?? ICONS.ready;
  const right = corner === "topRight" || corner === "bottomRight";
  return (
    <div className={cx("flex h-full items-center p-1", right ? "justify-end" : "justify-start")}>
      <div
        key={seq}
        className={cx(
          "flex w-full items-center gap-3 rounded-xl border border-white/10 bg-[#11141b]/92 px-3.5 py-3 shadow-2xl backdrop-blur transition-all duration-300",
          visible ? "translate-x-0 opacity-100" : right ? "translate-x-4 opacity-0" : "-translate-x-4 opacity-0",
          "overlay-in",
        )}
      >
        <span className={cx("flex size-10 shrink-0 items-center justify-center rounded-lg", tone)}>{icon}</span>
        <div className="min-w-0">
          <div className="truncate text-[15px] font-semibold text-white">{toast.title}</div>
          {toast.detail && <div className="mt-0.5 line-clamp-2 text-[12.5px] leading-snug text-slate-300">{toast.detail}</div>}
        </div>
      </div>
    </div>
  );
}
