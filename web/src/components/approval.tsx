// What every card that waits for the user shares: text shown honestly, and content that cannot
// be approved before all of it has been on screen.

import { Check, ChevronRight } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { Session } from "../api";
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

/**
 * A request ignores clicks for this long after it appears, and after any request of its session
 * is answered. Answering one moves the next one up, under the cursor: a click meant for the
 * first, or the second half of a double-click, must not land on it.
 */
export const ARM_MS = 600;

/** Whether the session's requests take clicks yet (see [`ARM_MS`]). */
export function useArmed(session: Session): boolean {
  const [armed, setArmed] = useState(false);
  const waiting = session.pending.map((p) => p.request_id).join(" ");
  useEffect(() => {
    setArmed(false);
    const t = setTimeout(() => setArmed(true), ARM_MS);
    return () => clearTimeout(t);
  }, [waiting]);
  return armed;
}

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
 * the side: long content is cut at a visible line, and `onClipped` tells the
 * row to hold Allow back until the user has opened all of it. `prose` is for
 * formatted text (a plan) instead of code.
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
  onClipped: (clipped: boolean) => void;
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
      onClipped(now);
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
