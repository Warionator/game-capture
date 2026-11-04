import { X } from "lucide-react";
import { useEffect, type ReactNode } from "react";
import { cx } from "../lib/format";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "record";

export function Button({
  variant = "secondary",
  size = "md",
  className,
  children,
  ref,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  ref?: React.Ref<HTMLButtonElement>;
}) {
  return (
    <button
      ref={ref}
      {...rest}
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap transition-colors disabled:cursor-not-allowed disabled:opacity-50",
        size === "sm" ? "h-7 px-2.5 text-xs" : "h-9 px-3.5 text-sm",
        variant === "primary" && "bg-accent text-slate-950 hover:bg-accent-strong",
        variant === "secondary" && "border border-line-strong bg-raised text-fg hover:bg-hover",
        variant === "ghost" && "text-muted hover:bg-hover hover:text-fg",
        variant === "danger" && "border border-red-500/40 bg-red-500/10 text-red-300 hover:bg-red-500/20",
        variant === "record" && "bg-rec text-white hover:bg-red-600",
        className,
      )}
    >
      {children}
    </button>
  );
}

export function IconButton({
  label,
  className,
  children,
  active,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      {...rest}
      aria-label={label}
      title={label}
      className={cx(
        "inline-flex size-8 items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg disabled:opacity-40",
        active && "bg-hover text-fg",
        className,
      )}
    >
      {children}
    </button>
  );
}

export function Toggle({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-40",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span
        className={cx(
          "inline-block size-4 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-[18px]" : "translate-x-0.5",
        )}
      />
    </button>
  );
}

export function Select<T extends string | number>({
  value,
  options,
  onChange,
  className,
  disabled,
  label,
  compact,
}: {
  value: T;
  options: { value: T; label: string; disabled?: boolean }[];
  onChange: (v: T) => void;
  className?: string;
  disabled?: boolean;
  label?: string;
  compact?: boolean;
}) {
  return (
    <select
      aria-label={label}
      disabled={disabled}
      value={String(value)}
      onChange={(e) => {
        const raw = e.target.value;
        const opt = options.find((o) => String(o.value) === raw);
        if (opt) onChange(opt.value);
      }}
      className={cx(
        "rounded-md border border-line-strong bg-raised text-fg outline-none hover:bg-hover focus:border-accent disabled:opacity-50",
        compact ? "h-8 px-2 text-xs" : "h-9 px-2.5 text-sm",
        className,
      )}
    >
      {options.map((o) => (
        <option key={String(o.value)} value={String(o.value)} disabled={o.disabled} className="bg-panel">
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function TextInput({
  className,
  compact,
  ...rest
}: React.InputHTMLAttributes<HTMLInputElement> & { compact?: boolean }) {
  return (
    <input
      {...rest}
      className={cx(
        "rounded-md border border-line-strong bg-raised px-3 text-fg outline-none placeholder:text-faint focus:border-accent",
        compact ? "h-8 text-xs" : "h-9 text-sm",
        className,
      )}
    />
  );
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  format,
  label,
  className,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  format?: (v: number) => string;
  label: string;
  className?: string;
}) {
  return (
    <div className={cx("flex items-center gap-3", className)}>
      <input
        type="range"
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1.5 w-full min-w-0 cursor-pointer"
      />
      <span className="w-20 shrink-0 text-right font-mono text-xs text-muted tabular-nums">
        {format ? format(value) : value}
      </span>
    </div>
  );
}

/** A labelled settings row: label + hint on the left, control on the right. */
export function Field({
  label,
  hint,
  children,
  stacked,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
  stacked?: boolean;
}) {
  return (
    <div
      className={cx(
        "border-b border-line py-3.5 last:border-b-0",
        stacked ? "space-y-2.5" : "flex items-center justify-between gap-6",
      )}
    >
      <div className="min-w-0">
        <div className="text-sm font-medium text-fg">{label}</div>
        {hint && <div className="mt-0.5 max-w-xl text-xs leading-relaxed text-muted">{hint}</div>}
      </div>
      <div className={cx(stacked ? "" : "shrink-0")}>{children}</div>
    </div>
  );
}

export function Section({
  title,
  description,
  children,
  actions,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="rounded-lg border border-line bg-panel">
      <header className="flex items-start justify-between gap-4 border-b border-line px-5 py-3.5">
        <div>
          <h2 className="text-[15px] font-semibold text-fg">{title}</h2>
          {description && <p className="mt-0.5 text-xs text-muted">{description}</p>}
        </div>
        {actions}
      </header>
      <div className="px-5">{children}</div>
    </section>
  );
}

export function Badge({ children, tone = "default" }: { children: ReactNode; tone?: "default" | "ok" | "warn" | "rec" | "accent" }) {
  return (
    <span
      className={cx(
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium whitespace-nowrap",
        tone === "default" && "bg-hover text-muted",
        tone === "ok" && "bg-green-500/15 text-green-300",
        tone === "warn" && "bg-amber-500/15 text-amber-300",
        tone === "rec" && "bg-red-500/15 text-red-300",
        tone === "accent" && "bg-sky-500/15 text-sky-300",
      )}
    >
      {children}
    </span>
  );
}

export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/70 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
        className={cx(
          "flex max-h-full w-full flex-col overflow-hidden rounded-xl border border-line-strong bg-panel shadow-2xl",
          wide ? "max-w-6xl" : "max-w-lg",
        )}
      >
        <header className="flex items-center justify-between gap-4 border-b border-line px-4 py-2.5">
          <h2 className="truncate text-sm font-semibold">{title}</h2>
          <IconButton label="Close" onClick={onClose}>
            <X size={16} />
          </IconButton>
        </header>
        <div className="min-h-0 overflow-auto">{children}</div>
      </div>
    </div>
  );
}

export function Empty({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
      <div className="text-faint">{icon}</div>
      <div className="text-base font-medium">{title}</div>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  );
}
