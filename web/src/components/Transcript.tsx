import { AlertTriangle, Brain, Check, ChevronRight, FileText, Globe, Hammer, Search, ShieldQuestion, SquareTerminal, X } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Session, SessionEvent } from "../api";
import { api } from "../api";
import { duration } from "../format";
import { report } from "../store";
import { relative, toRows, toolSummary, type Row } from "../transcript";
import { Markdown } from "./Markdown";
import { Button, cx } from "./ui";

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

export function Transcript({ session, events }: { session: Session; events: SessionEvent[] | undefined }) {
  const rows = useMemo(() => toRows(events ?? []), [events]);
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
      <div className="mx-auto max-w-3xl space-y-4 px-6 py-6">
        {events === undefined && <p className="text-[13px] text-faint">Loading…</p>}
        {rows.map((r) => (
          <RowView key={r.key} row={r} session={session} />
        ))}
        {(session.status === "working" || session.status === "starting") && (
          <div className="flex items-center gap-2 pl-1 text-[12.5px] text-muted">
            <span className="flex gap-1">
              {[0, 1, 2].map((i) => (
                <span key={i} className="size-1.5 animate-soft-pulse rounded-full bg-sky-400" style={{ animationDelay: `${i * 0.2}s` }} />
              ))}
            </span>
            {session.status === "starting" ? "Starting Claude…" : "Working…"}
          </div>
        )}
        {session.status === "queued" && (
          <div className="rounded-xl border border-dashed border-line-strong px-4 py-3 text-[13px] text-muted">
            <span className="font-medium text-fg">Waiting for memory.</span> {session.status_detail ?? "This session starts as soon as there is room."}
          </div>
        )}
        {session.status === "hibernated" && (
          <p className="text-center text-[12.5px] text-faint">Paused to free memory. Send a message to pick up where it left off.</p>
        )}
      </div>
    </div>
  );
}

function RowView({ row, session }: { row: Row; session: Session }) {
  switch (row.kind) {
    case "user":
      return (
        <div className="flex justify-end">
          <div className="max-w-[85%] whitespace-pre-wrap rounded-2xl rounded-br-md bg-accent-soft px-4 py-2.5 text-[14px] ring-1 ring-inset ring-accent/20">{row.text}</div>
        </div>
      );
    case "text":
      return (
        <div className="text-[14.5px]">
          <Markdown text={row.text} />
        </div>
      );
    case "thinking":
      return <Collapsible icon={<Brain className="size-3.5" />} label="Thinking" body={row.text} />;
    case "tool":
      return <ToolRow row={row} cwd={session.cwd} />;
    case "permission":
      return <PermissionRow row={row} session={session} />;
    case "result":
      return (
        <div className={cx("flex items-center gap-2 border-t border-line pt-2 text-[11.5px]", row.isError ? "text-rose-400" : "text-faint")}>
          {row.isError ? <AlertTriangle className="size-3.5" /> : <Check className="size-3.5" />}
          <span>
            {row.isError ? row.text ?? "The turn failed" : "Done"}
            {row.durationMs != null && ` in ${duration(row.durationMs)}`}
            {row.cost != null && row.cost > 0 && ` · $${row.cost.toFixed(row.cost < 0.1 ? 3 : 2)}`}
          </span>
        </div>
      );
    case "error":
      return (
        <div className="flex items-start gap-2 rounded-xl bg-rose-500/10 px-4 py-3 text-[13px] text-rose-300 ring-1 ring-inset ring-rose-500/25">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" />
          <span>{row.message}</span>
        </div>
      );
  }
}

