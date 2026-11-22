import { Check, ChevronLeft, ChevronRight, CircleHelp, MessagesSquare } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, type DecisionDetail, type Session } from "../api";
import { answerOf, answersOf, NOTHING, pick, toggleOther, type, type Picked, type Question } from "../ask";
import { report } from "../store";
import { ChoiceKey, Shown } from "./approval";
import { Button, cx, Kbd } from "./ui";

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/**
 * One row of the list of choices. Every row is at least as tall as a choice with one line of
 * explanation under it, so the list reads as equal rows, the own-answer row included.
 */
const ROW = "flex min-h-12 items-start gap-2.5 rounded-md px-2 py-1.5";

/**
 * Claude's questions, one at a time, in a panel that sits on top of the message box.
 *
 * Choosing an option answers the question and moves on to the next one still open; with none
 * left, the answers go to Claude. Questions that take several choices, or an answer of the
 * user's own, move on with Enter or the button. The dots and arrows go to any question in any
 * order, and an earlier answer can be changed until the last one is given. The answers go back
 * exactly as Claude Code's terminal sends them: one per question, several choices joined ", ".
 */
export function QuestionPanel({ session, requestId, questions }: { session: Session; requestId: string; questions: Question[] }) {
  const [picked, setPicked] = useState<Picked[]>(() => questions.map(() => NOTHING));
  const [at, setAt] = useState(0);
  const [busy, setBusy] = useState(false);
  // The option under the pointer or the keyboard focus: its preview shows before it is chosen.
  const [peek, setPeek] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const own = useRef<HTMLInputElement>(null);
  const alive = useRef(true);
  useEffect(() => () => void (alive.current = false), []);

  const q = questions[at];
  const p = picked[at];
  const open = (all: Picked[]) => questions.map((qq, i) => answerOf(qq, all[i]) === null);
  const left = open(picked).filter(Boolean).length;
  const send = async (behavior: "allow" | "deny", extra: Parameters<typeof api.decide>[3]) => {
    setBusy(true);
    try {
      await api.decide(session.id, requestId, behavior, extra);
    } catch (e) {
      report(e);
      if (alive.current) setBusy(false);
    }
  };
  /** After an answer: on to the next question still open or, with none left, off to Claude. */
  const onward = (all: Picked[]) => {
    const still = open(all);
    for (let step = 1; step <= questions.length; step++) {
      const next = (at + step) % questions.length;
      if (still[next]) return go(next);
    }
    const answers = answersOf(questions, all);
    if (answers && !busy) send("allow", { answers });
  };
  // The keys keep working on the question that is now showing: the focus moves to it as it is
  // drawn, before anything else can be pressed. (A frame later, a key pressed in between went
  // nowhere: the row that had the focus was gone.)
  const moved = useRef(false);
  useLayoutEffect(() => {
    if (!moved.current) return;
    moved.current = false;
    root.current?.querySelector<HTMLElement>("[data-choice]")?.focus();
  }, [at]);
  const go = (i: number) => {
    setPeek(null);
    if (i === at) return root.current?.querySelector<HTMLElement>("[data-choice]")?.focus();
    moved.current = true;
    setAt(i);
  };
  const set = (next: Picked) => {
    const all = picked.map((old, i) => (i === at ? next : old));
    setPicked(all);
    return all;
  };
  const choose = (label: string) => {
    if (busy) return;
    const all = set(pick(q, p, label));
    // One choice answers the question. The mark shows for a moment before the panel moves on.
    if (!q.multi) setTimeout(() => alive.current && onward(all), 160);
  };
  const answeredHere = answerOf(q, p) !== null;

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const typing = e.target === own.current;
    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      // Enter moves on once the question has an answer. Before that, on an option, it chooses
      // it (as Space does at any time).
      e.preventDefault();
      if (answeredHere) onward(picked);
      else if ((e.target as HTMLElement).hasAttribute("data-option")) (e.target as HTMLElement).click();
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const rows = Array.from(root.current?.querySelectorAll<HTMLElement>("[data-choice]") ?? []);
      const from = rows.indexOf(document.activeElement as HTMLElement);
      if (rows.length === 0) return;
      e.preventDefault();
      rows[from < 0 ? 0 : (from + (e.key === "ArrowDown" ? 1 : rows.length - 1)) % rows.length].focus();
      return;
    }
    if (typing) return;
    if ((e.key === "ArrowRight" || e.key === "ArrowLeft") && questions.length > 1) {
      e.preventDefault();
      go((at + (e.key === "ArrowRight" ? 1 : questions.length - 1)) % questions.length);
      return;
    }
    const letter = e.key.length === 1 ? LETTERS.indexOf(e.key.toUpperCase()) : -1;
    if (letter >= 0 && letter < q.options.length) {
      e.preventDefault();
      choose(q.options[letter].label);
    } else if (letter === q.options.length) {
      e.preventDefault();
      own.current?.focus();
    }
  };

  // Options with previews (mock-ups, snippets) show them beside the list, as Claude Code does:
  // the one under the pointer or focus, else the chosen one, else the first. Every preview is
  // laid out in the same cell and only one is visible, so the panel keeps its size and no
  // option moves away from the pointer when the preview changes.
  const previews = q.options.filter((o) => o.preview);
  const showing = previews.find((o) => o.label === peek) ?? previews.find((o) => p.labels.includes(o.label)) ?? previews[0];
  return (
    <div
      ref={root}
      role="group"
      aria-label={questions.length === 1 ? "Claude's question" : `Claude's questions: ${at + 1} of ${questions.length}`}
      data-question-panel
      onKeyDown={onKey}
      className="animate-rise mb-1.5 overflow-hidden rounded-lg bg-panel shadow-pop"
    >
      <div className="flex items-center gap-2 border-b border-line px-3.5 py-2 text-[12.5px]">
        <CircleHelp className="size-3.5 shrink-0 text-s-needs" />
        <span className="font-medium">{questions.length === 1 ? "Claude has a question" : "Claude has questions"}</span>
        {questions.length > 1 && (
          <span className="ml-auto flex items-center gap-2">
            <span className="flex items-center gap-1" role="tablist" aria-label="Questions">
              {questions.map((qq, i) => {
                const done = answerOf(qq, picked[i]) !== null;
                return (
                  <button
                    key={i}
                    role="tab"
                    aria-selected={i === at}
                    aria-label={`Question ${i + 1}${done ? ", answered" : ""}`}
                    title={qq.question}
                    onClick={() => go(i)}
                    className="grid size-4 place-items-center rounded-full"
                  >
                    <span className={cx("rounded-full transition-all", i === at ? "size-2.5" : "size-1.5", done ? "bg-accent" : i === at ? "bg-fg" : "bg-faint/60")} />
                  </button>
                );
              })}
            </span>
            <span className="text-faint tabular-nums" aria-live="polite">
              {at + 1} of {questions.length}
            </span>
            <span className="flex">
              <PagerButton label="Previous question" onClick={() => go((at + questions.length - 1) % questions.length)}>
                <ChevronLeft className="size-3.5" />
              </PagerButton>
              <PagerButton label="Next question" onClick={() => go((at + 1) % questions.length)}>
                <ChevronRight className="size-3.5" />
              </PagerButton>
            </span>
          </span>
        )}
      </div>

      <div className="max-h-[min(48vh,440px)] overflow-y-auto px-3.5 pt-3 pb-1">
        {/* Claude also sends a short tag with each question ("Sign-in"). It says nothing the
            question does not, so only the question shows. */}
        <p className="mb-2 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-[13.5px]">
          <span className="min-w-0 font-medium [overflow-wrap:anywhere]">
            <Shown text={q.question} />
          </span>
          {q.multi && <span className="text-[11.5px] text-faint">Choose any that apply</span>}
        </p>
        <div className={cx(previews.length > 0 && "grid items-start gap-3 sm:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]")}>
          <fieldset role={q.multi ? "group" : "radiogroup"} aria-label={q.question} disabled={busy} className="min-w-0 space-y-0.5">
            {q.options.map((o, i) => {
              const on = p.labels.includes(o.label);
              return (
                <button
                  key={o.label}
                  type="button"
                  role={q.multi ? "checkbox" : "radio"}
                  aria-checked={on}
                  data-choice
                  data-option
                  onClick={() => choose(o.label)}
                  onMouseEnter={() => setPeek(o.label)}
                  onMouseLeave={() => setPeek(null)}
                  onFocus={() => setPeek(o.label)}
                  className={cx(ROW, "w-full text-left transition-colors", on ? "bg-accent-soft" : "hover:bg-panel-2")}
                >
                  <ChoiceKey on={on}>{LETTERS[i]}</ChoiceKey>
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
            <div className={cx(ROW, p.otherOn ? "bg-accent-soft" : "focus-within:bg-panel-2")}>
              <button
                type="button"
                role={q.multi ? "checkbox" : "radio"}
                aria-checked={p.otherOn}
                aria-label="Your own answer"
                tabIndex={-1}
                onClick={() => (p.other.trim() ? set(toggleOther(q, p)) : own.current?.focus())}
                className="shrink-0 rounded"
              >
                <ChoiceKey on={p.otherOn}>{LETTERS[q.options.length]}</ChoiceKey>
              </button>
              <label className="min-w-0 flex-1 cursor-text">
                <input
                  ref={own}
                  data-choice
                  type="text"
                  value={p.other}
                  onChange={(e) => set(type(q, p, e.target.value))}
                  placeholder="Something else…"
                  aria-label={`Your own answer to: ${q.question}`}
                  maxLength={4000}
                  className="block h-5 w-full bg-transparent text-[13px] leading-5 outline-none placeholder:text-fg"
                />
                <span className="block text-[12px] leading-snug text-muted">Type your own answer</span>
              </label>
            </div>
          </fieldset>
          {previews.length > 0 && (
            <div className="mb-1 grid min-w-0" data-preview>
              {previews.map((o) => (
                <pre
                  key={o.label}
                  aria-hidden={o !== showing}
                  className={cx("col-start-1 row-start-1 min-w-0 overflow-x-auto rounded-md bg-code p-3 font-mono text-[11.5px] leading-snug text-muted ring-1 ring-inset ring-line", o !== showing && "invisible")}
                >
                  <Shown text={o.preview ?? ""} />
                </pre>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2 px-3.5 py-2.5">
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => send("deny", { clarify: true })} title="Answer nothing yet: Claude asks what you would like to clear up first">
          <MessagesSquare className="size-3.5" /> Chat about this
        </Button>
        <span className="ml-auto hidden items-center gap-1 text-[11px] text-faint sm:flex">
          <Kbd>A</Kbd>–<Kbd>{LETTERS[q.options.length]}</Kbd> choose
          {questions.length > 1 && (
            <>
              <span className="mx-1">·</span>
              <Kbd>←</Kbd>
              <Kbd>→</Kbd> other questions
            </>
          )}
        </span>
        <Button variant="primary" size="sm" className="ml-auto sm:ml-0" disabled={!answeredHere || busy} onClick={() => onward(picked)} data-onward>
          {left === 0 ? (
            <>
              <Check className="size-3.5" /> {questions.length === 1 ? "Send answer" : "Send answers"}
            </>
          ) : (
            "Next"
          )}
          <Kbd className="ml-0.5 border-accent-fg/25 bg-accent-fg/10 text-accent-fg/85">⏎</Kbd>
        </Button>
      </div>
    </div>
  );
}

function PagerButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button type="button" aria-label={label} title={label} onClick={onClick} className="grid size-6 place-items-center rounded text-muted hover:bg-panel-2 hover:text-fg">
      {children}
    </button>
  );
}

/** In the feed, while a question waits: where to answer it. */
export function Asking({ questions }: { questions: Question[] }) {
  return (
    <span data-asking>
      <span className="font-medium text-s-needs">{questions.length === 1 ? "Claude is asking a question." : `Claude is asking ${questions.length} questions.`}</span>{" "}
      <span className="text-faint">Answer {questions.length === 1 ? "it" : "them"} in the panel above the message box.</span>
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
    const rows = [...asked.map((q) => q.question), ...rest].map((question) => ({ question, answer: answers[question] }));
    return (
      <div data-answered>
        <span className="text-faint">You answered</span>
        <dl className="mt-1.5 space-y-2 border-l border-line-strong pl-3 text-[12.5px] [overflow-wrap:anywhere]">
          {rows.map((r, i) => (
            <div key={i}>
              <dt className="text-muted">
                <Shown text={r.question} />
              </dt>
              <dd className="text-fg">
                <Shown text={r.answer} />
              </dd>
            </div>
          ))}
        </dl>
      </div>
    );
  }
  const many = (questions?.length ?? 1) > 1;
  const what = many ? `Claude's ${questions!.length} questions` : "Claude's question";
  const label = detail?.clarify
    ? `You chose to talk about ${what} first`
    : decision === "deny"
      ? `You did not answer ${what}`
      : decision === "cancelled"
        ? `${what} ${many ? "were" : "was"} withdrawn`
        : decision === "allow"
          ? `${what}: sent on with no answer`
          : `${what} expired`;
  return <span className="text-faint">{label}</span>;
}
