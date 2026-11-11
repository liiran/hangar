import {
  AlertTriangle,
  Brain,
  Check,
  ChevronRight,
  FileText,
  Globe,
  Hammer,
  Search,
  Shield,
  ShieldAlert,
  Sparkles,
  SquareTerminal,
  User,
  X,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Session, SessionEvent } from "../api";
import { api } from "../api";
import { ago, duration, usd } from "../format";
import { report } from "../store";
import { useNow } from "../ui";
import { hiddenParts, sessionRules, visible } from "../safe";
import { relative, toRows, toolSummary, type Row } from "../transcript";
import { Markdown } from "./Markdown";
import { Button, cx, Guard } from "./ui";

/** Statuses in which a tool call without a result is still running. */
const LIVE = new Set(["working", "starting", "needs_you"]);

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
};

export function Transcript({ session, events, header }: { session: Session; events: SessionEvent[] | undefined; header?: ReactNode }) {
  const rows = useMemo(() => toRows(events ?? []), [events]);
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
          {rows.map((r) => (
            <Guard key={r.key} fallback={<Unshown row={r} />}>
              <RowView row={r} session={session} />
            </Guard>
          ))}
          {(session.status === "working" || session.status === "starting") && (
            <Activity icon={<Dots />}>
              <span className="text-muted">{session.status === "starting" ? "Starting Claude…" : "Working…"}</span>
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
          </div>
        </div>
      );
    case "thinking":
      return <Collapsible icon={<Brain className="size-3" />} label="Thinking" body={row.text} />;
    case "tool":
      return <ToolRow row={row} cwd={session.cwd} live={LIVE.has(session.status)} />;
    case "permission":
      return <PermissionRow row={row} session={session} />;
    case "result":
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

/**
 * Text the user is asked to approve. Invisible and direction-changing
 * characters are written out and marked, so it cannot read differently
 * from what would run.
 */
function Shown({ text }: { text: string }) {
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
const PREVIEW_HEIGHT = 288;

/**
 * The exact content being approved. It never scrolls and never runs off to
 * the side: long content is cut at a visible line, and `onClipped` tells the
 * row to hold Allow back until the user has opened all of it.
 */
function ApprovalBox({ children, lines, expanded, onExpand, onClipped }: { children: ReactNode; lines: number; expanded: boolean; onExpand: () => void; onClipped: (clipped: boolean) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const now = el.scrollHeight > PREVIEW_HEIGHT + 1;
      setClipped(now);
      onClipped(now);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [children]);
  return (
    <div className="mt-2.5 overflow-hidden rounded-md bg-code ring-1 ring-inset ring-line">
      <div ref={ref} dir="ltr" data-approval-content className="overflow-hidden p-3 font-mono text-[12px] [overflow-wrap:anywhere] whitespace-pre-wrap" style={expanded ? undefined : { maxHeight: PREVIEW_HEIGHT }}>
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
function OtherFields({ input, shown }: { input: any; shown: string[] }) {
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

const countLines = (...texts: string[]) => texts.reduce((n, t) => n + t.split("\n").length, 0);

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
  const failed = row.result?.isError;
  return (
    <Activity icon={<Icon className="size-3" />} tone={failed ? "error" : undefined}>
      <button onClick={() => setOpen(!open)} aria-expanded={open} className="group flex w-full min-w-0 items-center gap-2 text-left">
        <span className="shrink-0 font-medium text-fg">{row.name.replace(/^mcp__/, "").replace(/__/g, " · ")}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-faint group-hover:text-muted">{summary}</span>
        {!row.result && live && <span className="size-1.5 shrink-0 animate-soft-pulse rounded-full bg-s-working" title="Running" />}
        {!row.result && !live && <span className="shrink-0 text-[11px] text-faint">no result</span>}
        {failed && <X className="size-3.5 shrink-0 text-s-error" />}
        <ChevronRight className={cx("size-3 shrink-0 text-faint transition-transform", open && "rotate-90")} />
      </button>
      {open && (
        <div className="mt-2 space-y-2">
          <pre className="max-h-72 overflow-auto rounded-md bg-code p-2.5 font-mono text-[11.5px] whitespace-pre-wrap text-muted ring-1 ring-inset ring-line">
            {JSON.stringify(row.input, null, 2)}
          </pre>
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

const DECISIONS: Record<string, string> = {
  allow: "Allowed",
  allow_always: "Allowed for the session",
  deny: "Denied",
  cancelled: "Withdrawn",
};

/** A new request ignores clicks for this long, so a click meant for the request before it cannot land on this one. */
const ARM_MS = 600;

function PermissionRow({ row, session }: { row: Extract<Row, { kind: "permission" }>; session: Session }) {
  const [busy, setBusy] = useState(false);
  const [armed, setArmed] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [clipped, setClipped] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), ARM_MS);
    return () => clearTimeout(t);
  }, []);
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
  // Allow waits until the request has been on screen a moment and all of it is showing.
  const unread = clipped && !expanded;
  const hold = busy || !armed || unread;
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
        Claude wants to use {row.tool}
      </div>
      <PermissionPreview tool={row.tool} input={row.input} cwd={session.cwd} description={row.description} expanded={expanded} onExpand={() => setExpanded(true)} onClipped={setClipped} />
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