/** Shows what the user is approving in the most readable form for the tool. */
function PermissionPreview({ tool, input, cwd, description }: { tool: string; input: any; cwd: string; description?: string | null }) {
  const box = "mt-2.5 max-h-72 overflow-auto whitespace-pre-wrap rounded-lg bg-code p-3 font-mono text-[12px] ring-1 ring-inset ring-line";
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
              {String(e.old_string ?? "").split("\n").map((l, j) => (
                <div key={`o${j}`} className="bg-rose-500/10 text-rose-300 [@media(prefers-color-scheme:light)]:text-rose-700">- {l}</div>
              ))}
              {String(e.new_string ?? "").split("\n").map((l, j) => (
                <div key={`n${j}`} className="bg-emerald-500/10 text-emerald-300 [@media(prefers-color-scheme:light)]:text-emerald-700">+ {l}</div>
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

function Collapsible({ icon, label, body, mono }: { icon: React.ReactNode; label: React.ReactNode; body: string; mono?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="rounded-lg">
      <button onClick={() => setOpen(!open)} className="flex items-center gap-1.5 text-[12.5px] text-muted hover:text-fg">
        <ChevronRight className={cx("size-3.5 transition-transform", open && "rotate-90")} />
        {icon}
        {label}
      </button>
      {open && <pre className={cx("mt-2 max-h-96 overflow-auto whitespace-pre-wrap rounded-lg bg-code p-3 text-[12px] text-muted ring-1 ring-inset ring-line", mono !== false && "font-mono")}>{body}</pre>}
    </div>
  );
}

function ToolRow({ row, cwd }: { row: Extract<Row, { kind: "tool" }>; cwd: string }) {
  const [open, setOpen] = useState(false);
  const Icon = TOOL_ICONS[row.name] ?? Hammer;
  const summary = toolSummary(row.name, row.input, cwd);
  const failed = row.result?.isError;
  return (
    <div className={cx("rounded-xl bg-panel ring-1 ring-inset", failed ? "ring-rose-500/30" : "ring-line")}>
      <button onClick={() => setOpen(!open)} className="flex w-full items-center gap-2 px-3 py-2 text-left">
        <ChevronRight className={cx("size-3.5 shrink-0 text-faint transition-transform", open && "rotate-90")} />
        <Icon className="size-3.5 shrink-0 text-muted" />
        <span className="shrink-0 text-[12.5px] font-medium">{row.name.replace(/^mcp__/, "").replace(/__/g, " · ")}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-faint">{summary}</span>
        {!row.result && <span className="size-1.5 shrink-0 animate-soft-pulse rounded-full bg-sky-400" title="Running" />}
        {failed && <X className="size-3.5 shrink-0 text-rose-400" />}
      </button>
      {open && (
        <div className="space-y-2 border-t border-line px-3 py-2.5">
          <pre className="max-h-72 overflow-auto whitespace-pre-wrap rounded-lg bg-code p-2.5 font-mono text-[11.5px] text-muted">{JSON.stringify(row.input, null, 2)}</pre>
          {row.result && (
            <pre className={cx("max-h-96 overflow-auto whitespace-pre-wrap rounded-lg bg-code p-2.5 font-mono text-[11.5px]", failed ? "text-rose-300" : "text-muted")}>
              {row.result.text || "(no output)"}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

function PermissionRow({ row, session }: { row: Extract<Row, { kind: "permission" }>; session: Session }) {
  const [busy, setBusy] = useState(false);
  const live = row.decision == null && session.pending.some((p) => p.request_id === row.requestId);
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
  if (!live) {
    const label = row.decision === "deny" ? "Denied" : row.decision === "allow_always" ? "Always allowed" : row.decision ? "Allowed" : "Expired";
    return (
      <div className="flex items-center gap-2 text-[12px] text-faint">
        <ShieldQuestion className="size-3.5" />
        <span>
          {label} <span className="font-medium text-muted">{row.tool}</span>
          {summary && <span className="font-mono"> {summary.slice(0, 90)}</span>}
        </span>
      </div>
    );
  }
  return (
    <div className="rounded-xl bg-amber-400/[0.07] p-4 ring-1 ring-inset ring-amber-400/30">
      <div className="flex items-center gap-2 text-[13px] font-medium text-amber-300 [@media(prefers-color-scheme:light)]:text-amber-700">
        <ShieldQuestion className="size-4" />
        Claude wants to use {row.tool}
      </div>
      <PermissionPreview tool={row.tool} input={row.input} cwd={session.cwd} description={row.description} />
      <div className="mt-3 flex flex-wrap gap-2">
        <Button variant="primary" disabled={busy} onClick={() => decide("allow")}>
          <Check className="size-4" /> Allow
        </Button>
        <Button disabled={busy} onClick={() => decide("allow_always")} title="Allow this kind of call for the rest of the session">
          Always allow
        </Button>
        <Button variant="ghost" disabled={busy} onClick={() => decide("deny")}>
          <X className="size-4" /> Deny
        </Button>
      </div>
    </div>
  );
}
