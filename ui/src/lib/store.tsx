import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { api, on } from "./api";
import type { EngineStatus, Notice, Settings } from "./types";

interface Toast extends Notice {
  id: number;
}

interface Store {
  settings: Settings | null;
  /** Applies a change locally and persists it (debounced). */
  update: (fn: (s: Settings) => void) => void;
  status: EngineStatus | null;
  toasts: Toast[];
  notify: (message: string, error?: boolean) => void;
  dismiss: (id: number) => void;
}

const Ctx = createContext<Store | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const saveTimer = useRef<number | undefined>(undefined);
  const pending = useRef<Settings | null>(null);
  const nextId = useRef(1);
  const quiet = useRef(false);
  quiet.current = settings ? !settings.system.showNotifications : false;

  const notify = useCallback((message: string, error = false) => {
    const id = nextId.current++;
    setToasts((t) => [...t.slice(-3), { id, message, error }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), error ? 7000 : 3500);
  }, []);

  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);

  useEffect(() => {
    api.getSettings().then(setSettings).catch((e) => notify(String(e), true));
    api.getStatus().then(setStatus).catch(() => {});
    const subs = [
      on("status", setStatus),
      // Errors always surface; confirmations respect the notification setting.
      on("notice", (n) => (n.error || !quiet.current) && notify(n.message, n.error)),
    ];
    return () => {
      subs.forEach((p) => p.then((u) => u()));
    };
  }, [notify]);

  const flush = useCallback(() => {
    const s = pending.current;
    pending.current = null;
    if (!s) return;
    api
      .saveSettings(s)
      .then((saved) => {
        // Keep local edits made while the save was in flight.
        if (!pending.current) setSettings(saved);
      })
      .catch((e) => notify(`Saving settings failed: ${e}`, true));
  }, [notify]);

  const update = useCallback(
    (fn: (s: Settings) => void) => {
      setSettings((prev) => {
        if (!prev) return prev;
        const next = structuredClone(prev);
        fn(next);
        pending.current = next;
        window.clearTimeout(saveTimer.current);
        saveTimer.current = window.setTimeout(flush, 350);
        return next;
      });
    },
    [flush],
  );

  return <Ctx.Provider value={{ settings, update, status, toasts, notify, dismiss }}>{children}</Ctx.Provider>;
}

export function useStore(): Store {
  const s = useContext(Ctx);
  if (!s) throw new Error("useStore outside StoreProvider");
  return s;
}
