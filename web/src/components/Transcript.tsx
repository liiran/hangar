import {
  AlertTriangle,
  Bot,
  Brain,
  Check,
  ChevronRight,
  CircleHelp,
  ClipboardList,
  FileText,
  Globe,
  Hammer,
  Search,
  Shield,
  ShieldAlert,
  Sparkles,
  Square,
  SquareTerminal,
  User,
  X,
} from "lucide-react";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Session, SessionEvent } from "../api";
import { api } from "../api";
import { ASK_TOOL, ENTER_PLAN_TOOL, PLAN_TOOL, planOf, questionsOf } from "../ask";
import { diffOf } from "../diff";
import { ago, duration, usd } from "../format";
import { report } from "../store";
import { useNow } from "../ui";
import { rightToLeft, sessionRules, visible } from "../safe";
import { AGENT_TOOLS, agentLines, agentUsage, needsSignIn, PAGE, readSearchSummary, relative, rowState, toRows, toolSummary, type Row } from "../transcript";
import { ApprovalBox, countLines, OtherFields, Shown } from "./approval";
import { DiffStat, DiffView } from "./Diff";
import { Markdown } from "./Markdown";
import { PlanCard, PlanOutcome } from "./Plan";
import { Answered, Asking } from "./Question";
import { Button, cx, Guard } from "./ui";

/** Statuses in which a tool call without a result is still running. */
const LIVE = new Set(["working", "starting", "needs_you"]);

type ToolRowData = Extract<Row, { kind: "tool" }>;

/** Whether the helper agent an Agent call started is running now. The daemon says which are. */
const agentRunning = (s: Session, row: ToolRowData) => (s.agents ?? []).some((a) => a.tool_use_id === row.id || (row.agent?.id !== undefined && a.id === row.agent.id));

/** The agents running now, as one string: an Agent row is drawn again when it changes. */
const agentsKey = (s: Session) => (s.agents ?? []).map((a) => a.id).join(" ");

const TOOL_ICONS: Record<string, typeof Hammer> = {
  Bash: SquareTerminal,
  Read: FileText,
  Write: FileText,
  Edit: FileText,
  MultiEdit: FileText,
  Grep: Search,
  Glob: Search,
  WebFetch: Globe,
  WebSearch: Globe,
  [ASK_TOOL]: CircleHelp,
  [PLAN_TOOL]: ClipboardList,
  [ENTER_PLAN_TOOL]: ClipboardList,
  Agent: Bot,
  Task: Bot,
};

/** Claude Code's dialogs and its switch into plan mode, by what they are instead of by their tool names. */
const TOOL_NAMES: Record<string, string> = { [ASK_TOOL]: "Question", [PLAN_TOOL]: "Plan", [ENTER_PLAN_TOOL]: "Entered plan mode" };

