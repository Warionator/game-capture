import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cx } from "../lib/format";

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
  danger?: boolean;
  separatorBefore?: boolean;
  shortcut?: string;
}

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      x: Math.min(x, window.innerWidth - r.width - 8),
      y: Math.min(y, window.innerHeight - r.height - 8),
    });
  }, [x, y]);

  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      role="menu"
      onMouseDown={(e) => e.stopPropagation()}
      style={{ left: pos.x, top: pos.y }}
      className="fixed z-50 min-w-52 rounded-lg border border-line-strong bg-raised p-1 shadow-2xl"
    >
      {items.map((it) => (
        <div key={it.label}>
          {it.separatorBefore && <div className="my-1 h-px bg-line" />}
          <button
            role="menuitem"
            onClick={() => {
              onClose();
              it.onSelect();
            }}
            className={cx(
              "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-sm",
              it.danger ? "text-red-300 hover:bg-red-500/15" : "text-fg hover:bg-hover",
            )}
          >
            <span className="w-4 shrink-0 text-muted">{it.icon}</span>
            <span className="flex-1">{it.label}</span>
            {it.shortcut && <span className="text-xs text-faint">{it.shortcut}</span>}
          </button>
        </div>
      ))}
    </div>
  );
}
