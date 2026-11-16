import { Check, ChevronRight, ClipboardList } from "lucide-react";
import { useMemo, useState } from "react";
import { api, type DecisionDetail, type Session } from "../api";
import { AFTER_PLAN } from "../ask";
import { hiddenParts, rightToLeft } from "../safe";
import { report } from "../store";
import { relative } from "../transcript";
import { ApprovalBox, countLines, Shown } from "./approval";
import { Markdown } from "./Markdown";
import { Button, cx } from "./ui";

/**
 * How much of a plan shows before "Show all" is needed, in px: about a screenful, so that a plan
 * of ordinary length shows whole, as it does in Claude Code's terminal.
 */
const PLAN_HEIGHT = 900;

/**
 * Claude's finished plan, to approve or send back. The choices and their words are Claude
 * Code's own: go on with edits accepted, go on approving each edit, or keep planning (with what
 * to change, or with nothing, which stops the turn so the user can say it in a message).
 */
export function PlanCard({ session, requestId, plan, file, armed }: { session: Session; requestId: string; plan: string; file: string | null; armed: boolean }) {
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [clipped, setClipped] = useState(false);
  const [plain, setPlain] = useState(false);
  const [feedback, setFeedback] = useState("");
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
  // Approving waits until the plan has been on screen a moment and all of it is showing.
  const unread = clipped && !expanded;
  const hold = busy || !armed || unread;
  const change = feedback.trim();
  const keep = () => send("deny", change ? { feedback: change } : { stop: true });
  return (
    <div className="rounded-lg bg-s-needs/[0.06] p-4 ring-1 ring-inset ring-s-needs/35" data-plan-card>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] font-medium">
        <ClipboardList className="size-4 text-s-needs" />
        Claude's plan
        <span className="font-normal text-muted">Ready to code?</span>
      </div>
      <ApprovalBox prose={!plain} lines={countLines(plan)} expanded={expanded} onExpand={() => setExpanded(true)} onClipped={setClipped} height={PLAN_HEIGHT}>
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
      <div className="mt-3 flex flex-wrap items-center gap-2">
        {AFTER_PLAN.map((choice, i) => (
          <Button key={choice.mode} variant={i === 0 ? "primary" : "secondary"} size="sm" disabled={hold} onClick={() => send("allow", { mode: choice.mode })}>
            {i === 0 && <Check className="size-3.5" />} {choice.label}
          </Button>
        ))}
        {unread && <span className="text-[12px] text-s-needs">Show all of it to approve it.</span>}
      </div>
      <div className="mt-2 flex items-end gap-2">
        <textarea
          rows={1}
          value={feedback}
          onChange={(e) => setFeedback(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && change) {
              e.preventDefault();
              keep();
            }
          }}
          disabled={busy}
          maxLength={8000}
          placeholder="Tell Claude what to change"
          aria-label="What to change in the plan"
          className="max-h-32 min-h-7 min-w-0 flex-1 resize-none rounded-md bg-panel px-2.5 py-1 text-[12.5px] leading-relaxed outline-none ring-1 ring-inset ring-line-strong placeholder:text-faint focus-visible:ring-faint/60"
        />
        <Button variant="ghost" size="sm" disabled={busy} onClick={keep} title={change ? "Claude gets this and plans again" : "Stops here, still in plan mode, so you can say what to change in a message"}>
          No, keep planning
        </Button>
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