export function Transcript({ session, events, header }: { session: Session; events: SessionEvent[] | undefined; header?: ReactNode }) {
  // Each new event rebuilds the rows. A row that is the same as before keeps its identity, so
  // only what changed is drawn again.
  const kept = useRef(new Map<string, { state: string; row: Row }>());
  const rows = useMemo(() => {
    const next = new Map<string, { state: string; row: Row }>();
    const out = toRows(events ?? []).map((r) => {
      const state = rowState(r);
      const old = kept.current.get(r.key);
      const entry = old && old.state === state ? old : { state, row: r };
      next.set(r.key, entry);
      return entry.row;
    });
    kept.current = next;
    return out;
  }, [events]);
  // A long session is drawn from its newest entries: thousands of them make every keystroke and
  // every new message slow. Where the drawn part starts is settled when the session opens. It
  // moves up on request and never down, so nothing on screen goes away while you read.
  const opened = useRef<number | null>(null);
  if (opened.current === null && events !== undefined) opened.current = Math.max(0, rows.length - PAGE);
  const [earlier, setEarlier] = useState(0);
  const from = Math.max(0, (opened.current ?? 0) - earlier);
  const drawn = from > 0 ? rows.slice(from) : rows;
  const lastError = useMemo(() => {
    for (let i = rows.length - 1; i >= 0; i--) {
      const r = rows[i];
      if (r.kind === "error") return r.message;
    }
    return undefined;
  }, [rows]);
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // Follow new output only while the user is at the bottom.
  const onScroll = () => {
    const el = scroller.current;
    if (el) pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [rows.length, session.status]);
  useEffect(() => {
    pinned.current = true;
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [session.id]);
  // Earlier entries are added above what is on screen. The view moves down by as much as was
  // added, so that the entry being read stays where it is.
  const before = useRef<{ height: number; top: number } | null>(null);
  const showEarlier = (count: number) => {
    const el = scroller.current;
    if (el) before.current = { height: el.scrollHeight, top: el.scrollTop };
    setEarlier((n) => Math.min(Number.MAX_SAFE_INTEGER, n + count));
  };
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && before.current) el.scrollTop = before.current.top + (el.scrollHeight - before.current.height);
    before.current = null;
  }, [earlier]);

  return (
    <div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto max-w-[760px] px-4 pt-8 pb-6 sm:px-8">
        {header}
        <div className="mb-4 flex items-baseline justify-between border-b border-line pb-2">
          <h3 className="text-[13px] font-medium">Activity</h3>
          {session.turns > 0 && (
            <span className="text-[12px] text-faint">
              {session.turns} {session.turns === 1 ? "turn" : "turns"} · {usd(session.cost_usd)}
            </span>
          )}
        </div>
        <div className="space-y-3">
          {events === undefined && <p className="text-[12.5px] text-faint">Loading…</p>}
          {from > 0 && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-dashed border-line-strong px-4 py-2.5 text-[12.5px] text-muted">
              <span className="mr-auto" title="A long session is drawn from its newest entries, so that typing and new messages stay fast.">
                {from.toLocaleString()} earlier {from === 1 ? "entry is" : "entries are"} not drawn.
              </span>
              <span className="flex items-center gap-2">
                <Button size="sm" onClick={() => showEarlier(PAGE)}>
                  Show {Math.min(PAGE, from)} more
                </Button>
                {from > PAGE && (
                  <Button size="sm" variant="ghost" onClick={() => showEarlier(Number.MAX_SAFE_INTEGER)}>
                    Show all
                  </Button>
                )}
              </span>
            </div>
          )}
          {drawn.map((r, i) => (
            // Entries older than the newest page are skipped by the browser while they are off screen.
            <div key={r.key} className={i < drawn.length - PAGE ? "[contain-intrinsic-size:auto_72px] [content-visibility:auto]" : undefined}>
              <Guard fallback={<Unshown row={r} />}>
                <DrawnRow row={r} session={session} />
              </Guard>
            </div>
          ))}
          <RunningAgents session={session} rows={rows} />
          {(session.status === "working" || session.status === "starting") && (
            <Activity icon={<Dots />}>
              <span className="text-muted" data-working>
                {session.status === "starting" ? "Starting Claude…" : waitingOnAgents(session)}
              </span>
            </Activity>
          )}
          {session.status === "queued" && (
            <div className="rounded-lg border border-dashed border-line-strong px-4 py-3 text-[12.5px] text-muted">
              <span className="font-medium text-fg">Waiting for memory{session.queue_position != null && ` (#${session.queue_position} in line)`}.</span>{" "}
              {session.status_detail ?? "This session starts as soon as there is room."}
            </div>
          )}
          {session.status === "hibernated" && <p className="pt-2 text-center text-[12px] text-faint">Paused to free memory. Send a message to pick up where it left off.</p>}
          {(session.status === "error" || session.status === "stopped") && <Stopped session={session} shownError={lastError} />}
        </div>
      </div>
    </div>
  );
}

/**
 * What the line at the foot of the feed says while the session works. Claude's own turn can be
 * over with helper agents still at work in the background: then it says so.
 */
function waitingOnAgents(session: Session): string {
  const running = (session.agents ?? []).length;
  if (running === 0 || session.in_turn !== false) return "Working…";
  return running === 1 ? "1 agent running in the background…" : `${running} agents running in the background…`;
}

/** Every Agent call in the feed, also one made by an agent. */
function agentRows(rows: Row[], out: ToolRowData[] = []): ToolRowData[] {
  for (const r of rows) {
    if (r.kind !== "tool" || !AGENT_TOOLS.has(r.name)) continue;
    out.push(r);
    if (r.agent) agentRows(r.agent.steps, out);
  }
  return out;
}

/**
 * The agents at work in the background, at the foot of the feed: what each is doing right now.
 * Their rows are where Claude started them, which the conversation soon scrolls away from.
 * A click goes to the row.
 */
