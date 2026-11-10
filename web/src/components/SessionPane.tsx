import { ArrowUp, Circle, FolderOpen, MoreHorizontal, Pause, Play, Square, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, type Session } from "../api";
import { bytes, PERMISSION_MODES } from "../format";
import { report, useStore } from "../store";
import { Transcript } from "./Transcript";
import { Button, cx, Kbd, StatusChip } from "./ui";

export function SessionPane({ session }: { session: Session }) {
  const events = useStore((s) => s.events[session.id]);
  return (
    <main className="flex min-w-0 flex-1 flex-col">
      <Header session={session} />
      <Transcript session={session} events={events} />
      <Composer session={session} />
    </main>
  );
}

function Header({ session }: { session: Session }) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(session.title);
  const [menu, setMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => setTitle(session.title), [session.title]);
  // The menu closes on Escape or a click anywhere else.
  useEffect(() => {
    if (!menu) return;
    const onDown = (e: MouseEvent) => menuRef.current?.contains(e.target as Node) || setMenu(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setMenu(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [menu]);
  const live = session.pid != null;

  const saveTitle = async () => {
    setEditing(false);
    if (title.trim() && title !== session.title) await api.update(session.id, { title }).catch(report);
  };
  const act = (f: () => Promise<unknown>) => () => {
    setMenu(false);
    f().catch(report);
  };

  return (
    <header className="flex items-center gap-3 border-b border-line px-5 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2.5">
          {editing ? (
            <input
              autoFocus
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              onBlur={saveTitle}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveTitle();
                if (e.key === "Escape") {
                  e.preventDefault();
                  setTitle(session.title);
                  setEditing(false);
                }
              }}
              className="min-w-0 flex-1 rounded-md bg-panel-2 px-2 py-0.5 text-[15px] font-semibold outline-none ring-1 ring-accent"
            />
          ) : (
            <h1 onDoubleClick={() => setEditing(true)} className="truncate text-[15px] font-semibold" title="Double-click to rename">
              {session.title}
            </h1>
          )}
          <StatusChip status={session.status} />
        </div>
        <div className="mt-0.5 flex items-center gap-3 text-[12px] text-faint">
          <span className="flex min-w-0 items-center gap-1 truncate" title={session.cwd}>
            <FolderOpen className="size-3.5 shrink-0" />
            <span className="truncate">{session.cwd}</span>
          </span>
          {session.model && <span className="shrink-0">{session.model}</span>}
          {session.memory_bytes != null && <span className="shrink-0 tabular-nums">{bytes(session.memory_bytes)}</span>}
          {session.cost_usd > 0 && <span className="shrink-0 tabular-nums">${session.cost_usd.toFixed(2)}</span>}
          {session.mcp.length > 0 && (
            <span className="flex shrink-0 items-center gap-1" title={session.mcp.map((m) => `${m.name}: ${m.status}`).join("\n")}>
              <Circle className={cx("size-2", session.mcp.every((m) => m.status === "connected") ? "fill-emerald-400 text-emerald-400" : "fill-amber-400 text-amber-400")} />
              {session.mcp.length} MCP
            </span>
          )}
        </div>
      </div>

      <select
        value={session.permission_mode}
        onChange={(e) => api.update(session.id, { permission_mode: e.target.value }).catch(report)}
        className="h-8 rounded-lg bg-panel-2 px-2 text-[12.5px] ring-1 ring-inset ring-line outline-none hover:bg-raised"
        title={PERMISSION_MODES.find((m) => m.value === session.permission_mode)?.hint}
      >
        {PERMISSION_MODES.map((m) => (
          <option key={m.value} value={m.value}>
            {m.label}
          </option>
        ))}
      </select>

      {session.status === "working" && (
        <Button size="sm" onClick={act(() => api.interrupt(session.id))} title="Interrupt (Esc)">
          <Square className="size-3 fill-current" /> Stop turn
        </Button>
      )}
      {session.status === "queued" && (
        <Button size="sm" onClick={act(() => api.startNow(session.id))} title="Start now, ignoring the memory budget">
          <Play className="size-3.5" /> Start now
        </Button>
      )}

      <div className="relative" ref={menuRef}>
        <Button variant="ghost" size="sm" onClick={() => setMenu(!menu)} aria-label="More" aria-haspopup="menu" aria-expanded={menu}>
          <MoreHorizontal className="size-4" />
        </Button>
        {menu && (
          <div role="menu" className="absolute right-0 z-20 mt-1 w-56 overflow-hidden rounded-xl bg-panel p-1 shadow-xl ring-1 ring-line">
            {live && (
              <MenuItem icon={<Pause className="size-4" />} onClick={act(() => api.stop(session.id))}>
                Stop process <span className="ml-auto text-[11px] text-faint">resumable</span>
              </MenuItem>
            )}
            <MenuItem icon={<Pencil />} onClick={() => (setMenu(false), setEditing(true))}>
              Rename
            </MenuItem>
            <MenuItem
              icon={<Trash2 className="size-4" />}
              danger
              onClick={act(async () => {
                if (confirm(`Delete "${session.title}"? This removes it from hangar; Claude's own transcript stays.`)) await api.remove(session.id);
              })}
            >
              Delete
            </MenuItem>
          </div>
        )}
      </div>
    </header>
  );
}

