import { useRef, useState } from "react";
import type { ContextUsage } from "../api";
import { clock, tokens } from "../format";
import { cx, Meter, useDismiss } from "./ui";

const R = 6;
const AROUND = 2 * Math.PI * R;

/** The color of a fill level: the accent, then orange from 75%, then red from 90% (as the memory bar). */
function tone(percent: number) {
  return percent >= 90 ? "var(--s-error)" : percent >= 75 ? "var(--s-needs)" : "var(--accent)";
}

/**
 * How full Claude's context window is: a ring that fills up, and the percentage. Every number
 * is Claude Code's own (what `/context` shows in a terminal), as of its last reply; hangar adds
 * nothing up. Hovering or clicking opens what the window is filled with.
 */
export function ContextWheel({ context }: { context: ContextUsage }) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  useDismiss(open, wrap, () => setOpen(false));
  const percent = Math.max(0, Math.min(100, Math.round(context.percent)));
  const summary = `${tokens(context.used_tokens)} of ${tokens(context.max_tokens)} tokens`;
  return (
    <div ref={wrap} className="relative" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)} data-context-wheel>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        aria-label={`Context window ${percent}% full: ${summary}`}
        className="flex items-center gap-1.5 rounded px-1 py-0.5 text-[11px] text-faint tabular-nums transition-colors hover:bg-panel-2 hover:text-muted"
      >
        <svg width="16" height="16" viewBox="0 0 16 16" className="-rotate-90" aria-hidden>
          <circle cx="8" cy="8" r={R} fill="none" stroke="var(--line-strong)" strokeWidth="2.5" />
          {percent > 0 && (
            <circle
              cx="8"
              cy="8"
              r={R}
              fill="none"
              stroke={tone(percent)}
              strokeWidth="2.5"
              strokeLinecap={percent >= 100 ? "butt" : "round"}
              strokeDasharray={`${(AROUND * percent) / 100} ${AROUND}`}
              className="transition-[stroke-dasharray] duration-700"
            />
          )}
        </svg>
        {percent}%
      </button>
      {open && (
        // The padding bridges the gap to the wheel, so moving the pointer up keeps it open.
        <div className="absolute right-0 bottom-full z-30 pb-2">
          <div role="group" aria-label="Context window" className="animate-pop w-64 rounded-lg bg-panel p-3 text-[12px] shadow-pop">
            <div className="flex items-baseline justify-between gap-2">
              <span className="font-medium text-fg">Context window</span>
              <span className={cx("tabular-nums", percent >= 90 ? "text-s-error" : percent >= 75 ? "text-s-needs" : "text-muted")}>{percent}% full</span>
            </div>
            <div className="mt-0.5 mb-2 text-muted tabular-nums">{summary}</div>
            <Meter used={percent} total={100} />
            {context.parts.length > 0 && (
              <dl className="mt-2.5 space-y-0.5">
                {context.parts.map((p) => (
                  <div key={p.name} className="flex justify-between gap-3">
                    <dt className="min-w-0 truncate text-muted">{p.name}</dt>
                    <dd className="text-fg tabular-nums">{tokens(p.tokens)}</dd>
                  </div>
                ))}
              </dl>
            )}
            <p className="mt-2.5 border-t border-line pt-2 leading-snug text-faint">
              {context.auto_compact_at != null && <>Claude Code summarizes the conversation by itself at {tokens(context.auto_compact_at)}. </>}
              Claude Code's own figures, as of its last reply ({clock(context.at_ms)}){context.model ? `, on ${context.model}` : ""}.
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