function RunningAgents({ session, rows }: { session: Session; rows: Row[] }) {
  const background = (session.agents ?? []).filter((a) => a.background);
  const found = useMemo(() => (background.length > 0 ? agentRows(rows) : []), [rows, background.length]);
  if (background.length === 0) return null;
  return (
    <div data-running-agents className="space-y-1 rounded-lg border border-line px-3 py-2 text-[12px]">
      {background.map((a) => {
        const row = found.find((r) => r.id === a.tool_use_id || r.agent?.id === a.id);
        const { lines } = agentLines(row?.agent ?? { background: true, steps: [], rev: 0 }, true);
        const newest = lines[lines.length - 1];
        const doing = !newest
          ? "Initializing…"
          : newest.kind === "summary"
            ? readSearchSummary(newest.searches, newest.reads, newest.active)
            : newest.kind === "text"
              ? visible(newest.text.trim().split("\n")[0]).slice(0, 200)
              : `${visible(newest.row.name.replace(/^mcp__/, "").replace(/__/g, " · "))} ${toolSummary(newest.row.name, newest.row.input, session.cwd)}`;
        const used = agentUsage(row?.agent, false);
        return (
          <button
            key={a.id}
            data-running-agent={a.id}
            disabled={!row}
            onClick={() => row && document.querySelector(`[data-agent="${CSS.escape(row.id)}"]`)?.scrollIntoView({ block: "center", behavior: "smooth" })}
            title="Show this agent in the feed"
            className="flex w-full min-w-0 items-center gap-2 text-left text-muted enabled:hover:text-fg"
          >
            <span className="size-1.5 shrink-0 animate-soft-pulse rounded-full bg-s-working" />
            <Bot className="size-3 shrink-0" />
            <span className="shrink-0 font-medium text-fg">{visible(a.description) || "Agent"}</span>
            <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-faint">{doing}</span>
            {used && <span className="shrink-0 text-[11px] text-faint">{used}</span>}
          </button>
        );
      })}
    </div>
  );
}

/** Stands in for an entry that could not be drawn, with its raw data. */
function Unshown({ row }: { row: Row }) {
  let raw = "";
  try {
    raw = visible(JSON.stringify(row, null, 2) ?? "");
  } catch {
    raw = "(no data)";
  }
  return (
    <Activity icon={<AlertTriangle className="size-3" />} tone="error">
      <span className="text-muted">This entry is in a form hangar does not know. Its raw data:</span>
      <pre className="mt-2 max-h-72 overflow-auto rounded-md bg-code p-2.5 font-mono text-[11.5px] [overflow-wrap:anywhere] whitespace-pre-wrap text-muted ring-1 ring-inset ring-line">{raw}</pre>
    </Activity>
  );
}

/** How a stopped session can continue, and why it stopped if no row said so. */
function Stopped({ session, shownError }: { session: Session; shownError: string | undefined }) {
  const detail = session.status_detail && session.status_detail !== shownError ? session.status_detail : null;
  const what = session.status === "error" ? "Stopped with an error" : "The process is stopped";
  return (
    <p className="pt-2 text-center text-[12px] text-faint">
      {what}
      {detail ? `: ${detail}` : ""}. Send a message to {session.claude_session_id ? "resume where it left off" : "start it again"}.
    </p>
  );
}

/** A timeline entry: a small icon in the gutter and a line of text. */
function Activity({ icon, children, tone }: { icon: ReactNode; children: ReactNode; tone?: "error" }) {
  return (
    <div className="flex gap-3 text-[12.5px]">
      <span
        className={cx(
          "mt-px grid size-5 shrink-0 place-items-center rounded-full bg-panel ring-1 ring-inset",
          tone === "error" ? "text-s-error ring-s-error/30" : "text-muted ring-line-strong",
        )}
      >
        {icon}
      </span>
      <div className="min-w-0 flex-1 pt-0.5">{children}</div>
    </div>
  );
}

function Avatar({ claude }: { claude?: boolean }) {
  return (
    <span className={cx("grid size-5 shrink-0 place-items-center rounded-full", claude ? "bg-accent-soft text-accent" : "bg-raised text-muted")}>
      {claude ? <Sparkles className="size-3" /> : <User className="size-3" />}
    </span>
  );
}

function Dots() {
  return (
    <span className="flex gap-[2px]">
      {[0, 1, 2].map((i) => (
        <span key={i} className="size-[3px] animate-soft-pulse rounded-full bg-current" style={{ animationDelay: `${i * 0.2}s` }} />
      ))}
    </span>
  );
}

/**
 * A row, drawn again only when it changed. Of the session, a tool row uses its folder and
 * whether it is running; a permission row uses all of it; the other rows use nothing.
 */
