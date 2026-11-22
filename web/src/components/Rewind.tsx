import { RotateCcw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { RewindPreview, Session } from "../api";
import { api } from "../api";
import { ago } from "../format";
import { canRestoreCode, choiceLines, editedLine, RESTORE_WARNING, restoreProblems, rewindChoices, rewindPoints, type RewindChoice, type RewindPoint } from "../rewind";
import { visible } from "../safe";
import { useStore } from "../store";
import { toRows } from "../transcript";
import { useNow, useUI } from "../ui";
import { Button, cx, Kbd, useDialog } from "./ui";

/** The first line of a message, with invisible characters written out. */
const firstLine = (text: string) => visible(text.trim().split("\n")[0]).slice(0, 200);

/**
 * Claude Code's rewind, as its terminal lays it out (/rewind, or Esc twice): a list of your
 * messages, oldest first, ending in "(current)"; pick one and it works out what restoring the
 * code would change, then offers to restore the code, the conversation, or both. The words are
 * Claude Code's. Its two "Summarize" choices are not here: of all the requests Claude Code takes
 * (2.1.329), none summarizes part of a conversation; they are done inside its terminal.
 *
 * `at` opens the dialog on one message, past the list.
 */
export function Rewind({ session, at, onClose }: { session: Session; at: number | null; onClose: () => void }) {
  const events = useStore((s) => s.events[session.id]);
  const points = useMemo(() => rewindPoints(toRows(events ?? []), session.cwd), [events, session.cwd]);
  const now = useNow();
  const [picked, setPicked] = useState<RewindPoint | null>(null);
  // What restoring the code would change: null while Claude Code is being asked.
  const [preview, setPreview] = useState<RewindPreview | null>(null);
  const [asking, setAsking] = useState(false);
  const [focus, setFocus] = useState(-1);
  const [busy, setBusy] = useState(false);
  const [problems, setProblems] = useState<string[]>([]);
  const box = useRef<HTMLDivElement>(null);
  const asked = useRef(0);

  const back = () => {
    if (busy) return;
    // From the choices, back to the list; from the list (or a dialog opened on one message), out.
    if (picked && at === null && problems.length === 0) {
      asked.current++;
      setPicked(null);
      setPreview(null);
      setAsking(false);
      setFocus(-1);
    } else onClose();
  };
  useDialog(back);

  const pick = (point: RewindPoint) => {
    if (!point.canRewind) return;
    setPicked(point);
    setPreview(null);
    setAsking(true);
    setFocus(-1);
    const mine = ++asked.current;
    api
      .rewindPreview(session.id, point.seq)
      .catch((e): RewindPreview => ({ can_restore: false, files: [], insertions: 0, deletions: 0, error: e instanceof Error ? e.message : String(e) }))
      .then((p) => {
        if (asked.current !== mine) return;
        setPreview(p);
        setAsking(false);
      });
  };
  // Opened on one message: straight to its choices.
  const opened = useRef(false);
  useEffect(() => {
    if (opened.current || at === null || events === undefined) return;
    opened.current = true;
    const point = points.find((p) => p.seq === at);
    if (point?.canRewind) pick(point);
    else onClose();
  });
  useEffect(() => box.current?.focus(), [picked, asking]);

  const choices = rewindChoices(preview);
  // Claude Code starts on "both" when code can be restored, else on "conversation".
  const chosen: RewindChoice = focus >= 0 && focus < choices.length ? choices[focus].value : canRestoreCode(preview) ? "both" : "conversation";
  const chosenAt = choices.findIndex((c) => c.value === chosen);
  // The list ends in "(current)", where it starts.
  const listAt = focus >= 0 && focus <= points.length ? focus : points.length;

  const run = async (choice: RewindChoice) => {
    if (!picked || busy) return;
    if (choice === "nevermind") return back();
    setBusy(true);
    try {
      const done = await api.rewind(session.id, picked.seq, choice);
      // The message taken back goes into the message box, to be changed and sent again.
      if (done.conversation?.ok) useUI.getState().setPrefill({ session: session.id, text: done.conversation.prefill ?? picked.text });
      const wrong = restoreProblems(done);
      if (wrong.length === 0) return onClose();
      setProblems(wrong);
    } catch (e) {
      setProblems([e instanceof Error ? e.message : String(e)]);
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (busy || problems.length > 0) return;
    // A key held down does not confirm what it has not seen.
    if (e.repeat && (e.key === "Enter" || /^[0-9]$/.test(e.key))) return e.preventDefault();
    // Enter on a button reached with Tab does that button, not the highlighted line.
    const target = e.target as HTMLElement;
    if (e.key === "Enter" && target.dataset?.rewindChoice) {
      e.preventDefault();
      if (!asking) run(target.dataset.rewindChoice as RewindChoice);
      return;
    }
    if (e.key === "Enter" && target.dataset?.rewindPoint) {
      e.preventDefault();
      const point = points.find((p) => String(p.seq) === target.dataset.rewindPoint);
      if (point) pick(point);
      return;
    }
    if (e.key === "Enter" && target.dataset?.rewindCurrent !== undefined) {
      e.preventDefault();
      onClose();
      return;
    }
    const count = picked ? choices.length : points.length + 1;
    const at = picked ? chosenAt : listAt;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (picked && asking) return;
      // Skips messages that cannot be gone back to.
      let next = at;
      for (let i = 0; i < count; i++) {
        next = (next + (e.key === "ArrowDown" ? 1 : count - 1)) % count;
        if (picked || next === points.length || points[next].canRewind) break;
      }
      setFocus(next);
    } else if (picked && !asking && /^[1-9]$/.test(e.key) && Number(e.key) <= choices.length) {
      // A number chooses that line, as in Claude Code's lists.
      e.preventDefault();
      run(choices[Number(e.key) - 1].value);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!picked) listAt === points.length ? onClose() : pick(points[listAt]);
      else if (!asking) run(chosen);
    }
  };

  // Restoring the conversation stops agents still at work: Claude Code will not take it back while one runs.
  const agentsRunning = (session.agents ?? []).length > 0;
  const [conversationLine, codeLine] = choiceLines(chosen, preview);
  return (
    <div className="animate-fade fixed inset-0 z-50 flex items-start justify-center bg-black/40 px-4 pt-[14vh]" onMouseDown={(e) => e.target === e.currentTarget && back()}>
      <div
        ref={box}
        role="dialog"
        aria-modal="true"
        aria-labelledby="rewind-title"
        tabIndex={-1}
        onKeyDown={onKey}
        data-rewind={picked ? (problems.length > 0 ? "problems" : asking ? "asking" : "choices") : "list"}
        className="animate-pop flex max-h-[72vh] w-full max-w-[560px] flex-col rounded-xl bg-panel p-5 shadow-pop outline-none"
      >
        <h2 id="rewind-title" className="flex items-center gap-2 text-[15px] font-semibold">
          <RotateCcw className="size-4 text-muted" /> Rewind
        </h2>

        {!picked && (
          <>
            <p className="mt-1.5 text-[13px] text-muted">Restore the code and/or conversation to the point before…</p>
            <div role="listbox" aria-label="Your messages" className="mt-3 min-h-0 flex-1 space-y-0.5 overflow-y-auto">
              {points.map((p, i) => (
                <button
                  key={p.seq}
                  role="option"
                  aria-selected={i === listAt}
                  aria-disabled={!p.canRewind}
                  data-rewind-point={p.seq}
                  onMouseEnter={() => p.canRewind && setFocus(i)}
                  onFocus={() => p.canRewind && setFocus(i)}
                  onClick={() => pick(p)}
                  className={cx("block w-full rounded-md px-3 py-2 text-left", i === listAt ? "bg-accent-soft" : "hover:bg-raised", !p.canRewind && "cursor-default opacity-50")}
                >
                  <span className="block truncate text-[13px] text-fg">{firstLine(p.text)}</span>
                  <span className="block truncate text-[12px] text-faint">{editedLine(p)}</span>
                </button>
              ))}
              <button
                role="option"
                aria-selected={listAt === points.length}
                data-rewind-current
                onMouseEnter={() => setFocus(points.length)}
                onFocus={() => setFocus(points.length)}
                onClick={onClose}
                className={cx("block w-full rounded-md px-3 py-2 text-left text-[13px] text-muted", listAt === points.length ? "bg-accent-soft" : "hover:bg-raised")}
              >
                (current)
              </button>
            </div>
            {points.length === 0 && <p className="mt-2 text-[12.5px] text-faint">Nothing to rewind to yet.</p>}
            <p className="mt-3 flex items-center gap-1.5 text-[11.5px] text-faint">
              <Kbd>↑</Kbd>
              <Kbd>↓</Kbd> choose <Kbd>Enter</Kbd> pick <Kbd>Esc</Kbd> close
            </p>
          </>
        )}

        {picked && (
          <>
            <p className="mt-1.5 text-[13px] text-muted">Restore {!canRestoreCode(preview) && "the conversation "}to the point before you sent this message:</p>
            <div className="mt-2 max-h-28 overflow-y-auto border-l-2 border-line-strong pl-3">
              <p className="text-[13px] [overflow-wrap:anywhere] whitespace-pre-wrap text-fg" data-rewind-message>
                {visible(picked.text).slice(0, 2000)}
              </p>
              <p className="text-[12px] text-faint">({ago(picked.ts, now) === "now" ? "just now" : `${ago(picked.ts, now)} ago`})</p>
            </div>

            {asking && <p className="mt-4 text-[12.5px] text-faint">Asking Claude Code what restoring the code would change…</p>}

            {!asking && problems.length === 0 && (
              <>
                <div role="listbox" aria-label="What to restore" className="mt-4 space-y-0.5">
                  {choices.map((c, i) => (
                    <button
                      key={c.value}
                      role="option"
                      aria-selected={i === chosenAt}
                      data-rewind-choice={c.value}
                      disabled={busy}
                      onMouseEnter={() => setFocus(i)}
                      onFocus={() => setFocus(i)}
                      onClick={() => run(c.value)}
                      className={cx("flex w-full items-center gap-2.5 rounded-md px-3 py-1.5 text-left text-[13px]", i === chosenAt ? "bg-accent-soft text-fg" : "text-muted hover:bg-raised", busy && "opacity-60")}
                    >
                      <span className="w-3 shrink-0 text-faint tabular-nums">{i + 1}</span>
                      {c.label}
                    </button>
                  ))}
                </div>
                <div className="mt-3 space-y-0.5 text-[12.5px] text-muted" data-rewind-lines>
                  <p>{conversationLine}</p>
                  <p>{codeLine}</p>
                </div>
                {canRestoreCode(preview) && <p className="mt-2 text-[12px] text-s-needs">{RESTORE_WARNING}</p>}
                {preview && !preview.can_restore && preview.error && (
                  <p className="mt-2 text-[12px] [overflow-wrap:anywhere] text-faint" data-rewind-no-code>
                    The code cannot be restored: {visible(preview.error).slice(0, 300)}
                  </p>
                )}
                {chosen !== "nevermind" && (session.in_turn || (agentsRunning && chosen !== "code")) && (
                  <p className="mt-2 text-[12px] text-faint" data-rewind-stops>
                    {session.in_turn ? "The turn that is running is stopped first." : "The agents that are running are stopped first."}
                  </p>
                )}
                <p className="mt-3 flex items-center gap-1.5 text-[11.5px] text-faint">
                  {busy ? (
                    "Restoring…"
                  ) : (
                    <>
                      <Kbd>↑</Kbd>
                      <Kbd>↓</Kbd> choose <Kbd>Enter</Kbd> confirm <Kbd>Esc</Kbd> {at === null ? "back" : "close"}
                    </>
                  )}
                </p>
              </>
            )}

            {problems.length > 0 && (
              <>
                <div className="mt-4 space-y-2" data-rewind-problems>
                  {problems.map((p, i) => (
                    <p key={i} className="rounded-md bg-s-error/10 px-3 py-2 text-[12.5px] [overflow-wrap:anywhere] text-s-error ring-1 ring-inset ring-s-error/30">
                      {visible(p).slice(0, 800)}
                    </p>
                  ))}
                </div>
                <div className="mt-4 flex justify-end">
                  <Button size="sm" autoFocus onClick={onClose}>
                    Close
                  </Button>
                </div>
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
