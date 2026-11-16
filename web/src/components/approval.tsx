// What every card that waits for the user shares: text shown honestly, and long content cut at a
// visible line with a way to open the rest.

import { Check, ChevronRight } from "lucide-react";
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { hiddenParts } from "../safe";
import { cx } from "./ui";

/**
 * Text the user is asked to approve. Invisible and direction-changing
 * characters are written out and marked, so it cannot read differently
 * from what would run.
 */
export function Shown({ text }: { text: string }) {
  return (
    <>
      {hiddenParts(text).map((p, i) =>
        p.hidden ? (
          <mark key={i} className="rounded-sm bg-s-error/20 px-0.5 text-s-error" title="An invisible character in this text">
            {p.text}
          </mark>
        ) : (
          p.text
        ),
      )}
    </>
  );
}

/** How much of a request shows before "Show all" is needed, in px. */
export const PREVIEW_HEIGHT = 288;

/** The key of a choice in a list of choices: its letter or number, or a tick once it is chosen. */
export function ChoiceKey({ on, children }: { on?: boolean; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cx(
        "mt-px grid size-[18px] shrink-0 place-items-center rounded text-[10.5px] font-medium ring-1 ring-inset transition-colors",
        on ? "bg-accent text-accent-fg ring-accent" : "bg-panel-2 text-muted ring-line-strong",
      )}
    >
      {on ? <Check className="size-3" strokeWidth={3} /> : children}
    </span>
  );
}

export const countLines = (...texts: string[]) => texts.reduce((n, t) => n + t.split("\n").length, 0);

/**
 * The exact content being approved. It never scrolls and never runs off to
 * the side: long content is cut at a visible line. `onClipped` tells a row
 * that holds Allow back until the user has opened all of it (a command, an
 * edit) that there is more; a plan does not hold anything back. `prose` is
 * for formatted text (a plan) instead of code.
 */
export function ApprovalBox({
  children,
  lines,
  expanded,
  onExpand,
  onClipped,
  prose,
  height = PREVIEW_HEIGHT,
}: {
  children: ReactNode;
  lines: number;
  expanded: boolean;
  onExpand: () => void;
  onClipped?: (clipped: boolean) => void;
  prose?: boolean;
  height?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const now = el.scrollHeight > height + 1;
      setClipped(now);
      onClipped?.(now);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [children, height]);
  return (
    <div className="mt-2.5 overflow-hidden rounded-md bg-code ring-1 ring-inset ring-line">
      <div
        ref={ref}
        dir="ltr"
        data-approval-content
        className={cx("overflow-hidden [overflow-wrap:anywhere]", prose ? "px-4 py-3 text-[13px]" : "p-3 font-mono text-[12px] whitespace-pre-wrap")}
        style={expanded ? undefined : { maxHeight: height }}
      >
        {children}
      </div>
      {clipped && !expanded && (
        <button onClick={onExpand} className="flex w-full items-center justify-center gap-1.5 border-t border-line-strong bg-s-needs/10 px-3 py-1.5 text-[12px] font-medium text-fg hover:bg-s-needs/20">
          <ChevronRight className="size-3 rotate-90" />
          There is more below. Show all {lines} lines
        </button>
      )}
    </div>
  );
}

/** Input fields the preview did not already show, so none of them is silent. */
export function OtherFields({ input, shown }: { input: unknown; shown: string[] }) {
  if (!input || typeof input !== "object") return null;
  const rest = Object.entries(input).filter(([k, v]) => !shown.includes(k) && v !== undefined && v !== null && v !== false && v !== "");
  if (rest.length === 0) return null;
  return (
    <p className="mt-2 text-[12px] [overflow-wrap:anywhere] text-muted">
      <span className="text-faint">Also set: </span>
      {rest.map(([k, v], i) => (
        <span key={k} className="font-mono text-[11.5px]">
          {i > 0 && " · "}
          <Shown text={`${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`} />
        </span>
      ))}
    </p>
  );
}