const DrawnRow = memo(
  function DrawnRow(p: { row: Row; session: Session }) {
    return <RowView row={p.row} session={p.session} />;
  },
  (a, b) => {
    if (a.row !== b.row) return false;
    if (a.row.kind === "permission") return a.session === b.session;
    if (a.row.kind === "tool") {
      if (a.session.cwd !== b.session.cwd || LIVE.has(a.session.status) !== LIVE.has(b.session.status)) return false;
      // An Agent row also shows whether its helper runs, and can stop it.
      return !AGENT_TOOLS.has(a.row.name) || (a.session.id === b.session.id && agentsKey(a.session) === agentsKey(b.session));
    }
    return true;
  },
);

function RowView({ row, session }: { row: Row; session: Session }) {
  switch (row.kind) {
    case "user":
      return <UserMessage text={row.text} ts={row.ts} />;
    case "text":
      return (
        <div className="flex gap-3 py-1">
          <Avatar claude />
          <div className="min-w-0 flex-1 text-[13.5px]">
            <Markdown text={row.text} />
            {needsSignIn(row.text) && (
              <p className="mt-1.5 text-[12.5px] text-muted">
                Claude Code is not signed in on this computer. Open a terminal, run <code className="rounded bg-panel-2 px-1 font-mono text-fg">claude</code> and sign in there.
                Then send your message again.
              </p>
            )}
          </div>
        </div>
      );
    case "thinking":
      return <Collapsible icon={<Brain className="size-3" />} label="Thinking" body={row.text} />;
    case "tool":
      return AGENT_TOOLS.has(row.name) ? <AgentRow row={row} session={session} /> : <ToolRow row={row} cwd={session.cwd} live={LIVE.has(session.status)} />;
    case "permission":
      return <PermissionRow row={row} session={session} />;
    case "result":
      if (row.stopped) {
        return (
          <Activity icon={<Square className="size-2.5 fill-current" />}>
            <span className="text-faint">Stopped{row.durationMs != null && ` after ${duration(row.durationMs)}`}</span>
          </Activity>
        );
      }
      return (
        <Activity icon={row.isError ? <AlertTriangle className="size-3" /> : <Check className="size-3" />} tone={row.isError ? "error" : undefined}>
          <span className={row.isError ? "text-s-error" : "text-faint"}>
            {row.isError ? (row.text ?? "The turn failed") : "Done"}
            {row.durationMs != null && ` in ${duration(row.durationMs)}`}
            {row.cost != null && row.cost > 0 && ` · $${row.cost.toFixed(row.cost < 0.1 ? 3 : 2)}`}
          </span>
        </Activity>
      );
    case "error":
      return (
        <Activity icon={<AlertTriangle className="size-3" />} tone="error">
          <span className="text-s-error">{row.message}</span>
        </Activity>
      );
  }
}

function UserMessage({ text, ts }: { text: string; ts: number }) {
  const now = useNow();
  return (
    <div className="rounded-lg bg-panel-2/70 px-4 py-3 ring-1 ring-inset ring-line">
      <div className="mb-1.5 flex items-center gap-2 text-[12px]">
        <Avatar />
        <span className="font-medium">You</span>
        <span className="text-faint" title={new Date(ts).toLocaleString()}>
          {ago(ts, now)}
        </span>
      </div>
      <div className="text-[13.5px] leading-relaxed whitespace-pre-wrap">{text}</div>
    </div>
  );
}

/** Claude's own words about a request, marked as such: they are not hangar's. */
function ClaudeSays({ text }: { text: unknown }) {
  if (typeof text !== "string" || !text.trim()) return null;
  return (
    <p className="mt-1 text-[12.5px] [overflow-wrap:anywhere] text-muted">
      <span className="text-faint">Claude says: </span>
      <Shown text={text} />
    </p>
  );
}

