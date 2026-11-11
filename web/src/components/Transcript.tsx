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
import { relative, toRows, toolSummary, type Row } from "../transcript";
import { Markdown } from "./Markdown";
import { Button, cx } from "./ui";

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
            <RowView key={r.key} row={r} session={session} />
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
      return (
        <div className="rounded-lg bg-panel-2/70 px-4 py-3 ring-1 ring-inset ring-line">
          <div className="mb-1.5 flex items-center gap-2 text-[12px]">
            <Avatar />
            <span className="font-medium">You</span>
            <span className="text-faint">{ago(row.ts)}</span>
          </div>
          <div className="text-[13.5px] leading-relaxed whitespace-pre-wrap">{row.text}</div>
        </div>
      );
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

/** Shows what the user is approving in the most readable form for the tool. */
function PermissionPreview({ tool, input, cwd, description }: { tool: string; input: any; cwd: string; description?: string | null }) {
  const box = "mt-2.5 max-h-72 overflow-auto whitespace-pre-wrap rounded-md bg-code p-3 font-mono text-[12px] ring-1 ring-inset ring-line";
  const path = typeof input?.file_path === "string" ? relative(input.file_path, cwd) : null;
  if (tool === "Bash" && typeof input?.command === "string") {
    return (
      <>
        {input.description && <p className="mt-1 text-[12.5px] text-muted">{input.description}</p>}
        <pre className={box}>
          <span className="text-faint select-none">$ </span>
          {input.command}
        </pre>
      </>
    );
  }
  if (tool === "Write" && path && typeof input?.content === "string") {
    return (
      <>
        <p className="mt-1 text-[12.5px] text-muted">
          Create or overwrite <span className="font-mono text-fg">{path}</span>
        </p>
        <pre className={box}>{input.content}</pre>
      </>
    );
  }
  if ((tool === "Edit" || tool === "MultiEdit") && path) {
    const edits: { old_string: string; new_string: string }[] = tool === "Edit" ? [input] : Array.isArray(input.edits) ? input.edits : [];
    return (
      <>
        <p className="mt-1 text-[12.5px] text-muted">
          Edit <span className="font-mono text-fg">{path}</span>
        </p>
        <div className={box}>
          {edits.map((e, i) => (
            <div key={i} className={i > 0 ? "mt-3 border-t border-line pt-3" : ""}>
              {String(e.old_string ?? "")
                .split("\n")
                .map((l, j) => (
                  <div key={`o${j}`} className="bg-s-error/10 text-s-error">
                    - {l}
                  </div>
                ))}
              {String(e.new_string ?? "")
                .split("\n")
                .map((l, j) => (
                  <div key={`n${j}`} className="bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
                    + {l}
                  </div>
                ))}
            </div>
          ))}
        </div>
      </>
    );
  }
  return (
    <>
      {description && <p className="mt-1 text-[12.5px] text-muted">{description}</p>}
      <pre className={box}>{JSON.stringify(input, null, 2)}</pre>
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

function PermissionRow({ row, session }: { row: Extract<Row, { kind: "permission" }>; session: Session }) {
  const [busy, setBusy] = useState(false);
  const pending = row.decision == null ? session.pending.find((p) => p.request_id === row.requestId) : undefined;
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
      <PermissionPreview tool={row.tool} input={row.input} cwd={session.cwd} description={row.description} />
      <div className="mt-3 flex flex-wrap gap-2">
        <Button variant="primary" size="sm" disabled={busy} onClick={() => decide("allow")}>
          <Check className="size-3.5" /> Allow
        </Button>
        {pending.can_remember && (
          <Button size="sm" disabled={busy} onClick={() => decide("allow_always")} title="Allow calls like this for the rest of this session">
            Allow for session
          </Button>
        )}
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => decide("deny")}>
          <X className="size-3.5" /> Deny
        </Button>
      </div>
    </div>
  );
}
