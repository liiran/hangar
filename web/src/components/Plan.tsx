import { ChevronRight, ClipboardList } from "lucide-react";
import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, type DecisionDetail, type Session } from "../api";
import { AFTER_PLAN } from "../ask";
import { hiddenParts, rightToLeft } from "../safe";
import { report } from "../store";
import { relative } from "../transcript";
import { ApprovalBox, ChoiceKey, countLines, Shown } from "./approval";
import { Markdown } from "./Markdown";
import { cx, Kbd } from "./ui";

/**
 * How much of a plan shows before "Show all" is needed, in px: about a screenful, so that a plan
 * of ordinary length shows whole, as it does in Claude Code's terminal.
 */
const PLAN_HEIGHT = 900;

/**
 * Claude's finished plan, to approve or send back. Under the plan is the question Claude Code's
 * terminal asks, "Would you like to proceed?", with its choices as a numbered list in its own
 * words: go on with edits accepted, go on approving each edit, or keep planning. The last one
 * takes what to change on the same line; with nothing typed it stops the turn, so the change
 * can be said in a message.
 */
export function PlanCard({ session, requestId, plan, file, armed }: { session: Session; requestId: string; plan: string; file: string | null; armed: boolean }) {
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [plain, setPlain] = useState(false);
  const [feedback, setFeedback] = useState("");
  const changeBox = useRef<HTMLInputElement>(null);
  const invisible = useMemo(() => hiddenParts(plan).some((p) => p.hidden), [plan]);
  const send = async (behavior: "allow" | "deny", extra: Parameters<typeof api.decide>[3]) => {
    setBusy(true);
    try {
      await api.decide(session.id, requestId, behavior, extra);
    } catch (e) {
      report(e);
      setBusy(false);
    }
  };
  // Approving waits only until the plan has been on screen a moment. A plan longer than the card
  // can be answered without opening the rest: it is the user's to read as much of as they like.
  const hold = busy || !armed;
  const change = feedback.trim();
  const keep = () => send("deny", change ? { feedback: change } : { stop: true });
  // The number keys choose, as in Claude Code's terminal; Enter in the box sends what was typed.
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.nativeEvent.isComposing) return;
    if (e.target === changeBox.current) {
      if (e.key === "Enter") {
        e.preventDefault();
        if (!busy) keep();
      }
      return;
    }
    const n = Number(e.key);
    if (n >= 1 && n <= AFTER_PLAN.length && !hold) {
      e.preventDefault();
      send("allow", { mode: AFTER_PLAN[n - 1].mode });
    } else if (n === AFTER_PLAN.length + 1) {
      e.preventDefault();
      changeBox.current?.focus();
    }
  };
  return (
    <div className="rounded-lg bg-s-needs/[0.06] p-4 ring-1 ring-inset ring-s-needs/35" data-plan-card>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] font-medium">
        <ClipboardList className="size-4 text-s-needs" />
        Claude's plan
        <span className="font-normal text-muted">Ready to code?</span>
      </div>
      <ApprovalBox prose={!plain} lines={countLines(plan)} expanded={expanded} onExpand={() => setExpanded(true)} height={PLAN_HEIGHT}>
        {plain ? <Shown text={plan} /> : <Markdown text={plan} />}
      </ApprovalBox>
      {invisible && (
        <p className="mt-2.5 text-[12px] text-s-needs">
          This plan has invisible characters in it.{" "}
          <button onClick={() => setPlain(!plain)} className="underline underline-offset-2 hover:text-fg">
            {plain ? "Show it formatted again" : "Show it as plain text, with them written out"}
          </button>
        </p>
      )}
      {rightToLeft(plan) && (
        <p className="mt-2.5 text-[12px] text-s-needs">This plan has right-to-left letters in it. On screen they can put parts of a line in a different order than they are written.</p>
      )}
      {file && (
        <p className="mt-2 text-[12px] [overflow-wrap:anywhere] text-faint">
          Saved to{" "}
          <span className="font-mono text-[11.5px]">
            <Shown text={relative(file, session.cwd)} />
          </span>
        </p>
      )}
      <p className="mt-3.5 text-[12.5px] font-medium">Would you like to proceed?</p>
      <div role="group" aria-label="What to do with the plan" onKeyDown={onKey} className="mt-1.5 divide-y divide-line overflow-hidden rounded-md bg-panel ring-1 ring-inset ring-line-strong" data-plan-choices>
        {AFTER_PLAN.map((choice, i) => (
          <button
            key={choice.mode}
            type="button"
            disabled={hold}
            onClick={() => send("allow", { mode: choice.mode })}
            className="flex w-full items-center gap-2.5 px-3 py-2 text-left text-[13px] text-fg transition-colors hover:bg-panel-2 disabled:pointer-events-none disabled:opacity-45"
          >
            <ChoiceKey>{i + 1}</ChoiceKey>
            {choice.label}
          </button>
        ))}
        <div className="flex items-center gap-2.5 px-3 py-1 focus-within:bg-panel-2">
          <button
            type="button"
            disabled={busy}
            onClick={keep}
            title={change ? "Claude gets this and plans again" : "Stops here, still in plan mode, so you can say what to change in a message"}
            className="flex shrink-0 items-center gap-2.5 py-1 text-left text-[13px] text-fg disabled:pointer-events-none disabled:opacity-45"
          >
            <ChoiceKey>{AFTER_PLAN.length + 1}</ChoiceKey>
            No, keep planning
          </button>
          <input
            ref={changeBox}
            type="text"
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            disabled={busy}
            maxLength={8000}
            placeholder="Tell Claude what to change"
            aria-label="What to change in the plan"
            className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-faint"
          />
          {change && <Kbd>⏎</Kbd>}
        </div>
      </div>
    </div>
  );
}

/** A plan after it was answered: what happened, and the plan itself on request. */
export function PlanOutcome({ plan, decision, detail }: { plan: string | null; decision: string | null; detail: DecisionDetail | null | undefined }) {
  const [open, setOpen] = useState(false);
  const went = AFTER_PLAN.find((c) => c.mode === detail?.mode)?.done;
  const label =
    decision === "allow" || decision === "allow_always"
      ? `Plan approved${went ? `, ${went}` : ""}`
      : decision === "deny"
        ? detail?.feedback
          ? "Plan sent back"
          : detail?.stopped
            ? "Plan not approved: stopped, to hear what to change"
            : "Plan not approved"
        : decision === "cancelled"
          ? "Plan withdrawn"
          : "Plan expired";
  return (
    <div data-plan-outcome>
      <button onClick={() => plan && setOpen(!open)} aria-expanded={open} disabled={!plan} className="flex max-w-full items-center gap-1 text-left text-faint enabled:hover:text-fg">
        <span className="min-w-0 [overflow-wrap:anywhere]">
          {label}
          {detail?.feedback && (
            <span className="text-muted">
              : “<Shown text={detail.feedback.length > 160 ? `${detail.feedback.slice(0, 160)}…` : detail.feedback} />”
            </span>
          )}
        </span>
        {plan && <ChevronRight className={cx("size-3 shrink-0 transition-transform", open && "rotate-90")} />}
      </button>
      {open && plan && (
        <div className="mt-2 rounded-md bg-code px-4 py-3 text-[13px] ring-1 ring-inset ring-line">
          <Markdown text={plan} />
        </div>
      )}
    </div>
  );
}