/** Shows what the user is approving in the most readable form for the tool. */
function PermissionPreview({ tool, input, cwd, description, ...box }: { tool: string; input: any; cwd: string; description?: string | null; expanded: boolean; onExpand: () => void; onClipped: (clipped: boolean) => void }) {
  const path = typeof input?.file_path === "string" ? relative(input.file_path, cwd) : null;
  const target = (verb: string) => (
    <p className="mt-1 text-[12.5px] [overflow-wrap:anywhere] text-muted">
      {verb}{" "}
      <span className="font-mono text-fg">
        <Shown text={path ?? ""} />
      </span>
    </p>
  );
  if (tool === "Bash" && typeof input?.command === "string") {
    return (
      <>
        <ClaudeSays text={input.description} />
        <ApprovalBox lines={countLines(input.command)} {...box}>
          <span className="text-faint select-none">$ </span>
          <Shown text={input.command} />
        </ApprovalBox>
        <OtherFields input={input} shown={["command", "description"]} />
      </>
    );
  }
  if (tool === "Write" && path && typeof input?.content === "string") {
    return (
      <>
        {target("Create or overwrite")}
        <ApprovalBox lines={countLines(input.content)} {...box}>
          <Shown text={input.content} />
        </ApprovalBox>
        <OtherFields input={input} shown={["file_path", "content"]} />
      </>
    );
  }
  if ((tool === "Edit" || tool === "MultiEdit") && path) {
    const list: unknown[] = tool === "Edit" ? [input] : Array.isArray(input.edits) ? input.edits : [];
    const edits = list.filter((e): e is Record<string, unknown> => !!e && typeof e === "object").map((e) => ({ old: String(e.old_string ?? ""), new: String(e.new_string ?? ""), all: e.replace_all === true }));
    return (
      <>
        {target("Edit")}
        <ApprovalBox lines={countLines(...edits.flatMap((e) => [e.old, e.new]))} {...box}>
          {edits.map((e, i) => (
            <div key={i} className={i > 0 ? "mt-3 border-t border-line pt-3" : ""}>
              {e.all && <div className="mb-1 font-sans font-medium text-s-needs">Every occurrence in the file, not only this one:</div>}
              {e.old.split("\n").map((l, j) => (
                <div key={`o${j}`} className="bg-s-error/10 text-s-error">
                  - <Shown text={l} />
                </div>
              ))}
              {e.new.split("\n").map((l, j) => (
                <div key={`n${j}`} className="bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
                  + <Shown text={l} />
                </div>
              ))}
            </div>
          ))}
        </ApprovalBox>
        <OtherFields input={input} shown={["file_path", "old_string", "new_string", "replace_all", "edits"]} />
      </>
    );
  }
  const json = JSON.stringify(input, null, 2) ?? "";
  return (
    <>
      <ClaudeSays text={description} />
      <ApprovalBox lines={countLines(json)} {...box}>
        <Shown text={json} />
      </ApprovalBox>
    </>
  );
}

function Collapsible({ icon, label, body }: { icon: ReactNode; label: ReactNode; body: string }) {
  const [open, setOpen] = useState(false);
  return (
    <Activity icon={icon}>
      <button onClick={() => setOpen(!open)} aria-expanded={open} className="flex items-center gap-1 text-muted hover:text-fg">
        {label}
        <ChevronRight className={cx("size-3 transition-transform", open && "rotate-90")} />
      </button>
      {open && <pre className="mt-2 max-h-96 overflow-auto rounded-md bg-code p-3 font-sans text-[12px] whitespace-pre-wrap text-muted ring-1 ring-inset ring-line">{body}</pre>}
    </Activity>
  );
}

function ToolRow({ row, cwd, live }: { row: Extract<Row, { kind: "tool" }>; cwd: string; live: boolean }) {
  const [open, setOpen] = useState(false);
  const Icon = TOOL_ICONS[row.name] ?? Hammer;
  const summary = toolSummary(row.name, row.input, cwd);
  // A question or a plan that was turned down is not a tool that broke: the row under it says what happened.
  const dialog = row.name === ASK_TOOL || row.name === PLAN_TOOL;
  const failed = row.result?.isError && !dialog;
  // What an edit changed, once it has run: the lines Claude Code reported, or, if it did not
  // go through, the change that was asked for.
  const diff = useMemo(() => diffOf(row.name, row.input, row.result), [row]);
  const plan = row.name === PLAN_TOOL ? planOf(row.input) : null;
  return (
    <Activity icon={<Icon className="size-3" />} tone={failed ? "error" : undefined}>
      <button onClick={() => setOpen(!open)} aria-expanded={open} className="group flex w-full min-w-0 items-center gap-2 text-left">
        <span className="shrink-0 font-medium text-fg">{TOOL_NAMES[row.name] ?? visible(row.name.replace(/^mcp__/, "").replace(/__/g, " · "))}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-faint group-hover:text-muted">{summary}</span>
        {diff && !failed && (row.result?.patch as { type?: unknown } | undefined)?.type === "create" && <span className="shrink-0 text-[11px] text-faint">new file</span>}
        {diff && !failed && <DiffStat diff={diff} />}
        {!row.result && live && <span className="size-1.5 shrink-0 animate-soft-pulse rounded-full bg-s-working" title="Running" />}
        {!row.result && !live && <span className="shrink-0 text-[11px] text-faint">no result</span>}
        {failed && <X className="size-3.5 shrink-0 text-s-error" />}
        <ChevronRight className={cx("size-3 shrink-0 text-faint transition-transform", open && "rotate-90")} />
      </button>
      {diff && failed && <p className="mt-1 text-[12px] [overflow-wrap:anywhere] text-s-error">Not applied: {visible(row.result?.text.split("\n")[0] ?? "").slice(0, 200)}</p>}
      {diff && <DiffView diff={diff} dim={failed} />}
      {open && (
        <div className="mt-2 space-y-2">
          {plan ? (
            <div className="rounded-md bg-code px-4 py-3 text-[13px] ring-1 ring-inset ring-line">
              <Markdown text={plan.plan} />
            </div>
          ) : (
            <pre className="max-h-72 overflow-auto rounded-md bg-code p-2.5 font-mono text-[11.5px] whitespace-pre-wrap text-muted ring-1 ring-inset ring-line">
              {JSON.stringify(row.input, null, 2)}
            </pre>
          )}
          {row.result && (
            <pre
              className={cx(
                "max-h-96 overflow-auto rounded-md bg-code p-2.5 font-mono text-[11.5px] whitespace-pre-wrap ring-1 ring-inset ring-line",
                failed ? "text-s-error" : "text-muted",
              )}
            >
              {row.result.text || "(no output)"}
            </pre>
          )}
        </div>
      )}
    </Activity>
  );
}

