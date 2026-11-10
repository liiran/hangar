import type { ButtonHTMLAttributes, ReactNode } from "react";
import type { Status } from "../api";
import { STATUS } from "../format";

export function cx(...parts: (string | false | null | undefined)[]) {
  return parts.filter(Boolean).join(" ");
}

const TONES: Record<string, { dot: string; chip: string }> = {
  amber: { dot: "bg-amber-400", chip: "text-amber-300 bg-amber-400/10 ring-amber-400/25 [@media(prefers-color-scheme:light)]:text-amber-700" },
  sky: { dot: "bg-sky-400", chip: "text-sky-300 bg-sky-400/10 ring-sky-400/25 [@media(prefers-color-scheme:light)]:text-sky-700" },
  emerald: { dot: "bg-emerald-400", chip: "text-emerald-300 bg-emerald-400/10 ring-emerald-400/25 [@media(prefers-color-scheme:light)]:text-emerald-700" },
  violet: { dot: "bg-violet-400", chip: "text-violet-300 bg-violet-400/10 ring-violet-400/25 [@media(prefers-color-scheme:light)]:text-violet-700" },
  rose: { dot: "bg-rose-400", chip: "text-rose-300 bg-rose-400/10 ring-rose-400/25 [@media(prefers-color-scheme:light)]:text-rose-700" },
  slate: { dot: "bg-slate-400", chip: "text-slate-300 bg-slate-400/10 ring-slate-400/25 [@media(prefers-color-scheme:light)]:text-slate-600" },
  zinc: { dot: "bg-zinc-500", chip: "text-zinc-400 bg-zinc-400/10 ring-zinc-400/20 [@media(prefers-color-scheme:light)]:text-zinc-600" },
};

export function StatusDot({ status, className }: { status: Status; className?: string }) {
  const tone = TONES[STATUS[status].tone];
  const pulse = status === "working" || status === "starting";
  return <span className={cx("inline-block size-2 shrink-0 rounded-full", tone.dot, pulse && "animate-soft-pulse", className)} aria-hidden />;
}

export function StatusChip({ status }: { status: Status }) {
  const tone = TONES[STATUS[status].tone];
  return (
    <span className={cx("inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 ring-inset", tone.chip)}>
      <StatusDot status={status} className="size-1.5" />
      {STATUS[status].label}
    </span>
  );
}

type Variant = "primary" | "secondary" | "ghost" | "danger";

export function Button({ variant = "secondary", size = "md", className, children, ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" }) {
  const base = "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium transition-colors disabled:opacity-40 disabled:pointer-events-none select-none";
  const sizes = size === "sm" ? "h-7 px-2.5 text-xs" : "h-9 px-3.5 text-[13px]";
  const variants: Record<Variant, string> = {
    primary: "bg-accent text-accent-fg hover:brightness-110 shadow-sm",
    secondary: "bg-panel-2 text-fg ring-1 ring-inset ring-line hover:bg-raised",
    ghost: "text-muted hover:text-fg hover:bg-panel-2",
    danger: "bg-rose-500/12 text-rose-400 ring-1 ring-inset ring-rose-500/25 hover:bg-rose-500/20 [@media(prefers-color-scheme:light)]:text-rose-700",
  };
  return (
    <button type="button" className={cx(base, sizes, variants[variant], className)} {...rest}>
      {children}
    </button>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-line bg-panel-2 px-1 font-mono text-[10px] text-faint">{children}</kbd>;
}

export function Meter({ used, total, reserve }: { used: number; total: number; reserve?: number }) {
  const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
  const reservePct = reserve && total > 0 ? Math.min(100, (reserve / total) * 100) : 0;
  const tone = pct > 90 ? "bg-rose-400" : pct > 75 ? "bg-amber-400" : "bg-accent";
  return (
    <div className="relative h-1.5 w-full overflow-hidden rounded-full bg-raised">
      <div className={cx("h-full rounded-full transition-[width] duration-700", tone)} style={{ width: `${pct}%` }} />
      {reservePct > 0 && <div className="absolute inset-y-0 right-0 border-l border-dashed border-faint/70 bg-faint/10" style={{ width: `${reservePct}%` }} title="Reserve hangar keeps free" />}
    </div>
  );
}
