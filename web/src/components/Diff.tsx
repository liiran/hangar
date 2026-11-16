import { useMemo, useState } from "react";
import type { DiffLine, FileDiff } from "../diff";
import { visible } from "../safe";
import { cx } from "./ui";

/** A longer change opens folded to this many lines. */
const FOLD = 12;
/** The most lines drawn even when opened; the file itself has the rest. */
const MAX_DRAWN = 1000;

type Entry = { kind: "line"; line: DiffLine } | { kind: "gap" };

/** `+3 −1`, for a row's header. */
export function DiffStat({ diff }: { diff: FileDiff }) {
  return (
    <span className="shrink-0 font-mono text-[11px] tabular-nums" data-diff-stat>
      {diff.added > 0 && <span className="text-emerald-600 dark:text-emerald-400">+{diff.added}</span>}
      {diff.added > 0 && diff.removed > 0 && " "}
      {diff.removed > 0 && <span className="text-s-error">−{diff.removed}</span>}
    </span>
  );
}

/**
 * The lines an edit changed, as Claude Code shows them in a terminal: each line's number in the
 * file, removed lines in red, added lines in green, and a few unchanged lines around them. A
 * long change opens folded. `dim` is for a change that was not made.
 */
export function DiffView({ diff, dim }: { diff: FileDiff; dim?: boolean }) {
  const [open, setOpen] = useState(false);
  const entries = useMemo(() => {
    const out: Entry[] = [];
    diff.hunks.forEach((h, i) => {
      if (i > 0) out.push({ kind: "gap" });
      for (const line of h) out.push({ kind: "line", line });
    });
    return out;
  }, [diff]);
  // Folding saves nothing when only a few lines would hide.
  const folds = entries.length > FOLD + 4;
  const drawn = open || !folds ? entries.slice(0, MAX_DRAWN) : entries.slice(0, FOLD);
  const numbered = entries.some((e) => e.kind === "line" && e.line.n !== null);
  const width = `${String(entries.reduce((m, e) => (e.kind === "line" && e.line.n !== null ? Math.max(m, e.line.n) : m), 0)).length + 1}ch`;
  return (
    <div className={cx("mt-2 overflow-hidden rounded-md bg-code font-mono text-[11.5px] leading-[1.6] ring-1 ring-inset ring-line", dim && "opacity-60")} dir="ltr" data-diff>
      <div className="py-1">
        {drawn.map((e, i) =>
          e.kind === "gap" ? (
            <div key={i} className="my-1 border-y border-line bg-panel-2/50 px-3 text-center text-[10.5px] leading-4 text-faint select-none" aria-hidden>
              ⋯
            </div>
          ) : (
            <Line key={i} line={e.line} numbered={numbered} width={width} />
          ),
        )}
      </div>
      {folds && (
        <button
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className="flex w-full items-center border-t border-line px-3 py-1 text-left font-sans text-[11.5px] text-muted hover:bg-panel-2 hover:text-fg"
        >
          {open ? "Show less" : `Show all ${entries.filter((e) => e.kind === "line").length} lines`}
        </button>
      )}
      {(diff.truncated || (open && entries.length > MAX_DRAWN)) && (
        <div className="border-t border-line px-3 py-1 font-sans text-[11.5px] text-faint">This change is longer than what is shown here. The file has all of it.</div>
      )}
    </div>
  );
}

const TONE: Record<DiffLine["sign"], string> = {
  "+": "bg-emerald-500/[0.13]",
  "-": "bg-s-error/[0.13]",
  " ": "",
  "\\": "",
};

function Line({ line, numbered, width }: { line: DiffLine; numbered: boolean; width: string }) {
  if (line.sign === "\\") {
    return <div className="px-3 font-sans text-[11px] text-faint italic select-none">{line.text}</div>;
  }
  return (
    <div className={cx("flex", TONE[line.sign])} data-sign={line.sign}>
      {numbered && (
        <span className="shrink-0 pl-2 text-right text-faint tabular-nums select-none" style={{ width: `calc(${width} + 0.5rem)` }}>
          {line.n ?? ""}
        </span>
      )}
      <span className={cx("w-5 shrink-0 text-center select-none", line.sign === "+" ? "text-emerald-600 dark:text-emerald-400" : line.sign === "-" ? "text-s-error" : "text-faint")}>
        {line.sign === " " ? "" : line.sign === "-" ? "−" : "+"}
      </span>
      <span className={cx("min-w-0 flex-1 pr-3 [overflow-wrap:anywhere] whitespace-pre-wrap", line.sign === " " ? "text-muted" : "text-fg")}>{visible(line.text) || " "}</span>
    </div>
  );
}