/** Claude Code's words for how a helper agent ended. */
function agentOutcome(row: ToolRowData, state: string): string {
  if (state === "completed") {
    const used = agentUsage(row.agent, true);
    return used ? `Done (${used})` : "Done";
  }
  if (state === "failed") {
    // Why the Agent call itself failed (an unknown kind of agent, a refusal), in Claude Code's words.
    const why = row.result?.isError ? visible(row.result.text.trim().split("\n")[0]).slice(0, 200) : "";
    return why ? `Failed: ${why}` : "Failed";
  }
  return state === "stopped" ? "Stopped" : state === "unfinished" ? "Did not finish" : "";
}

/**
 * A helper agent Claude started with its Agent tool. While it runs, the row shows what Claude
 * Code's terminal shows: "Initializing…", then its newest lines (see `agentLines`) and how many
 * more there were. Under them, hangar adds Claude Code's count of tool uses and tokens. When it
 * has ended, the row says how, with Claude Code's own totals. Opened, it shows the prompt, every
 * step and the agent's report, as the terminal's transcript view does.
 */
function AgentRow({ row, session }: { row: ToolRowData; session: Session }) {
  const [open, setOpen] = useState(false);
  const [stopping, setStopping] = useState(false);
  const agent = row.agent;
  const running = agentRunning(session, row);
  // Before Claude Code has announced the agent, the call is one more tool call that is running.
  const starting = !running && !agent && !row.result && LIVE.has(session.status);
  const state = running
    ? "running"
    : starting
      ? "starting"
      : (agent?.ended ?? (row.result && !agent?.background ? (row.result.isError ? "failed" : "completed") : agent?.id !== undefined || !row.result ? "unfinished" : "unknown"));
  const active = state === "running" || state === "starting";
  const type = typeof row.input?.subagent_type === "string" && row.input.subagent_type !== "general-purpose" ? visible(row.input.subagent_type).slice(0, 40) : null;
  const prompt = typeof row.input?.prompt === "string" ? row.input.prompt : null;
  const steps = agent?.steps ?? [];
  // The agent's last words are its report.
  const last = steps[steps.length - 1];
  const answer = state === "completed" && last?.kind === "text" ? last : null;
  const { lines, hidden } = agentLines(agent ?? { background: false, steps: [], rev: 0 }, running);
  const used = agentUsage(agent, false);
  const outcome = agentOutcome(row, state);
  const stop = () => {
    if (!agent?.id) return;
    setStopping(true);
    api
      .stopAgent(session.id, agent.id)
      .catch(report)
      .finally(() => setStopping(false));
  };
  return (
    <Activity icon={<Bot className="size-3" />} tone={state === "failed" ? "error" : undefined}>
      <div data-agent={row.id} data-agent-state={state}>
        <div className="flex w-full min-w-0 items-center gap-2">
          <button onClick={() => setOpen(!open)} aria-expanded={open} className="group flex min-w-0 flex-1 items-center gap-2 text-left">
            <span className="shrink-0 font-medium text-fg">Agent</span>
            {type && <span className="shrink-0 rounded bg-raised px-1.5 py-px text-[11px] text-muted">{type}</span>}
            <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-faint group-hover:text-muted">{toolSummary(row.name, row.input, session.cwd)}</span>
          </button>
          {active && agent?.background && <span className="shrink-0 text-[11px] text-faint">in background</span>}
          {running && agent?.id && (
            <button
              data-agent-stop
              disabled={stopping}
              onClick={stop}
              title="Stop this agent. Whatever else is running carries on."
              className="shrink-0 rounded px-1.5 py-px text-[11px] text-muted ring-1 ring-inset ring-line-strong hover:text-fg disabled:opacity-50"
            >
              Stop
            </button>
          )}
          {active && <span className="size-1.5 shrink-0 animate-soft-pulse rounded-full bg-s-working" title="Running" />}
          <button onClick={() => setOpen(!open)} aria-label={open ? "Hide the agent's steps" : "Show the agent's steps"} className="shrink-0 text-faint hover:text-fg">
            <ChevronRight className={cx("size-3 transition-transform", open && "rotate-90")} />
          </button>
        </div>
        {!open && active && (
          <div data-agent-live className="mt-1 space-y-0.5 border-l border-line-strong pl-3 text-[12px] text-muted">
            {lines.length === 0 ? (
              <p className="text-faint">Initializing…</p>
            ) : (
              <>
                {lines.map((l) =>
                  l.kind === "summary" ? (
                    <p key={l.key} className="truncate">
                      {readSearchSummary(l.searches, l.reads, l.active)}
                    </p>
                  ) : l.kind === "text" ? (
                    <p key={l.key} className="truncate text-faint">
                      {visible(l.text.trim().split("\n")[0]).slice(0, 200)}
                    </p>
                  ) : (
                    <p key={l.row.key} className="flex min-w-0 items-center gap-1.5">
                      <span className="shrink-0 text-fg">{visible(l.row.name.replace(/^mcp__/, "").replace(/__/g, " · "))}</span>
                      <span className="min-w-0 truncate font-mono text-[11.5px] text-faint">{toolSummary(l.row.name, l.row.input, session.cwd)}</span>
                      {!l.row.result && <span className="size-1 shrink-0 animate-soft-pulse rounded-full bg-s-working" />}
                    </p>
                  ),
                )}
                {hidden > 0 && (
                  <p className="text-faint">
                    +{hidden} more {hidden === 1 ? "tool use" : "tool uses"}
                  </p>
                )}
              </>
            )}
            {/* Claude Code's terminal says this in place of the lines when its window is too small for them. */}
            {used && <p className="text-faint">In progress… · {used}</p>}
          </div>
        )}
        {!open && !active && outcome && (
          <p data-agent-outcome className={cx("mt-1 border-l border-line-strong pl-3 text-[12px]", state === "failed" ? "text-s-error" : "text-faint")}>
            {outcome}
          </p>
        )}
        {open && (
          <div data-agent-steps className="mt-2 space-y-2.5 border-l border-line-strong pl-3">
            {prompt && (
              <div>
                <p className="text-[11px] font-medium text-faint">Prompt</p>
                <pre className="mt-1 max-h-60 overflow-auto rounded-md bg-code p-2.5 font-sans text-[12px] whitespace-pre-wrap text-muted ring-1 ring-inset ring-line">{visible(prompt)}</pre>
              </div>
            )}
            {steps.length === 0 && active && <p className="text-[12px] text-faint">Initializing…</p>}
            {steps.map((s) =>
              s === answer ? null : s.kind === "tool" ? (
                AGENT_TOOLS.has(s.name) ? (
                  <AgentRow key={s.key} row={s} session={session} />
                ) : (
                  <ToolRow key={s.key} row={s} cwd={session.cwd} live={running} />
                )
              ) : s.kind === "text" ? (
                <div key={s.key} className="text-[13px] text-muted">
                  <Markdown text={s.text} />
                </div>
              ) : null,
            )}
            {answer && (
              <div>
                <p className="text-[11px] font-medium text-faint">Response</p>
                <div className="mt-1 text-[13px]">
                  <Markdown text={answer.text} />
                </div>
              </div>
            )}
            {active && used && <p className="text-[12px] text-faint">In progress… · {used}</p>}
            {!active && outcome && <p className={cx("text-[12px]", state === "failed" ? "text-s-error" : "text-faint")}>{outcome}</p>}
          </div>
        )}
      </div>
    </Activity>
  );
}

