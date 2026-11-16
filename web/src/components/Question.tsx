import { Check, CircleHelp, MessagesSquare } from "lucide-react";
import { useState, type KeyboardEvent } from "react";
import { api, type DecisionDetail, type Session } from "../api";
import { answerOf, answersOf, NOTHING, pick, toggleOther, type, type Picked, type Question } from "../ask";
import { report } from "../store";
import { Shown } from "./approval";
import { Button, cx, Kbd } from "./ui";

/**
 * Claude's questions, to answer. Each question lists its options the way the app's own dropdowns
 * do; the last row takes an answer of the user's own. The answers go back to Claude exactly as
 * its terminal sends them: one line per question, several choices joined with ", ".
 */
export function QuestionCard({ session, requestId, questions, armed }: { session: Session; requestId: string; questions: Question[]; armed: boolean }) {
  const [picked, setPicked] = useState<Picked[]>(() => questions.map(() => NOTHING));
  const [busy, setBusy] = useState(false);
  const answers = answersOf(questions, picked);
  const answered = questions.filter((q, i) => answerOf(q, picked[i]) !== null).length;
  const set = (i: number, p: Picked) => setPicked((all) => all.map((old, j) => (j === i ? p : old)));
  const send = async (behavior: "allow" | "deny", extra: Parameters<typeof api.decide>[3]) => {
    setBusy(true);
    try {
      await api.decide(session.id, requestId, behavior, extra);
    } catch (e) {
      report(e);
      setBusy(false);
    }
  };
  const ready = answers !== null && armed && !busy;
  const submit = () => ready && send("allow", { answers });
  return (
    <div
      className="rounded-lg bg-s-needs/[0.06] p-4 ring-1 ring-inset ring-s-needs/35"
      data-question-card
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          submit();
        }
      }}
    >
      <div className="flex items-center gap-2 text-[13px] font-medium">
        <CircleHelp className="size-4 text-s-needs" />
        {questions.length === 1 ? "Claude has a question" : `Claude has ${questions.length} questions`}
      </div>
      {questions.map((q, i) => (
        <QuestionBlock key={i} q={q} picked={picked[i]} onChange={(p) => set(i, p)} disabled={busy} />
      ))}
      <div className="mt-4 flex flex-wrap items-center gap-2">
        <Button variant="primary" size="sm" disabled={!ready} onClick={submit}>
          <Check className="size-3.5" /> {questions.length === 1 ? "Send answer" : "Send answers"}
          <span className="ml-1 hidden items-center gap-0.5 sm:inline-flex">
            <Kbd className="border-white/25 bg-white/10 text-white/85">Ctrl</Kbd>
            <Kbd className="border-white/25 bg-white/10 text-white/85">⏎</Kbd>
          </span>
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => send("deny", { clarify: true })} title="Answer nothing yet: Claude asks what you would like to clear up first">
          <MessagesSquare className="size-3.5" /> Chat about this
        </Button>
        {questions.length > 1 && (
          <span className="text-[12px] text-faint" aria-live="polite">
            {answered} of {questions.length} answered
          </span>
        )}
      </div>
    </div>
  );
}

