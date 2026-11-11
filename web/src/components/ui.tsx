import { forwardRef, useEffect, useRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import type { Status } from "../api";
import { STATUS } from "../format";

export function cx(...parts: (string | false | null | undefined)[]) {
  return parts.filter(Boolean).join(" ");
}

/**
 * Linear-style status glyphs: dashed for not started, a half-filled pie for
 * in progress, a solid disc with a mark for needs-you, ready and error.
 */
export function StatusIcon({ status, size = 14, className }: { status: Status; size?: number; className?: string }) {
  const common = { width: size, height: size, viewBox: "0 0 14 14", className: cx("shrink-0", className), "aria-hidden": true } as const;
  switch (status) {
    case "queued":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="5.5" fill="none" stroke="var(--s-idle)" strokeWidth="1.5" strokeDasharray="1.6 1.9" />
        </svg>
      );
    case "starting":
      return (
        <svg {...common} className={cx(common.className, "animate-spin-slow")}>
          <circle cx="7" cy="7" r="5.5" fill="none" stroke="var(--s-working)" strokeWidth="1.5" strokeDasharray="1.6 1.9" />
        </svg>
      );
    case "working":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="5.5" fill="none" stroke="var(--s-working)" strokeWidth="1.5" />
          <path d="M7 3.5 A3.5 3.5 0 0 1 7 10.5 Z" fill="var(--s-working)" />
        </svg>
      );
    case "needs_you":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="6.25" fill="var(--s-needs)" />
          <rect x="6.25" y="3.4" width="1.5" height="4.4" rx="0.75" fill="var(--panel)" />
          <circle cx="7" cy="9.85" r="0.85" fill="var(--panel)" />
        </svg>
      );
    case "idle":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="6.25" fill="var(--s-done)" />
          <path d="M4.4 7.2 6.2 9 9.7 5.3" fill="none" stroke="var(--panel)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "hibernated":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="5.5" fill="none" stroke="var(--s-idle)" strokeWidth="1.5" />
          <rect x="5" y="4.6" width="1.4" height="4.8" rx="0.5" fill="var(--s-idle)" />
          <rect x="7.6" y="4.6" width="1.4" height="4.8" rx="0.5" fill="var(--s-idle)" />
        </svg>
      );
    case "stopped":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="6.25" fill="var(--s-idle)" />
          <rect x="4.9" y="4.9" width="4.2" height="4.2" rx="0.8" fill="var(--panel)" />
        </svg>
      );
    case "error":
      return (
        <svg {...common}>
          <circle cx="7" cy="7" r="6.25" fill="var(--s-error)" />
          <path d="M4.9 4.9 9.1 9.1M9.1 4.9 4.9 9.1" stroke="var(--panel)" strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      );
  }
}

/** Status icon and label, for headers and properties. */
export function StatusLabel({ status, className }: { status: Status; className?: string }) {
  return (
    <span className={cx("inline-flex items-center gap-1.5", className)} title={STATUS[status].hint}>
      <StatusIcon status={status} />
      {STATUS[status].label}
    </span>
  );
}

type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg shadow-sm hover:brightness-110",
  secondary: "bg-panel-2 text-fg ring-1 ring-inset ring-line-strong hover:bg-raised",
  ghost: "text-muted hover:bg-panel-2 hover:text-fg",
  danger: "text-s-error ring-1 ring-inset ring-s-error/30 hover:bg-s-error/10",
};

export const Button = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" }>(function Button(
  { variant = "secondary", size = "md", className, children, ...rest },
  ref,
) {
  const sizes = size === "sm" ? "h-7 px-2.5 text-[12px] gap-1.5" : "h-8 px-3 text-[12.5px] gap-1.5";
  return (
    <button
      ref={ref}
      type="button"
      className={cx(
        "inline-flex shrink-0 select-none items-center justify-center rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-40",
        sizes,
        VARIANTS[variant],
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
});

/** A square, icon-only button. `label` is its accessible name and tooltip. */
export const IconButton = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean; shortcut?: string }>(
  function IconButton({ label, active, shortcut, className, children, ...rest }, ref) {
    return (
      <button
        ref={ref}
        type="button"
        aria-label={label}
        title={shortcut ? `${label}  ${shortcut}` : label}
        className={cx(
          "grid size-7 shrink-0 place-items-center rounded-md text-muted transition-colors hover:bg-panel-2 hover:text-fg disabled:pointer-events-none disabled:opacity-40",
          active && "bg-panel-2 text-fg",
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    );
  },
);

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd className={cx("inline-grid h-[18px] min-w-[18px] place-items-center rounded border whitespace-nowrap border-line-strong bg-panel-2 px-1 font-sans text-[10.5px] font-medium text-muted", className)}>
      {children}
    </kbd>
  );
}

export function Meter({ used, total, reserve }: { used: number; total: number; reserve?: number }) {
  const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
  const reservePct = reserve && total > 0 ? Math.min(100, (reserve / total) * 100) : 0;
  const tone = pct > 90 ? "bg-s-error" : pct > 75 ? "bg-s-needs" : "bg-accent";
  return (
    <div className="relative h-1 w-full overflow-hidden rounded-full bg-raised" role="meter" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
      <div className={cx("h-full rounded-full transition-[width] duration-700", tone)} style={{ width: `${pct}%` }} />
      {reservePct > 0 && <div className="absolute inset-y-0 right-0 border-l border-faint/60" style={{ width: `${reservePct}%` }} title="Memory hangar keeps free" />}
    </div>
  );
}

/** The mark: a hangar roof, in the accent color. */
/**
 * The hangar mark. At 16 px it uses the drawing made for that pixel grid
 * (web/icons/logo-16.svg, also the favicon), so it stays sharp.
 */
export function Logo({ size = 32, className }: { size?: number; className?: string }) {
  const small = size <= 16;
  return (
    <svg width={size} height={size} viewBox={small ? "0 0 16 16" : "0 0 32 32"} className={cx("shrink-0", className)} aria-hidden>
      {small ? (
        <>
          <rect width="16" height="16" rx="4" fill="var(--accent)" />
          <path d="M3 11 L8 4.5 L13 11" fill="none" stroke="#fff" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" />
          <path d="M5.5 12V8.5h5V12" fill="none" stroke="#fff" strokeWidth="1" />
        </>
      ) : (
        <>
          <rect width="32" height="32" rx="8" fill="var(--accent)" />
          <path d="M6 22 L16 9 L26 22" fill="none" stroke="#fff" strokeWidth="2.6" strokeLinejoin="round" strokeLinecap="round" />
          <path d="M11.5 22v-4.5h9V22" fill="none" stroke="#fff" strokeWidth="2.2" strokeLinejoin="round" />
        </>
      )}
    </svg>
  );
}

/**
 * Dialog behavior: Escape closes it, and focus goes back where it was when
 * the dialog closes.
 */
export function useDialog(onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented && !e.isComposing) {
        e.preventDefault();
        close.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      before?.focus?.();
    };
  }, []);
}

/** Closes a popover on Escape or a click outside `ref`. */
export function useDismiss(open: boolean, ref: React.RefObject<HTMLElement | null>, close: () => void) {
  const fn = useRef(close);
  fn.current = close;
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) fn.current();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        fn.current();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open, ref]);
}