const DECISIONS: Record<string, string> = {
  allow: "Allowed",
  allow_always: "Allowed for the session",
  deny: "Denied",
  cancelled: "Withdrawn",
};

function PermissionRow({ row, session }: { row: Extract<Row, { kind: "permission" }>; session: Session }) {
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [clipped, setClipped] = useState(false);
  const pending = row.decision == null && !row.superseded ? session.pending.find((p) => p.request_id === row.requestId) : undefined;
  const decide = async (behavior: "allow" | "allow_always" | "deny") => {
    setBusy(true);
    try {
      await api.decide(session.id, row.requestId, behavior);
    } catch (e) {
      report(e);
      setBusy(false);
    }
  };
  const summary = toolSummary(row.tool, row.input, session.cwd);
  const rules = pending ? sessionRules(pending.suggestions) : [];
  // A helper agent asks for itself. Which one it is, is said with the request.
  const asking = row.agentId ? (session.agents ?? []).find((a) => a.id === row.agentId) : undefined;
  // Allow waits until all of the request is showing.
  const unread = clipped && !expanded;
  const hold = busy || unread;
  // A question or a plan in a shape hangar does not know falls through to the plain request below.
  const questions = row.tool === ASK_TOOL ? questionsOf(row.input) : null;
  const plan = row.tool === PLAN_TOOL ? planOf(row.input) : null;
  // A question is answered in the panel on top of the message box (see SessionPane); the feed says so.
  if (pending && questions) {
    return (
      <Activity icon={<CircleHelp className="size-3" />}>
        <Asking questions={questions} />
      </Activity>
    );
  }
  if (pending && plan) return <PlanCard session={session} requestId={row.requestId} plan={plan.plan} file={plan.file} input={row.input} />;
  if (!pending && row.tool === ASK_TOOL) {
    return (
      <Activity icon={<CircleHelp className="size-3" />}>
        <Answered questions={questions} decision={row.superseded ? null : row.decision} detail={row.detail} />
      </Activity>
    );
  }
  if (!pending && row.tool === PLAN_TOOL) {
    return (
      <Activity icon={<ClipboardList className="size-3" />}>
        <PlanOutcome plan={plan?.plan ?? null} decision={row.superseded ? null : row.decision} detail={row.detail} />
      </Activity>
    );
  }
  if (!pending) {
    const label = DECISIONS[row.decision ?? ""] ?? "Expired";
    return (
      <Activity icon={<Shield className="size-3" />}>
        <span className="text-faint">
          {label} <span className="font-medium text-muted">{row.tool}</span>
          {summary && <span className="font-mono text-[11.5px]"> {summary.slice(0, 90)}</span>}
        </span>
      </Activity>
    );
  }
  return (
    <div className="rounded-lg bg-s-needs/[0.06] p-4 ring-1 ring-inset ring-s-needs/35">
      <div className="flex items-center gap-2 text-[13px] font-medium">
        <ShieldAlert className="size-4 text-s-needs" />
        {row.agentId ? "An agent wants to use" : "Claude wants to use"} {row.tool}
      </div>
      {row.agentId && (
        <p data-asking-agent className="mt-1 text-[12px] [overflow-wrap:anywhere] text-muted">
          Asked by the helper agent{asking ? <>: <span className="text-fg">{visible(asking.description)}</span></> : "."}
        </p>
      )}
      <PermissionPreview tool={row.tool} input={row.input} cwd={session.cwd} description={row.description} expanded={expanded} onExpand={() => setExpanded(true)} onClipped={setClipped} />
      {rightToLeft(JSON.stringify(row.input ?? "")) && (
        <p className="mt-2.5 text-[12px] text-s-needs">
          This request has right-to-left letters in it. On screen they can put parts of a line in a different order than they are written.
        </p>
      )}
      {rules.length > 0 && (
        <p className="mt-2.5 text-[12px] [overflow-wrap:anywhere] text-muted">
          <span className="text-faint">"Allow for session" also allows, without asking again: </span>
          {rules.map((r, i) => (
            <span key={i} className="font-mono text-[11.5px] text-fg">
              {i > 0 && ", "}
              <Shown text={r} />
            </span>
          ))}
        </p>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button variant="primary" size="sm" disabled={hold} onClick={() => decide("allow")}>
          <Check className="size-3.5" /> Allow
        </Button>
        {pending.can_remember && rules.length > 0 && (
          <Button size="sm" disabled={hold} onClick={() => decide("allow_always")} title={`Allow for the rest of this session: ${rules.join(", ")}`}>
            Allow for session
          </Button>
        )}
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => decide("deny")}>
          <X className="size-3.5" /> Deny
        </Button>
        {unread && <span className="text-[12px] text-s-needs">Show all of it to allow it.</span>}
      </div>
    </div>
  );
}