function QuestionBlock({ q, picked, onChange, disabled }: { q: Question; picked: Picked; onChange: (p: Picked) => void; disabled: boolean }) {
  const chosen = q.multi ? null : q.options.find((o) => picked.labels.includes(o.label));
  // Up and down move between the rows of one question, as in a dropdown.
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const rows = Array.from(e.currentTarget.querySelectorAll<HTMLElement>("[data-choice]"));
    const at = rows.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    e.preventDefault();
    rows[(at + (e.key === "ArrowDown" ? 1 : rows.length - 1)) % rows.length].focus();
  };
  return (
    <fieldset className="mt-3.5 min-w-0" disabled={disabled}>
      <legend className="flex w-full flex-wrap items-baseline gap-x-2 gap-y-1 text-[13px]">
        {q.header.trim() && (
          <span className="rounded bg-panel-2 px-1.5 py-px text-[11px] font-medium text-muted ring-1 ring-inset ring-line-strong">
            <Shown text={q.header} />
          </span>
        )}
        <span className="min-w-0 font-medium [overflow-wrap:anywhere]">
          <Shown text={q.question} />
        </span>
        {q.multi && <span className="text-[11.5px] text-faint">Choose any that apply</span>}
      </legend>
      <div role={q.multi ? "group" : "radiogroup"} aria-label={q.question} onKeyDown={onKey} className="mt-2 divide-y divide-line overflow-hidden rounded-md bg-panel ring-1 ring-inset ring-line-strong">
        {q.options.map((o) => {
          const on = picked.labels.includes(o.label);
          return (
            <button
              key={o.label}
              type="button"
              role={q.multi ? "checkbox" : "radio"}
              aria-checked={on}
              data-choice
              onClick={() => onChange(pick(q, picked, o.label))}
              className={cx("flex w-full items-start gap-2.5 px-3 py-2 text-left transition-colors hover:bg-panel-2", on && "bg-accent-soft hover:bg-accent-soft")}
            >
              <Mark on={on} multi={q.multi} />
              <span className="min-w-0 flex-1 [overflow-wrap:anywhere]">
                <span className="block text-[13px] leading-5 text-fg">
                  <Shown text={o.label} />
                </span>
                {o.description.trim() && (
                  <span className="block text-[12px] leading-snug text-muted">
                    <Shown text={o.description} />
                  </span>
                )}
              </span>
            </button>
          );
        })}
        <div className={cx("flex items-center gap-2.5 px-3 py-1.5", picked.otherOn && "bg-accent-soft")}>
          <button
            type="button"
            role={q.multi ? "checkbox" : "radio"}
            aria-checked={picked.otherOn}
            aria-label="Your own answer"
            disabled={!picked.other.trim()}
            onClick={() => onChange(toggleOther(q, picked))}
            className="shrink-0 rounded-full disabled:cursor-default"
          >
            <Mark on={picked.otherOn} multi={q.multi} />
          </button>
          <input
            data-choice
            type="text"
            value={picked.other}
            onChange={(e) => onChange(type(q, picked, e.target.value))}
            placeholder="Something else: type your own answer"
            aria-label={`Your own answer to: ${q.question}`}
            maxLength={4000}
            className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-faint"
          />
        </div>
      </div>
      {chosen?.preview && (
        <pre className="mt-2 overflow-x-auto rounded-md bg-code p-3 font-mono text-[11.5px] leading-snug text-muted ring-1 ring-inset ring-line" aria-label={`Preview of ${chosen.label}`} data-preview>
          <Shown text={chosen.preview} />
        </pre>
      )}
    </fieldset>
  );
}

/** A radio dot or a checkbox tick, in the app's own style. */
function Mark({ on, multi }: { on: boolean; multi: boolean }) {
  return (
    <span
      aria-hidden
      className={cx(
        "mt-[3px] grid size-3.5 shrink-0 place-items-center ring-1 ring-inset transition-colors",
        multi ? "rounded-[4px]" : "rounded-full",
        on ? "bg-accent text-accent-fg ring-accent" : "bg-panel ring-line-strong",
      )}
    >
      {on && (multi ? <Check className="size-2.5" strokeWidth={3} /> : <span className="size-1.5 rounded-full bg-accent-fg" />)}
    </span>
  );
}

/** A question after it was answered: what was asked and what went back to Claude. */
export function Answered({ questions, decision, detail }: { questions: Question[] | null; decision: string | null; detail: DecisionDetail | null | undefined }) {
  const answers = detail?.answers;
  if (decision === "allow" && answers && Object.keys(answers).length > 0) {
    // In the order Claude asked; anything else that was answered comes after.
    const asked = (questions ?? []).filter((q) => q.question in answers);
    const rest = Object.keys(answers).filter((k) => !asked.some((q) => q.question === k));
    const rows = [...asked.map((q) => ({ name: q.header.trim() || q.question, title: q.question, answer: answers[q.question] })), ...rest.map((k) => ({ name: k, title: k, answer: answers[k] }))];
    return (
      <div data-answered>
        <span className="text-faint">You answered</span>
        <dl className="mt-1 space-y-0.5">
          {rows.map((r, i) => (
            <div key={i} className="flex gap-2 text-[12.5px]" title={r.title}>
              <dt className="max-w-[45%] shrink-0 truncate text-muted">
                <Shown text={r.name} />
              </dt>
              <dd className="min-w-0 [overflow-wrap:anywhere] text-fg">
                <Shown text={r.answer} />
              </dd>
            </div>
          ))}
        </dl>
      </div>
    );
  }
  const count = questions?.length ?? 1;
  const what = count === 1 ? "Claude's question" : `Claude's ${count} questions`;
  const label = detail?.clarify ? `You chose to talk about ${what} first` : decision === "deny" ? `You did not answer ${what}` : decision === "cancelled" ? `${what} was withdrawn` : decision === "allow" ? `${what}: sent on with no answer` : `${what} expired`;
  return <span className="text-faint">{label.replace("questions was", "questions were")}</span>;
}