function Pencil() {
  return (
    <svg viewBox="0 0 24 24" className="size-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M12 20h9M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4Z" />
    </svg>
  );
}

function MenuItem({ icon, children, onClick, danger }: { icon: React.ReactNode; children: React.ReactNode; onClick: () => void; danger?: boolean }) {
  return (
    <button role="menuitem" onClick={onClick} className={cx("flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px]", danger ? "text-rose-400 hover:bg-rose-500/10" : "hover:bg-panel-2")}>
      {icon}
      {children}
    </button>
  );
}

/** Unsent text per session. Outside React: the pane remounts on every switch. */
const drafts = new Map<string, string>();

function Composer({ session }: { session: Session }) {
  const [text, setText] = useState(() => drafts.get(session.id) ?? "");
  const [sending, setSending] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    ref.current?.focus();
  }, [session.id]);
  useEffect(() => {
    if (text) drafts.set(session.id, text);
    else drafts.delete(session.id);
    const el = ref.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
    }
  }, [text, session.id]);

  const send = async () => {
    const t = text.trim();
    if (!t || sending) return;
    setSending(true);
    try {
      await api.send(session.id, t);
      setText("");
    } catch (e) {
      report(e);
    } finally {
      setSending(false);
    }
  };

  const placeholder =
    session.status === "needs_you"
      ? "Answer the request above, or type to steer…"
      : session.status === "hibernated" || session.status === "stopped" || session.status === "error"
        ? "Send a message to resume this session…"
        : session.status === "queued"
          ? "Messages are sent when the session starts…"
          : session.status === "working"
            ? "Queue a follow-up for when Claude finishes…"
            : "Reply to Claude…";

  return (
    <div className="border-t border-line px-6 py-4">
      <div className="mx-auto max-w-3xl">
        <div className="flex items-end gap-2 rounded-2xl bg-panel p-2 ring-1 ring-inset ring-line focus-within:ring-accent/50">
          <textarea
            ref={ref}
            data-composer
            aria-label="Message"
            rows={1}
            value={text}
            placeholder={placeholder}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            className="max-h-60 min-h-9 flex-1 resize-none bg-transparent px-2 py-1.5 text-[14px] outline-none focus-visible:outline-none placeholder:text-faint"
          />
          <button
            onClick={send}
            disabled={!text.trim() || sending}
            className="grid size-9 shrink-0 place-items-center rounded-xl bg-accent text-accent-fg transition disabled:opacity-30"
            aria-label="Send"
          >
            <ArrowUp className="size-4" />
          </button>
        </div>
        <div className="mt-1.5 flex justify-between px-1 text-[11px] text-faint">
          <span>
            <Kbd>Enter</Kbd> send · <Kbd>Shift</Kbd>+<Kbd>Enter</Kbd> new line
          </span>
          {session.status === "working" && (
            <span>
              <Kbd>Esc</Kbd> stop the turn
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
